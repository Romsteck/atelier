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
    category          TEXT         NOT NULL DEFAULT 'autres',  -- axe (par kind), voir RunKind::categories
    status            TEXT         NOT NULL DEFAULT 'open',  -- open | dismissed | resolved
    first_seen        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    last_seen         TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT findings_kind_chk     CHECK (kind IN ('code_review', 'suggestion', 'security')),
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
    CONSTRAINT runs_kind_chk    CHECK (kind IN ('code_review', 'suggestions', 'security')),
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
-- surveillance_config — paramétrage per-app (gates des runs manuels).
-- Pas de scheduler interne : un cron consommerait trop l'abonnement GPT+.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS surveillance_config (
    slug                       TEXT         PRIMARY KEY,
    throttle_threshold         INTEGER      NOT NULL DEFAULT 5,
    max_tokens_per_day         INTEGER      NOT NULL DEFAULT 100000,
    updated_at                 TIMESTAMPTZ  NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- Schema evolution (idempotent) — pour les DB déjà créées avant l'ajout du
-- 3e kind `security`, du champ `category` et du cron sécurité.
-- ---------------------------------------------------------------------------
ALTER TABLE findings           ADD COLUMN IF NOT EXISTS category TEXT NOT NULL DEFAULT 'autres';
ALTER TABLE findings           DROP CONSTRAINT IF EXISTS findings_kind_chk;
ALTER TABLE findings           ADD  CONSTRAINT findings_kind_chk CHECK (kind IN ('code_review', 'suggestion', 'security'));
ALTER TABLE surveillance_runs  DROP CONSTRAINT IF EXISTS runs_kind_chk;
ALTER TABLE surveillance_runs  ADD  CONSTRAINT runs_kind_chk CHECK (kind IN ('code_review', 'suggestions', 'security'));
-- Ajout du statut `cancelled` (kill d'un run in-progress depuis l'UI).
ALTER TABLE surveillance_runs  DROP CONSTRAINT IF EXISTS runs_status_chk;
ALTER TABLE surveillance_runs  ADD  CONSTRAINT runs_status_chk CHECK (status IN ('running', 'success', 'success_empty', 'skipped', 'failed', 'cancelled'));
-- Scheduling retiré (consommait trop l'abonnement GPT+) — purge des colonnes cron.
ALTER TABLE surveillance_config DROP COLUMN IF EXISTS cron_code_review_enabled;
ALTER TABLE surveillance_config DROP COLUMN IF EXISTS cron_suggestions_enabled;
ALTER TABLE surveillance_config DROP COLUMN IF EXISTS cron_security_enabled;
ALTER TABLE surveillance_config DROP COLUMN IF EXISTS code_review_at;
ALTER TABLE surveillance_config DROP COLUMN IF EXISTS suggestions_at;
ALTER TABLE surveillance_config DROP COLUMN IF EXISTS security_at;
-- Promotion findings→todos retirée (système de todos supprimé) — purge statut + colonne.
UPDATE findings SET status = 'open' WHERE status = 'promoted';
ALTER TABLE findings DROP COLUMN IF EXISTS promoted_todo_id;
ALTER TABLE findings DROP CONSTRAINT IF EXISTS findings_status_chk;
ALTER TABLE findings ADD  CONSTRAINT findings_status_chk CHECK (status IN ('open', 'dismissed', 'resolved'));
