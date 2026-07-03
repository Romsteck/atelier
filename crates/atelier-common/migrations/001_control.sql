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
-- platform_issues — remontées de frictions PLATEFORME signalées par les chats
-- Claude Code des apps (Studio) via la skill `0-report-issue`. WHY centralisé
-- ici (et non dans `{app}/src/CLAUDE_ISSUES.json`) : la feature concerne des
-- bugs de la PLATEFORME, pas des apps — le store appartient donc au control-
-- plane Atelier, pas à l'arbre source d'une app. L'ancien fichier per-app a été
-- rapatrié ici une fois puis supprimé (cf. issue_store::backfill_from_files).
--   slug     = app émettrice (pas de FK : même raison que homeroute_routes —
--              le hook AppDelete purge explicitement, évite une dépendance
--              d'ordre au boot/backfill).
--   status   = open | resolved | dismissed
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS platform_issues (
    id          TEXT         PRIMARY KEY,
    slug        TEXT         NOT NULL,
    area        TEXT         NOT NULL DEFAULT 'other',
    severity    TEXT         NOT NULL DEFAULT 'medium',
    title       TEXT         NOT NULL,
    context     TEXT,
    tried       TEXT,
    status      TEXT         NOT NULL DEFAULT 'open',
    note        TEXT,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS platform_issues_status_idx ON platform_issues (status);
CREATE INDEX IF NOT EXISTS platform_issues_slug_idx   ON platform_issues (slug);

-- ---------------------------------------------------------------------------
-- platform_notifications — notifications & journal d'actions plateforme (canal
-- agent → utilisateur). Alimentée par le tool MCP `notify_user` (kind=notice),
-- le journal AUTOMATIQUE des mutations MCP des agents projet (kind=action,
-- inséré par handle_tools_call) et la plateforme elle-même (source=system).
--   source  = agent | scan | system | user   (émetteur)
--   kind    = notice (mérite l'attention de Romain — badge/notif PWA)
--           | action (journal auto — né lu : read_at = created_at)
--   level   = info | warn | error
--   slug    = app émettrice (NULL = plateforme ; pas de FK, même raison que
--             platform_issues — le hook AppDelete purge explicitement)
--   read_at = NULL tant que non lue
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS platform_notifications (
    id          TEXT         PRIMARY KEY,
    slug        TEXT,
    source      TEXT         NOT NULL DEFAULT 'system',
    kind        TEXT         NOT NULL DEFAULT 'notice',
    level       TEXT         NOT NULL DEFAULT 'info',
    title       TEXT         NOT NULL,
    body        TEXT,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    read_at     TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS platform_notifications_unread_idx
    ON platform_notifications (created_at DESC) WHERE read_at IS NULL;
CREATE INDEX IF NOT EXISTS platform_notifications_slug_idx
    ON platform_notifications (slug);

-- ---------------------------------------------------------------------------
-- ---------------------------------------------------------------------------
-- homeroute_settings — singleton de configuration de la liaison vers le reverse
-- proxy Homeroute (hr-api). Atelier appelle l'API EXISTANTE de Homeroute
-- (`{base_url}/api/reverseproxy/*`, sans auth en v1) pour créer/retirer des
-- routes hostname pour ses apps ; Homeroute se charge du reste (hot-reload du
-- proxy, enregistrement DNS, TLS via le wildcard `*.mynetwk.biz` déjà provisionné).
--   bearer_token = réservé v2 (auth) ; masqué en UI, stocké en clair (même
--                  exposition que dataverse-secrets / le .env, root-only).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS homeroute_settings (
    id            INTEGER      PRIMARY KEY DEFAULT 1,
    enabled       BOOLEAN      NOT NULL DEFAULT false,
    base_url      TEXT         NOT NULL DEFAULT 'http://127.0.0.1:4000',
    bearer_token  TEXT,
    updated_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT homeroute_settings_single CHECK (id = 1)
);
INSERT INTO homeroute_settings (id) VALUES (1) ON CONFLICT (id) DO NOTHING;
-- environment_name (v2 allégée) : étiquette de CET environnement Atelier, estampillée
-- sur chaque host créé côté Homeroute (`managedBy:"atelier"` + `environmentName`) pour
-- qu'il s'affiche comme « géré » dans l'UI revprox. NULL ⇒ fallback hostname au runtime.
ALTER TABLE homeroute_settings ADD COLUMN IF NOT EXISTS environment_name TEXT;
-- public_url : URL publique annoncée à Homeroute lors de l'enregistrement (lien
-- retour cliquable dans la page Environnements). registered_at : dernier
-- enregistrement réussi (affiché dans la page Paramètres). NULL ⇒ valeurs par défaut.
ALTER TABLE homeroute_settings ADD COLUMN IF NOT EXISTS public_url TEXT;
ALTER TABLE homeroute_settings ADD COLUMN IF NOT EXISTS registered_at TIMESTAMPTZ;

-- ---------------------------------------------------------------------------
-- homeroute_routes — liaison app Atelier (slug) → host Homeroute. C'est un CACHE
-- de l'uuid renvoyé par Homeroute + du dernier état connu : la SOURCE DE VÉRITÉ
-- reste la config live de Homeroute (`GET /api/reverseproxy/config`). On re-résout
-- toujours l'uuid par `subdomain` avant un PUT/DELETE (jamais d'action sur un uuid
-- périmé). Pas de FK vers `applications` : le hook de suppression d'app lit cette
-- ligne PUIS supprime le host distant ; une FK ON DELETE CASCADE introduirait une
-- dépendance d'ordre (la ligne disparaîtrait avant le nettoyage distant).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS homeroute_routes (
    slug          TEXT         PRIMARY KEY,
    host_id       TEXT         NOT NULL,
    subdomain     TEXT         NOT NULL,
    hostname      TEXT         NOT NULL,
    target_port   INTEGER      NOT NULL,
    require_auth  BOOLEAN      NOT NULL DEFAULT false,
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ  NOT NULL DEFAULT now()
);

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

-- Onglet TOP-NIVEAU du Studio (code/preview/db/…/surveillance) sélectionné par
-- app, + le sous-scan surveillance ciblé par un éventuel deep-link. Source de
-- vérité serveur (suit l'utilisateur entre PCs) + porte le deep-link homepage→
-- Studio via le broadcast WS `studio:tab` (un onglet déjà ouvert bascule live).
-- Colonnes ajoutées à la table existante (idempotent : ce blob rejoue au boot).
ALTER TABLE agent_open_tabs ADD COLUMN IF NOT EXISTS studio_tab  TEXT;
ALTER TABLE agent_open_tabs ADD COLUMN IF NOT EXISTS studio_kind TEXT;

-- ---------------------------------------------------------------------------
-- studio_state — RETIRÉE (2026-06-21). Le singleton « app ouverte » n'a plus de
-- sens depuis que le Studio est une app Vite séparée, ouverte en un onglet par
-- app (`/studio/{slug}`) : l'app vient de l'URL, plus d'une sélection globale.
-- DROP idempotent (ce blob DDL rejoue à chaque boot) pour nettoyer les bases
-- existantes ; la sync per-app `agent_open_tabs` ci-dessus est conservée.
-- ---------------------------------------------------------------------------
DROP TABLE IF EXISTS studio_state;
