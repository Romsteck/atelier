-- atelier-common — control-plane bootstrap DDL (idempotent).
-- Run as `dataverse_admin` on the `atelier_meta` database, alongside the
-- surveillance tables owned by atelier-watcher. All statements are idempotent
-- (CREATE ... IF NOT EXISTS) so the whole blob is safe to re-run on every boot.

-- ---------------------------------------------------------------------------
-- applications — catalogue des apps + assignation de port (fusion de l'ancien
-- apps.json et port-registry.json). `port` est UNIQUE : l'invariant qui rendait
-- nécessaire reconcile_registries est désormais garanti par le schéma.
--   port NULL        = app sans port assigné
--   data (JSONB)     = Application sérialisée (champs non requêtés directement)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS applications (
    slug        TEXT         PRIMARY KEY,
    port        INTEGER      UNIQUE,
    state       TEXT         NOT NULL DEFAULT 'stopped',
    data        JSONB        NOT NULL,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- tasks / task_steps — suivi des tâches de fond (remplace tasks.db SQLite).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tasks (
    id            TEXT         PRIMARY KEY,
    task_type     TEXT         NOT NULL,
    title         TEXT         NOT NULL,
    status        TEXT         NOT NULL DEFAULT 'pending',
    trigger_type  TEXT         NOT NULL,
    trigger_info  TEXT,
    target        TEXT,
    created_at    TIMESTAMPTZ  NOT NULL,
    started_at    TIMESTAMPTZ,
    finished_at   TIMESTAMPTZ,
    error         TEXT
);

CREATE INDEX IF NOT EXISTS tasks_status_idx  ON tasks (status);
CREATE INDEX IF NOT EXISTS tasks_created_idx ON tasks (created_at DESC);
CREATE INDEX IF NOT EXISTS tasks_type_idx    ON tasks (task_type);

CREATE TABLE IF NOT EXISTS task_steps (
    id            TEXT         PRIMARY KEY,
    task_id       TEXT         NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    step_name     TEXT         NOT NULL,
    status        TEXT         NOT NULL DEFAULT 'running',
    started_at    TIMESTAMPTZ  NOT NULL,
    finished_at   TIMESTAMPTZ,
    message       TEXT,
    details       JSONB
);

CREATE INDEX IF NOT EXISTS task_steps_task_idx ON task_steps (task_id);

-- ---------------------------------------------------------------------------
-- doc_entries — index de recherche des docs (remplace docs-index.sqlite/FTS5).
-- Les fichiers .md/.mmd restent la source de vérité ; cette table est un cache
-- reconstructible (rebuild_from_fs). `tsv` est un tsvector généré + index GIN
-- pour la recherche plein-texte (remplace bm25/FTS5).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS doc_entries (
    app_id        TEXT         NOT NULL,
    doc_type      TEXT         NOT NULL,
    name          TEXT         NOT NULL,
    title         TEXT,
    summary       TEXT,
    scope         TEXT,
    parent_screen TEXT,
    code_refs     JSONB,
    links         JSONB,
    has_diagram   BOOLEAN      NOT NULL DEFAULT false,
    body          TEXT         NOT NULL,
    updated_at    TIMESTAMPTZ,
    tsv           tsvector     GENERATED ALWAYS AS (
        to_tsvector('simple',
            coalesce(title, '') || ' ' || coalesce(summary, '') || ' ' || body)
    ) STORED,
    PRIMARY KEY (app_id, doc_type, name)
);

CREATE INDEX IF NOT EXISTS doc_entries_tsv_idx  ON doc_entries USING GIN (tsv);
CREATE INDEX IF NOT EXISTS doc_entries_app_idx  ON doc_entries (app_id);
CREATE INDEX IF NOT EXISTS doc_entries_type_idx ON doc_entries (app_id, doc_type);

-- ---------------------------------------------------------------------------
-- agent_open_tabs — état d'UI du Studio par app : ensemble des onglets ouverts
-- (conversations + fichiers + diffs + commits) et onglet actif. WHY côté serveur :
-- le Studio est utilisé depuis plusieurs PCs contre le même backend Atelier ; cet
-- état doit rester ouvert et SYNCHRONISÉ entre machines (couplé au broadcast WS
-- `agent:open-tabs`). Le localStorage des navigateurs ne reste qu'un cache de repli.
--   tabs (JSONB)  = liste ordonnée de descripteurs (cf. RESTORE_TABS côté front)
--   active        = clé de l'onglet au premier plan (NULL = aucun)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS agent_open_tabs (
    slug        TEXT         PRIMARY KEY,
    tabs        JSONB        NOT NULL DEFAULT '[]'::jsonb,
    active      TEXT,
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- studio_state — singleton : la DERNIÈRE app ouverte dans le Studio (sélection
-- GLOBALE, ≠ agent_open_tabs qui est par-app). WHY côté serveur : restaurer l'app
-- au refresh ET au changement de navigateur/PC (le localStorage est per-browser,
-- donc absent sur un autre poste) ; couplé au broadcast WS `studio:selected-app`
-- pour un suivi live entre PCs. Une seule ligne (id = true), upsert ON CONFLICT (id).
--   selected_app NULL = aucune app ouverte (galerie)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS studio_state (
    id            BOOLEAN      PRIMARY KEY DEFAULT true,
    selected_app  TEXT,
    updated_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT studio_state_singleton CHECK (id)
);
