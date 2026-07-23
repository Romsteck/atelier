-- Atelier Pilote — backlog, runs and autonomous-night control plane.
-- Idempotent: replayed at every Atelier boot on the shared atelier_meta pool.

CREATE TABLE IF NOT EXISTS backlog_items (
    id                BIGSERIAL PRIMARY KEY,
    scope             TEXT NOT NULL,
    title             TEXT NOT NULL,
    request           TEXT NOT NULL,
    description       TEXT NOT NULL DEFAULT '',
    plan              TEXT,
    kind              TEXT NOT NULL DEFAULT 'improvement',
    priority          TEXT NOT NULL DEFAULT 'medium',
    severity          TEXT NOT NULL DEFAULT 'medium',
    effort            TEXT NOT NULL DEFAULT 'm',
    lane              TEXT NOT NULL DEFAULT 'ready',
    position          DOUBLE PRECISION NOT NULL DEFAULT 0,
    exec_status       TEXT NOT NULL DEFAULT 'idle',
    attempts          INTEGER NOT NULL DEFAULT 0,
    engine            TEXT NOT NULL DEFAULT 'auto',
    needs_user        BOOLEAN NOT NULL DEFAULT false,
    needs_user_reason TEXT,
    questions         JSONB NOT NULL DEFAULT '[]'::jsonb,
    session_id        TEXT,
    finding_id        BIGINT,
    last_run_id       UUID,
    commit_shas       JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_by        TEXT NOT NULL DEFAULT 'user',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    done_at           TIMESTAMPTZ,
    CONSTRAINT backlog_items_kind_check CHECK (kind IN ('feature','bug','improvement','finding_fix')),
    CONSTRAINT backlog_items_priority_check CHECK (priority IN ('critical','high','medium','low')),
    CONSTRAINT backlog_items_severity_check CHECK (severity IN ('critical','high','medium','low')),
    CONSTRAINT backlog_items_effort_check CHECK (effort IN ('xs','s','m','l','xl')),
    CONSTRAINT backlog_items_lane_check CHECK (lane IN ('ready','in_progress','attention','done')),
    CONSTRAINT backlog_items_exec_check CHECK (exec_status IN ('idle','queued','running','done','failed','blocked')),
    CONSTRAINT backlog_items_engine_check CHECK (engine IN ('auto','claude','codex')),
    CONSTRAINT backlog_items_created_by_check CHECK (created_by IN ('user','assistant','scan','system'))
);

-- Moteur du dernier run, posé au settle (done ET blocked) — lu par le front.
ALTER TABLE backlog_items ADD COLUMN IF NOT EXISTS last_engine TEXT;

-- Lane `inbox` supprimée (2026-07-22) : les items naissent rédigés et scorés par
-- le chef de projet, il n'y a plus de saisie brute à parquer. Migration des bases
-- existantes (rejouée à chaque boot, idempotente) : les résidus passent en `ready`,
-- puis le CHECK est resserré.
-- Lane `archived` ajoutée (2026-07-23) : rangement manuel des items livrés — la
-- colonne « Terminé » reste courte, l'historique vit dans la vue Archivés.
UPDATE backlog_items SET lane = 'ready' WHERE lane = 'inbox';
ALTER TABLE backlog_items DROP CONSTRAINT IF EXISTS backlog_items_lane_check;
ALTER TABLE backlog_items ADD CONSTRAINT backlog_items_lane_check
    CHECK (lane IN ('ready','in_progress','attention','done','archived'));
ALTER TABLE backlog_items ALTER COLUMN lane SET DEFAULT 'ready';

CREATE INDEX IF NOT EXISTS backlog_items_board_idx ON backlog_items (scope, lane, position, id);
CREATE INDEX IF NOT EXISTS backlog_items_attention_idx ON backlog_items (updated_at DESC) WHERE lane = 'attention';
CREATE UNIQUE INDEX IF NOT EXISTS backlog_items_open_finding_idx
    ON backlog_items (finding_id) WHERE finding_id IS NOT NULL AND lane <> 'done';

CREATE TABLE IF NOT EXISTS backlog_runs (
    id                UUID PRIMARY KEY,
    item_id           BIGINT REFERENCES backlog_items(id) ON DELETE SET NULL,
    scope             TEXT NOT NULL,
    run_kind          TEXT NOT NULL DEFAULT 'item',
    trigger           TEXT NOT NULL DEFAULT 'manual',
    attempt           INTEGER NOT NULL DEFAULT 1,
    engine            TEXT NOT NULL DEFAULT 'claude',
    model             TEXT,
    phase             TEXT NOT NULL DEFAULT 'checkpoint',
    status            TEXT NOT NULL DEFAULT 'running',
    failure_reason    TEXT,
    started_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at       TIMESTAMPTZ,
    tokens_in         BIGINT,
    tokens_out        BIGINT,
    checkpoint_sha    TEXT,
    git_sha_before    TEXT,
    commit_sha        TEXT,
    report            TEXT,
    transcript_tail   TEXT,
    error             TEXT,
    CONSTRAINT backlog_runs_kind_check CHECK (run_kind IN ('item','findings')),
    CONSTRAINT backlog_runs_trigger_check CHECK (trigger IN ('night','manual')),
    CONSTRAINT backlog_runs_attempt_check CHECK (attempt BETWEEN 1 AND 3),
    CONSTRAINT backlog_runs_engine_check CHECK (engine IN ('claude','codex')),
    CONSTRAINT backlog_runs_status_check CHECK (status IN ('running','success','failed','cancelled','attention'))
);

CREATE INDEX IF NOT EXISTS backlog_runs_item_idx ON backlog_runs (item_id, started_at DESC);
CREATE INDEX IF NOT EXISTS backlog_runs_status_idx ON backlog_runs (status, started_at DESC);

CREATE TABLE IF NOT EXISTS pilot_schedule (
    id                BOOLEAN PRIMARY KEY DEFAULT TRUE,
    enabled           BOOLEAN NOT NULL DEFAULT false,
    start_hour        INTEGER NOT NULL DEFAULT 1,
    end_hour          INTEGER NOT NULL DEFAULT 7,
    max_concurrent    INTEGER NOT NULL DEFAULT 2,
    include_atelier   BOOLEAN NOT NULL DEFAULT true,
    resolve_findings  BOOLEAN NOT NULL DEFAULT true,
    engine_policy     TEXT NOT NULL DEFAULT 'claude',
    last_run_at       TIMESTAMPTZ,
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT pilot_schedule_single CHECK (id = TRUE),
    CONSTRAINT pilot_schedule_start_check CHECK (start_hour BETWEEN 0 AND 23),
    CONSTRAINT pilot_schedule_end_check CHECK (end_hour BETWEEN 0 AND 23),
    CONSTRAINT pilot_schedule_concurrency_check CHECK (max_concurrent BETWEEN 1 AND 4),
    CONSTRAINT pilot_schedule_engine_check CHECK (engine_policy IN ('claude','auto'))
);
INSERT INTO pilot_schedule (id) VALUES (TRUE) ON CONFLICT (id) DO NOTHING;

CREATE TABLE IF NOT EXISTS pilot_night (
    id          BOOLEAN PRIMARY KEY DEFAULT TRUE,
    status      TEXT NOT NULL DEFAULT 'idle',
    started_at  TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    stats       JSONB NOT NULL DEFAULT '{}'::jsonb,
    atelier_unit TEXT,
    secret      TEXT,
    CONSTRAINT pilot_night_single CHECK (id = TRUE),
    CONSTRAINT pilot_night_status_check CHECK (status IN ('idle','running','waiting_atelier','done','failed','cancelled'))
);
INSERT INTO pilot_night (id) VALUES (TRUE) ON CONFLICT (id) DO NOTHING;

-- Triage automatique des remontées plateforme (2026-07-23) : une remontée
-- d'agent (issue_report / POST /api/apps/{slug}/issues) est enfilée ICI plutôt
-- que dans l'ex-table platform_issues. Une instance headless du chef de projet
-- (run Claude lecture seule) la lit, investigue, et crée un item de backlog.
-- La TABLE EST LA FILE : le dispatcher single-flight claim la plus ancienne
-- ligne `pending` ; au boot les `running` (run tué par un restart) repassent
-- `pending` et sont rejoués — restart-safe par construction (pas de file mémoire).
CREATE TABLE IF NOT EXISTS pilot_triage (
    id              BIGSERIAL PRIMARY KEY,
    slug            TEXT NOT NULL,
    payload         JSONB NOT NULL,            -- {title,kind,area,severity,context,tried}
    status          TEXT NOT NULL DEFAULT 'pending',
    outcome         TEXT,                      -- planned|needs_user|duplicate|rejected|fallback
    attempts        INTEGER NOT NULL DEFAULT 0,
    backlog_item_id BIGINT,
    error           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT pilot_triage_status_check CHECK (status IN ('pending','running','done','failed'))
);
CREATE INDEX IF NOT EXISTS pilot_triage_pending_idx ON pilot_triage (id) WHERE status IN ('pending','running');

