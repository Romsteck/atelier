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
--   kind     = error | limitation | suggestion (axe de nature — les lignes
--              antérieures à l'axe sont des frictions → DEFAULT 'error')
--   status   = open | resolved | dismissed
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS platform_issues (
    id          TEXT         PRIMARY KEY,
    slug        TEXT         NOT NULL,
    kind        TEXT         NOT NULL DEFAULT 'error',
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

ALTER TABLE platform_issues ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'error';

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
-- agent_conversation_meta — réglages par conversation agent (modèle/effort/mode).
-- WHY côté serveur : ces réglages doivent suivre l'utilisateur entre PCs (le
-- localStorage ne le peut pas) — sans eux, rouvrir une conversation depuis un
-- autre navigateur la relançait sur le modèle/effort par défaut. model NULL =
-- défaut abonnement (Opus [1m]). Upserté au binding de session (query/resume) +
-- sur set_model/set_mode live. Purge : delete conversation + delete app.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS agent_conversation_meta (
    slug        TEXT         NOT NULL,
    session_id  TEXT         NOT NULL,
    model       TEXT,
    effort      TEXT,
    mode        TEXT,
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    PRIMARY KEY (slug, session_id)
);
-- engine — moteur d'agent de la conversation ('claude' | 'codex'), FIGÉ au binding
-- de session. WHY une colonne plutôt qu'une déduction : les deux moteurs stockent
-- leurs transcripts dans des espaces disjoints (sessions SDK Claude vs threads
-- Codex) — sans cet axe, l'API ne saurait pas à quel runner adresser un
-- resume/list/delete pour un sessionId donné. DEFAULT 'claude' : tout le legacy
-- est Claude, les lignes existantes se qualifient donc correctement d'office.
ALTER TABLE agent_conversation_meta ADD COLUMN IF NOT EXISTS engine TEXT NOT NULL DEFAULT 'claude';

-- ---------------------------------------------------------------------------
-- agent_auth — singleton : token OAuth abonnement LONGUE DURÉE du runner/scan
-- (produit par `claude setup-token` sur un poste avec navigateur, ~1 an, inference-
-- only) + télémétrie d'auth. WHY côté serveur : le runner tourne headless en
-- hr-studio → impossible d'y relancer `claude login` (flow navigateur) ; quand le
-- refresh token meurt (authentication_failed), Romant recolle un token depuis
-- Paramètres → Authentification Claude, injecté au runner par stdin (JAMAIS argv/env :
-- sudo journalise l'env — même anti-leak que MCP_TOKEN). En clair (base root-only,
-- même exposition que homeroute_settings.bearer_token / dataverse-secrets / le .env).
--   token            = setup-token (NULL = non configuré → fallback .credentials.json)
--   last_ok_at       = dernier smoke-test / run authentifié OK
--   last_error_at    = dernière authentication_failed observée
--   last_notified_at = watermark de dédup des notifications (claim atomique cross-
--                      subsystem : un token mort touche chaque scan du sweep + l'agent)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS agent_auth (
    id               INTEGER      PRIMARY KEY DEFAULT 1,
    token            TEXT,
    updated_at       TIMESTAMPTZ,
    last_ok_at       TIMESTAMPTZ,
    last_error_at    TIMESTAMPTZ,
    last_error_msg   TEXT,
    last_notified_at TIMESTAMPTZ,
    created_at       TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT agent_auth_single CHECK (id = 1)
);
INSERT INTO agent_auth (id) VALUES (1) ON CONFLICT (id) DO NOTHING;

-- ---------------------------------------------------------------------------
-- app_claude_auth — singleton : token Claude LONGUE DURÉE destiné aux APPS
-- (produit par `claude setup-token`, ~1 an, inference-only). SÉPARÉ d'agent_auth
-- (token du runner/scan plateforme) : une app est un tiers moins fiable que
-- l'agent plateforme → un credential dédié, révocable indépendamment, borne le
-- rayon de fuite. Injecté aux apps opt-in (Application.claude_access) comme var
-- plateforme calculée CLAUDE_CODE_OAUTH_TOKEN — remplace le hack où une app
-- pointait CLAUDE_CONFIG_DIR sur /var/lib/hr-studio/.claude et clobberait
-- .credentials.json en root:root (iss-d10ef97b). En clair (base root-only, même
-- exposition que agent_auth / dataverse-secrets / le .env rendu).
--   token         = setup-token (NULL = non configuré → aucune injection)
--   last_ok_at    = dernier smoke-test OK
--   last_error_at = dernier smoke-test en échec
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS app_claude_auth (
    id             INTEGER      PRIMARY KEY DEFAULT 1,
    token          TEXT,
    updated_at     TIMESTAMPTZ,
    last_ok_at     TIMESTAMPTZ,
    last_error_at  TIMESTAMPTZ,
    last_error_msg TEXT,
    created_at     TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT app_claude_auth_single CHECK (id = 1)
);
INSERT INTO app_claude_auth (id) VALUES (1) ON CONFLICT (id) DO NOTHING;

-- ---------------------------------------------------------------------------
-- codex_auth — singleton : authentification du moteur Codex (OAuth abonnement
-- ChatGPT UNIQUEMENT, jamais de clé API) + télémétrie d'auth.
-- WHY ce store ne fait PAS autorité, contrairement à agent_auth : la vérité
-- runtime de Codex est le fichier $CODEX_HOME/auth.json, que le CLI relit ET
-- réécrit seul (rotation du refresh token à chaque tour). Ce que porte la table,
-- c'est le SEED de ce fichier — le contenu d'auth.json collé depuis Paramètres,
-- gardé ici pour (1) restaurer le fichier après une perte de /var/lib, (2) servir
-- le statut à l'UI, (3) dédupliquer les notifications d'expiration. Le flow
-- device-login, lui, écrit auth.json DIRECTEMENT via le CLI et ne passe pas par
-- PG : `configured=false` avec un auth.json présent est donc un état NORMAL, pas
-- une incohérence. En clair (base root-only, même exposition qu'agent_auth).
--   token            = seed auth.json collé (NULL = jamais collé)
--   last_ok_at       = dernier tour/smoke-test authentifié OK
--   last_error_at    = dernière panne d'auth observée
--   last_notified_at = watermark de dédup des notifications (claim atomique)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS codex_auth (
    id               INTEGER      PRIMARY KEY DEFAULT 1,
    token            TEXT,
    updated_at       TIMESTAMPTZ,
    last_ok_at       TIMESTAMPTZ,
    last_error_at    TIMESTAMPTZ,
    last_error_msg   TEXT,
    last_notified_at TIMESTAMPTZ,
    created_at       TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT codex_auth_single CHECK (id = 1)
);
INSERT INTO codex_auth (id) VALUES (1) ON CONFLICT (id) DO NOTHING;

-- ---------------------------------------------------------------------------
-- studio_state — RETIRÉE (2026-06-21). Le singleton « app ouverte » n'a plus de
-- sens depuis que le Studio est une app Vite séparée, ouverte en un onglet par
-- app (`/studio/{slug}`) : l'app vient de l'URL, plus d'une sélection globale.
-- DROP idempotent (ce blob DDL rejoue à chaque boot) pour nettoyer les bases
-- existantes ; la sync per-app `agent_open_tabs` ci-dessus est conservée.
-- ---------------------------------------------------------------------------
DROP TABLE IF EXISTS studio_state;

-- ===========================================================================
-- Statistiques d'utilisation (page /stats du panneau de contrôle). Trois tables
-- alimentées par des instrumentations légères, séparées du reste du control-plane
-- pour être débrayables sans impact. Toutes no-op si Postgres est down (le store
-- UsageStatsStore dégrade en silencieux). Store : atelier_common::usage_stats.
-- ===========================================================================

-- ---------------------------------------------------------------------------
-- app_traffic_daily — compteurs de trafic HTTP/WS par app agrégés PAR JOUR.
-- WHY par jour (et non une ligne par requête) : le path-proxy voit chaque hit ;
-- écrire en base à chaque requête serait un coût par-requête inacceptable. Un
-- compteur mémoire (ProxyStats) est flushé périodiquement en UPSERT incrémental
-- (`hits = hits + EXCLUDED.hits`, …), coût amorti quasi nul. latency_ms_sum/_n
-- permettent une latence moyenne (time-to-headers upstream) sans stocker chaque
-- valeur. Rétention ~400 j (couvre une heatmap annuelle ; lignes minuscules).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS app_traffic_daily (
    slug            TEXT         NOT NULL,
    day             DATE         NOT NULL,
    hits            BIGINT       NOT NULL DEFAULT 0,
    errors_5xx      BIGINT       NOT NULL DEFAULT 0,
    ws_upgrades     BIGINT       NOT NULL DEFAULT 0,
    latency_ms_sum  BIGINT       NOT NULL DEFAULT 0,
    latency_n       BIGINT       NOT NULL DEFAULT 0,
    PRIMARY KEY (slug, day)
);

-- ---------------------------------------------------------------------------
-- agent_turn_usage — tokens/coût/durée de CHAQUE tour de l'agent Studio. WHY :
-- le scan de surveillance persiste déjà ses tokens dans `surveillance_runs`,
-- mais l'agent interactif ne le faisait PAS (l'event `result` du runner était
-- seulement streamé). Cette table capte le même signal côté Studio → conso
-- Claude par app comparable aux scans. Rétention ~365 j (coût annuel).
--   cache_read / cache_creation = tokens de cache prompt (facturés différemment)
--   is_error                    = tour terminé en erreur (SDK)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS agent_turn_usage (
    id              BIGSERIAL    PRIMARY KEY,
    slug            TEXT         NOT NULL,
    session_id      TEXT,
    ts              TIMESTAMPTZ  NOT NULL DEFAULT now(),
    model           TEXT,
    tokens_in       BIGINT,
    tokens_out      BIGINT,
    cache_read      BIGINT,
    cache_creation  BIGINT,
    cost_usd        DOUBLE PRECISION,
    num_turns       INTEGER,
    duration_ms     BIGINT,
    is_error        BOOLEAN      NOT NULL DEFAULT false
);

CREATE INDEX IF NOT EXISTS agent_turn_usage_slug_ts_idx ON agent_turn_usage (slug, ts DESC);
CREATE INDEX IF NOT EXISTS agent_turn_usage_ts_idx      ON agent_turn_usage (ts DESC);

-- ---------------------------------------------------------------------------
-- app_build_runs — historique des builds/ships des apps. WHY : les AppBuildEvent
-- étaient un broadcast WS ÉPHÉMÈRE (badge live du Studio), rien n'était persisté
-- → aucune fréquence de déploiement observable. Un subscriber central du canal
-- `app_build` matérialise ici started→finished/error (kind déduit de la phase :
-- `ship` si phase="ship", sinon `build`). Réconciliation au boot (running →
-- interrupted, cf. un run tué par un restart d'Atelier). Rétention ~90 j.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS app_build_runs (
    id            UUID         PRIMARY KEY,
    slug          TEXT         NOT NULL,
    kind          TEXT         NOT NULL DEFAULT 'build',   -- build | ship
    started_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    finished_at   TIMESTAMPTZ,
    status        TEXT         NOT NULL DEFAULT 'running', -- running | success | error | interrupted
    duration_ms   BIGINT,
    error         TEXT
);

CREATE INDEX IF NOT EXISTS app_build_runs_slug_idx ON app_build_runs (slug, started_at DESC);
