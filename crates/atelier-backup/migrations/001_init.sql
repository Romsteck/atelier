-- atelier-backup — bootstrap DDL (idempotent)
-- Run as `dataverse_admin` on the `atelier_meta` database.

-- ---------------------------------------------------------------------------
-- backup_target — cible de sauvegarde unique (singleton id=1). `kind` reste
-- extensible (seul 'smb' implémenté). `password` (SMB) et `restic_password`
-- sont des secrets : JAMAIS renvoyés tels quels par l'API (rédigés en booléens
-- has_password / has_restic_password ; le mdp dépôt est révélable via un
-- endpoint dédié pour conservation hors-ligne).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS backup_target (
    id                INTEGER      PRIMARY KEY DEFAULT 1,
    kind              TEXT         NOT NULL DEFAULT 'smb',
    label             TEXT         NOT NULL DEFAULT '',
    host              TEXT         NOT NULL DEFAULT '',     -- serveur SMB (sans //)
    share             TEXT         NOT NULL DEFAULT '',     -- nom du partage
    username          TEXT         NOT NULL DEFAULT '',
    domain            TEXT         NOT NULL DEFAULT '',     -- workgroup / domaine AD
    password          TEXT,                                 -- mot de passe SMB (en clair, rédigé en sortie)
    restic_password   TEXT,                                 -- généré à l'init du dépôt (rédigé/révélable)
    repo_subpath      TEXT         NOT NULL DEFAULT 'atelier-backup',
    schedule_enabled  BOOLEAN      NOT NULL DEFAULT false,  -- planification désactivée par défaut
    schedule_cadence  TEXT         NOT NULL DEFAULT 'daily',
    schedule_hour     SMALLINT     NOT NULL DEFAULT 3,      -- 0..23 (heure locale)
    retention_keep    INTEGER      NOT NULL DEFAULT 7,
    updated_at        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_at        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT backup_target_single      CHECK (id = 1),
    CONSTRAINT backup_target_kind_chk    CHECK (kind IN ('smb')),
    CONSTRAINT backup_target_cadence_chk CHECK (schedule_cadence IN ('daily', 'weekly')),
    CONSTRAINT backup_target_hour_chk    CHECK (schedule_hour BETWEEN 0 AND 23),
    CONSTRAINT backup_target_keep_chk    CHECK (retention_keep >= 1)
);

-- Garantit la ligne singleton (vierge, non configurée).
INSERT INTO backup_target (id) VALUES (1) ON CONFLICT (id) DO NOTHING;

-- Évolution idempotente : remote_subdir retiré (redondant avec repo_subpath).
ALTER TABLE backup_target DROP COLUMN IF EXISTS remote_subdir;

-- ---------------------------------------------------------------------------
-- backup_runs — une ligne par exécution de sauvegarde (manuelle ou planifiée).
-- *_added = octets RÉELLEMENT ajoutés au dépôt restic (delta dédupliqué).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS backup_runs (
    id              UUID         PRIMARY KEY,
    trigger         TEXT         NOT NULL,                  -- 'manual' | 'cron'
    status          TEXT         NOT NULL DEFAULT 'running',-- running | success | failed | cancelled
    phase           TEXT,                                   -- dernière phase atteinte
    started_at      TIMESTAMPTZ  NOT NULL DEFAULT now(),
    finished_at     TIMESTAMPTZ,
    git_added       BIGINT,
    postgres_added  BIGINT,
    config_added    BIGINT,
    total_added     BIGINT,
    total_processed BIGINT,
    error           TEXT,
    CONSTRAINT backup_runs_trigger_chk CHECK (trigger IN ('manual', 'cron')),
    CONSTRAINT backup_runs_status_chk  CHECK (status IN ('running', 'success', 'failed', 'cancelled'))
);

CREATE INDEX IF NOT EXISTS backup_runs_started_idx ON backup_runs (started_at DESC);

-- ---------------------------------------------------------------------------
-- backup_run_snapshots — détail par snapshot restic (3 par run : git/postgres/config).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS backup_run_snapshots (
    run_id          UUID         NOT NULL REFERENCES backup_runs(id) ON DELETE CASCADE,
    tag             TEXT         NOT NULL,                  -- 'git' | 'postgres' | 'config'
    snapshot_id     TEXT,
    status          TEXT         NOT NULL DEFAULT 'success',-- success | failed | skipped
    files           BIGINT       NOT NULL DEFAULT 0,
    bytes_processed BIGINT       NOT NULL DEFAULT 0,
    bytes_added     BIGINT       NOT NULL DEFAULT 0,
    error           TEXT,
    PRIMARY KEY (run_id, tag)
);
