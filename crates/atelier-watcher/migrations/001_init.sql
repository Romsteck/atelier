-- atelier-watcher — bootstrap DDL (idempotent)
-- Run as `dataverse_admin` on the `atelier_meta` database.

-- ---------------------------------------------------------------------------
-- findings — issues raised by Codex (code review or improvement suggestion)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS findings (
    id                BIGSERIAL    PRIMARY KEY,
    slug              TEXT         NOT NULL,
    kind              TEXT         NOT NULL,             -- 'security' | 'code_review' | 'business'
    severity          TEXT         NOT NULL,             -- 'critical' | 'high' | 'medium' | 'low'
    title             TEXT         NOT NULL,
    summary           TEXT         NOT NULL,             -- présentation de l'issue (liste)
    evidence          JSONB,
    plan              TEXT         NOT NULL,             -- document de résolution (annexe markdown)
    fingerprint       TEXT         NOT NULL,
    category          TEXT         NOT NULL DEFAULT 'autres',  -- axe (par kind)
    status            TEXT         NOT NULL DEFAULT 'open',  -- open | dismissed | resolved
    first_seen        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    last_seen         TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT findings_kind_chk     CHECK (kind IN ('security', 'code_review', 'business')),
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
    kind              TEXT         NOT NULL,             -- 'security' | 'code_review' | 'business'
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
    CONSTRAINT runs_kind_chk    CHECK (kind IN ('security', 'code_review', 'business')),
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
-- app_scan — définition du scan `business` (le seul scan possédé par l'agent du
-- projet ; créé/maintenu via le tool MCP `scan_set`, sans validation humaine).
-- Les scans `security` et `code_review` sont des scans plateforme FIXES (prompt
-- en code) et n'ont PAS de ligne ici. Le scan business est VIDE par défaut
-- (prompt='') → en veille.
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

-- Backfill une ligne de scan business VIDE pour chaque app déjà vue en
-- surveillance (le boot loop + AppCreate couvrent le reste).
INSERT INTO app_scan(slug) SELECT DISTINCT slug FROM surveillance_runs ON CONFLICT DO NOTHING;
INSERT INTO app_scan(slug) SELECT DISTINCT slug FROM findings          ON CONFLICT DO NOTHING;

-- Modèle hybride 3 scans : security + code_review (plateforme, fixes) + business
-- (possédé par l'agent). La refonte précédente avait écrasé TOUS les kinds en
-- 'scan' (= le scan agent) ; on le renomme 'business' et on rouvre le CHECK aux
-- 3 kinds. Idempotent (drop CHECK → rename → re-add CHECK). Rename 1:1, pas de
-- collapse → pas de dédup destructive cross-kind.
ALTER TABLE findings           DROP CONSTRAINT IF EXISTS findings_kind_chk;
ALTER TABLE surveillance_runs  DROP CONSTRAINT IF EXISTS runs_kind_chk;
UPDATE findings          SET kind = 'business' WHERE kind = 'scan';
UPDATE surveillance_runs SET kind = 'business' WHERE kind = 'scan';
ALTER TABLE findings           ADD CONSTRAINT findings_kind_chk CHECK (kind IN ('security', 'code_review', 'business'));
ALTER TABLE surveillance_runs  ADD CONSTRAINT runs_kind_chk     CHECK (kind IN ('security', 'code_review', 'business'));
-- Purge des anciennes clés mémoire mono-kind devenues orphelines (gate fraîcheur).
DELETE FROM agent_memory WHERE kind = 'last_run' AND key IN ('scan_sha', 'scan_watermark');

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
