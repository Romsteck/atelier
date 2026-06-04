-- atelier-watcher — bootstrap DDL (idempotent)
-- Run as `dataverse_admin` on the `atelier_meta` database.

-- ---------------------------------------------------------------------------
-- findings — issues raised by Codex (code review or improvement suggestion)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS findings (
    id                BIGSERIAL    PRIMARY KEY,
    slug              TEXT         NOT NULL,
    kind              TEXT         NOT NULL,             -- 'code_review' | 'suggestion'
    severity          TEXT         NOT NULL,             -- 'critical' | 'high' | 'medium' | 'low'
    title             TEXT         NOT NULL,
    summary           TEXT         NOT NULL,
    evidence          JSONB,
    plan              TEXT         NOT NULL,             -- markdown actionnable
    fingerprint       TEXT         NOT NULL,
    category          TEXT         NOT NULL DEFAULT 'autres',  -- axe (cf. app_scan.categories)
    status            TEXT         NOT NULL DEFAULT 'open',  -- open | dismissed | resolved
    first_seen        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    last_seen         TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT findings_kind_chk     CHECK (kind = 'scan'),  -- un seul scan par app (slug discrimine)
    CONSTRAINT findings_severity_chk CHECK (severity IN ('critical', 'high', 'medium', 'low')),
    CONSTRAINT findings_status_chk   CHECK (status IN ('open', 'dismissed', 'resolved'))
);

CREATE UNIQUE INDEX IF NOT EXISTS findings_fingerprint_uniq
    ON findings (slug, kind, fingerprint);

CREATE INDEX IF NOT EXISTS findings_app_status_idx
    ON findings (slug, status, last_seen DESC);

CREATE INDEX IF NOT EXISTS findings_app_kind_idx
    ON findings (slug, kind);

-- ---------------------------------------------------------------------------
-- surveillance_runs — audit log de chaque exécution Codex (cron ou manuel)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS surveillance_runs (
    id                UUID         PRIMARY KEY,
    slug              TEXT         NOT NULL,
    kind              TEXT         NOT NULL,             -- 'code_review' | 'suggestions'
    trigger           TEXT         NOT NULL,             -- 'cron' | 'manual'
    started_at        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    finished_at       TIMESTAMPTZ,
    status            TEXT         NOT NULL DEFAULT 'running',  -- running | success | success_empty | skipped | failed
    skip_reason       TEXT,
    findings_count    INTEGER      NOT NULL DEFAULT 0,
    tokens_in         INTEGER,
    tokens_out        INTEGER,
    git_sha_before    TEXT,
    git_sha_reviewed  TEXT,
    error             TEXT,
    CONSTRAINT runs_kind_chk    CHECK (kind = 'scan'),
    CONSTRAINT runs_trigger_chk CHECK (trigger IN ('cron', 'manual')),
    CONSTRAINT runs_status_chk  CHECK (status IN ('running', 'success', 'success_empty', 'skipped', 'failed'))
);

CREATE INDEX IF NOT EXISTS runs_app_started_idx
    ON surveillance_runs (slug, started_at DESC);

CREATE INDEX IF NOT EXISTS runs_app_kind_idx
    ON surveillance_runs (slug, kind, started_at DESC);

-- ---------------------------------------------------------------------------
-- agent_memory — mémoire structurée injectée à Codex (preferences, dismissed
-- patterns, applied fixes, last_run state). Tri par last_used_at pour
-- l'éviction LRU côté code.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS agent_memory (
    id            BIGSERIAL    PRIMARY KEY,
    slug          TEXT         NOT NULL,
    kind          TEXT         NOT NULL,                 -- dismissed_pattern | recurring_issue | user_preference | last_run | applied_fix | notified
    key           TEXT         NOT NULL,
    value         JSONB        NOT NULL,
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    last_used_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    ttl_at        TIMESTAMPTZ,
    CONSTRAINT memory_kind_chk CHECK (kind IN (
        'dismissed_pattern', 'recurring_issue', 'user_preference',
        'last_run', 'applied_fix', 'notified'
    ))
);

CREATE UNIQUE INDEX IF NOT EXISTS memory_key_uniq
    ON agent_memory (slug, kind, key);

CREATE INDEX IF NOT EXISTS memory_app_used_idx
    ON agent_memory (slug, last_used_at DESC);

-- ---------------------------------------------------------------------------
-- surveillance_config — SUPPRIMÉE. Le seul gate restant est le plafond de
-- findings ouvertes par kind (constante `MAX_OPEN_FINDINGS`, non configurable)
-- + le diff-aware. Plus de budget tokens/jour ni de throttle paramétrable.
-- ---------------------------------------------------------------------------
DROP TABLE IF EXISTS surveillance_config;

-- ---------------------------------------------------------------------------
-- app_scan — UN SEUL scan par app, défini en données et possédé par l'agent du
-- projet (créé/maintenu via le tool MCP `scan_set`, sans validation humaine).
-- Remplace l'enum RunKind (code_review/security/suggestions) + l'ancien portail
-- `scan_type_registry`. Le scan est VIDE par défaut (prompt='') → en veille.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS app_scan (
    slug         TEXT         PRIMARY KEY,
    label        TEXT         NOT NULL DEFAULT '',          -- nom UI choisi par l'agent ; '' = vierge
    prompt       TEXT         NOT NULL DEFAULT '',          -- template avec slots ; '' = en veille (aucun run)
    cadence      TEXT         NOT NULL DEFAULT 'manual',    -- 'manual' | 'daily' | 'weekly'
    gate         TEXT         NOT NULL DEFAULT 'code',      -- 'code' | 'data' | 'manual'
    gate_sql     TEXT,                                      -- SELECT-only watermark (gate='data')
    categories   JSONB        NOT NULL DEFAULT '[]'::jsonb, -- ["bug","perf",...] (cible de coercion)
    updated_by   TEXT,                                      -- 'agent:<slug>' | 'system'
    updated_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT app_scan_gate_chk CHECK (gate IN ('code', 'data', 'manual'))
);

-- ---------------------------------------------------------------------------
-- Schema evolution (idempotent).
-- ---------------------------------------------------------------------------
ALTER TABLE findings           ADD COLUMN IF NOT EXISTS category TEXT NOT NULL DEFAULT 'autres';

-- Backfill une ligne de scan VIDE pour chaque app déjà vue en surveillance
-- (le boot loop + AppCreate couvrent le reste).
INSERT INTO app_scan(slug) SELECT DISTINCT slug FROM surveillance_runs ON CONFLICT DO NOTHING;
INSERT INTO app_scan(slug) SELECT DISTINCT slug FROM findings          ON CONFLICT DO NOTHING;

-- Collapse de TOUS les kinds vers l'unique 'scan' (un seul scan par app).
-- Dédup sur (slug,fingerprint) en gardant la plus récente AVANT le remap (l'index
-- unique est (slug,kind,fingerprint) ; collapser le kind pourrait collisionner).
ALTER TABLE findings           DROP CONSTRAINT IF EXISTS findings_kind_chk;
ALTER TABLE surveillance_runs  DROP CONSTRAINT IF EXISTS runs_kind_chk;
DELETE FROM findings a USING findings b
  WHERE a.slug = b.slug AND a.fingerprint = b.fingerprint AND a.id < b.id;
UPDATE findings          SET kind = 'scan' WHERE kind <> 'scan';
UPDATE surveillance_runs SET kind = 'scan' WHERE kind <> 'scan';
ALTER TABLE findings           ADD CONSTRAINT findings_kind_chk CHECK (kind = 'scan');
ALTER TABLE surveillance_runs  ADD CONSTRAINT runs_kind_chk     CHECK (kind = 'scan');

-- Ajout du statut `cancelled` (kill d'un run in-progress depuis l'UI).
ALTER TABLE surveillance_runs  DROP CONSTRAINT IF EXISTS runs_status_chk;
ALTER TABLE surveillance_runs  ADD  CONSTRAINT runs_status_chk CHECK (status IN ('running', 'success', 'success_empty', 'skipped', 'failed', 'cancelled'));
-- Promotion findings→todos retirée (système de todos supprimé) — purge statut + colonne.
UPDATE findings SET status = 'open' WHERE status = 'promoted';
ALTER TABLE findings DROP COLUMN IF EXISTS promoted_todo_id;
ALTER TABLE findings DROP CONSTRAINT IF EXISTS findings_status_chk;
ALTER TABLE findings ADD  CONSTRAINT findings_status_chk CHECK (status IN ('open', 'dismissed', 'resolved'));

-- Démontage de l'ancien portail de gouvernance (remplacé par app_scan).
DROP TABLE IF EXISTS scan_type_registry;
