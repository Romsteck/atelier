# Atelier — Plateforme applicative HomeRoute

## Statut migration (2026-06-15)

> ✅ **Plateforme stabilisée sur Medion** — 5 apps live (`www:3005`, `home:3007`, `trader:3008`, `wallet:3009`, `myfrigo:3010`), l'app `files` décommissionnée (2026-05-31). Atelier tourne en `atelier.service` (4100) : supervisor + apps API + frontend + passerelle dataverse + agent Claude (Studio) + surveillance IA (Claude Agent SDK) + backup restic.
>
> ✅ **Path-routing interne LIVE** — les apps sont servies via le proxy même-origine `/apps/{slug}/` (`crates/atelier-api/src/routes/apps_proxy.rs`). Les sous-domaines `{slug}.mynetwk.biz` sont **morts** (404 hr-edge) et `app.mynetwk.biz` n'a **plus de route hr-edge**. Hostname externe fonctionnel = **`atelier.mynetwk.biz`** ; accès direct sans auth en local = `http://127.0.0.1:4100/apps/{slug}/`.
>
> ✅ **Control-plane en Postgres** — registres (apps/ports), tasks et index docs migrés des fichiers/SQLite vers la base partagée `atelier_meta` (commit `ac85017`, 2026-05-31 ; déployé dans le binaire installé le 2026-06-11). Les fichiers `/opt/atelier/data/{apps.json,port-registry.json}` + `docs-index.sqlite` ne restent que comme filets de backfill (importés une seule fois, idempotent, au 1er boot post-migration).
>
> ✅ **Source Atelier + binaire sur Medion** — code à `/home/romain/atelier`, `make deploy` build **en place** et installe dans `/opt/atelier`. CloudMaster décommissionné (2026-05-31).

### Historique condensé

- **2026-05-09** — Rapatriement supervisor + apps sur Medion (Atelier autonome, port 4100).
- **2026-05-26** — Système de flux `hr-flow` **éradiqué** (lib + daemon + macros + callback + 34 TOML sur 6 apps). Chaque app refondue en code natif (Rust/TS). 4 crates `hr-flow*` supprimées, daemon `hr-flowd` désinstallé, routes/MCP/UI flow retirés. Cf. [docs/refonte/](docs/refonte/).
- **2026-05-27** — Studio (code-server) rapatrié sur Medion (`atelier-studio.service`, 127.0.0.1:8443). Sources canoniques apps = runtime à `/var/lib/atelier/apps/{slug}/src/`.
- **2026-05-30** — Migration **postgres-dataverse** finalisée + découplage homeroute terminé (crates renommées `atelier-*`). SQLite excisé. Décommission de l'accès Postgres direct (gateway-only).
- **2026-05-31** — Control-plane → Postgres `atelier_meta`. App `files` décommissionnée. CloudMaster décommissionné.
- **2026-06-05 → 06-11** — Agent Claude natif intégré au Studio (multi-conversations, gate de plan, drain au shutdown) ; surveillance IA refondue en 3 scans/app ; backup restic+rclone ; control-plane déployé.
- **2026-06-17** — Éditeur **code-server du Studio décommissionné** : service `atelier-studio.service`/:8443 arrêté+supprimé (+ alias `hr-studio.service`), user-data `/var/lib/atelier/studio/code-server` purgé. Remplacé par l'UI custom (`AgentWorkspace` : files/diffs/commits + agent Claude). `code-server@romain` (:8081, édition du source Atelier) **conservé**. ⏳ Reste à purger côté **homeroute/hr-edge** : la route `codeserver.mynetwk.biz` (pointe désormais vers un upstream :8443 mort).

---

## Migration postgres-dataverse + gateway-only (finalisée 2026-05-30)

Les **5 apps** (www, home, trader, wallet, myfrigo) tournent sur **postgres-dataverse** (bases `app_{slug}`, provisionnées dans `dataverse-secrets.json`). L'ancien moteur **SQLite** a été supprimé (crates `atelier-db` + `atelier-dataverse-migrate` retirées du workspace). Tous les handlers `db_*` routent vers le moteur Postgres (`atelier-dataverse`) ; plus de `db.sqlite`, plus d'env `DB_PATH`/`DATABASE_PATH`. Le SQL brut (`db_query`/`db_exec`) renvoie une erreur dirigée vers `dv_*` / la passerelle REST `/api/dv/{slug}/{table}`.

**Gateway-only.** Les 5 apps n'ont **aucune lecture directe** de Postgres : plus de `DATABASE_URL`/`sqlx` dans leur code, tout passe par la passerelle REST `/api/dv/{slug}` + le contrat `HR_DV_*` :

```
HR_DV_BASE_URL=http://127.0.0.1:4100/api/dv/{slug}
HR_DV_TOKEN=<base64url(32 octets aléatoires)>   # Authorization: Bearer
HR_APP_UUID=<identité stable par app>
```

Aucun `DATABASE_URL` n'est injecté ; le process applicatif n'a **plus aucun moyen** de se connecter directement à Postgres. `HR_DV_*` est désormais un **tier plateforme calculé** du modèle env unifié (cf. ci-dessous), rendu dans le `.env` à chaque reconcile.

## Gestion des variables d'environnement (modèle unifié, 2026-06-16)

> Avant : deux stores non réconciliés (map `env_vars` en Postgres + fichier `.env` à la main), un Save UI qui ne marchait pas (route `PUT` inexistante), des vars mortes (`HR_FLOW_*`) jamais nettoyées. Refondu en **un seul modèle à 3 tiers** ([crates/atelier-api/src/mcp/env_ops.rs](crates/atelier-api/src/mcp/env_ops.rs)).

- **Tiers** : `platform` (calculé, jamais stocké : `PORT`, `HR_DV_BASE_URL`/`HR_DV_TOKEN`/`HR_APP_UUID` si `has_db`, `ATELIER_INGEST_URL`/`ATELIER_LOGS_TOKEN`) · `user config` · `user secret`. Seul le tier user est stocké (champ structuré `Application.env: Vec<EnvVar>` en JSONB ; l'ancienne map `env_vars` est legacy/vide).
- **Secrets** : le flag `secret` pilote le **masquage UI** (vue masquée par défaut, révélation par ligne via `GET .../env/{key}`) ; la valeur est stockée **en clair** dans le JSONB (même exposition que `dataverse-secrets.json`, et que le `.env` rendu + l'unité systemd, root-only). Pas de chiffrement au repos (choix assumé 2026-06-16 : le gain ne couvrait que le dump PG alors que `.env`/state exposent déjà du clair dans le même backup).
- **`.env` = artefact généré** (`/var/lib/atelier/apps/{slug}/.env`), **NE PAS éditer à la main**. `reconcile_app_env` est le **seul writer** : rend une projection déterministe (platform calculé + user déchiffré), GC les vars mortes (denylist `HR_FLOW_*`/`HR_FLOWD_*`/`FLOW_RUNS_DIR`), importe une fois les vars résiduelles hand-seeded. Appelé sur **create / boot-sweep / changement d'env / rotation de token**. Le supervisor lit ce `.env` comme **canal de livraison unique** (identique Node `process.env` et Rust `std::env`).
- **Scope `runtime|build|both`** : `build`/`both` sont aussi exportées avant la commande de build (canal pour `VITE_*`/`NEXT_PUBLIC_*` ; `GET /api/apps/{slug}/build-env` consommé par `build.sh`/`deploy-app.sh`). Runtime-only par défaut → compat des 2 stacks.
- **API** : `GET /api/apps/{slug}/env` (vue structurée, secrets masqués sauf `?reveal=1`) · `GET .../env/{key}` (révèle 1 valeur) · `PUT/DELETE .../env/{key}` (CRUD user, rejette les clés plateforme) · `POST .../reconcile-env` (dry-run par défaut). UI : onglet **Variables** du Studio (tableau ligne-par-ligne, masquage par ligne, badges owner/scope). MCP `app.update env_vars` converge sur le même modèle.
- **Boot-sweep** : gated par `ATELIER_ENV_RECONCILE_APPLY=1` (sinon dry-run/log only). Idempotent une fois migré.

> ⚠️ Les rôles PG `app_{slug}` **gardent `LOGIN`** : la passerelle (`atelier-dataverse`) se connecte à la base `app_{slug}` **en s'authentifiant comme ce rôle** (isolation par app via les credentials de `dataverse-secrets.json`), pas comme `dataverse_admin`. Les passer en `NOLOGIN` casse la passerelle (vérifié 2026-05-30). Ne PAS révoquer `LOGIN` sans re-câbler la passerelle sur un rôle partagé (perdrait l'isolation).

**DbExplorer** écrit via les endpoints admin (`POST/PATCH/DELETE /api/apps/{slug}/db/tables/...`, identité système, capture de version côté serveur pour éviter les races optimistic-lock), plus aucun SQL brut.

> **Bug réglé (ca7c94e, 2026-06-05)** : empoisonnement du cache de prepared-statements (`08P01`). Fix : le littéral NULL est inliné typé pour **tous** les types de colonne nullable (pas seulement jsonb/date/uuid/timestamptz) + contrôle du nombre de paramètres sur les builders CRUD.

---

## Quoi est Atelier

Plateforme applicative autonome (sur Medion, port 4100). Contient :

- **Apps** : supervisor Tokio des apps locales (lifecycle, ports, logs, adoption d'unités orphelines) — services `atelier-app-{slug}.service` (slice `atelier-apps.slice`).
- **Dataverse** : moteur Postgres avec schéma dynamique, passerelle REST gateway-only, dvexpr.
- **Path-proxy** : sert les apps en même-origine sous `/apps/{slug}/` (strip ou no-strip).
- **Studio** : **UI custom d'édition** (`AgentWorkspace` : explorateur/diffs/commits + panneau git) + **agent Claude natif** (Agent SDK Node : chat/raisonnement/planification/approbation).
- **Surveillance IA** : 3 scans Claude Agent SDK (lecture seule, headless) par app (sécurité, qualité, business) + *sweep* automatique (manuel ou planifié) — crate `atelier-watcher`.
- **Backup** : restic + rclone vers SMB (incrémental, chiffré, dédupliqué) — crate `atelier-backup`.
- **Docs** : système de documentation per-app (index de recherche désormais en Postgres `doc_entries`).
- **Git** : bare repos.
- **MCP** : tools `app.*`, `db.*`/`dv.*`, `docs.*`, `git.*`, `scan.*`. (`app.ship`/deploy est exposé en **HTTP-only** via `POST /api/apps/{slug}/ship`, pas comme tool MCP.)

Atelier ne contient **pas** : DNS, DHCP, reverse proxy, ACME (ces concerns restent dans `hr-edge` + `hr-netcore` côté homeroute sur Medion).

## Architecture

```
Internet → Cloudflare → Medion (10.0.0.254)
                          ├─ hr-edge (proxy + ACME + auth + tunnel)
                          │   └─ atelier.mynetwk.biz   → 127.0.0.1:4100  (Atelier API + frontend, 302→/login anonyme)
                          │   ⚠ app.mynetwk.biz n'a PLUS de route edge ; {slug}.mynetwk.biz morts (404) ; codeserver.mynetwk.biz → upstream :8443 mort (service retiré 2026-06-17 ; route hr-edge à purger côté homeroute)
                          ├─ atelier.service (4100) — supervisor + apps API + frontend + dataverse + agent + watcher + backup
                          │   └─ /apps/{slug}/ — path-proxy même-origine vers 127.0.0.1:3005-3010
                          ├─ runner Node (Agent SDK) spawné en hr-studio par atelier.service
                          ├─ code-server@romain.service (127.0.0.1:8081) — édition source Atelier (à la demande, normalement arrêté)
                          ├─ /home/romain/atelier — sources Atelier (édition + make deploy en place)
                          ├─ atelier-app-{home,myfrigo,trader,wallet,www}.service
                          ├─ hr-edge.service / hr-orchestrator.service / homeroute.service
                          └─ Postgres (5432) : bases app_{slug} (dataverse) + atelier_meta (control-plane) + atelier_logs
```

## Stockage

| Données | Chemin |
|---------|--------|
| Sources canoniques apps (= runtime) | `/var/lib/atelier/apps/{slug}/{src,bin,.env,runs}` (Medion) — édition via Studio. Données app dans Postgres-dataverse (`app_{slug}`), plus de `db.sqlite`. |
| Studio user HOME + sessions agent | `/var/lib/hr-studio/` (UID 993) ; sessions agent à `/var/lib/hr-studio/.claude/sessions/{scope}/`, credentials OAuth à `.claude/.credentials.json` |
| Control-plane canonical | **Postgres `atelier_meta`** : apps/ports (`applications`), tasks (`tasks`/`task_steps`), index docs (`doc_entries`, tsvector+GIN), surveillance (`app_scan`/`findings`/`surveillance_runs`), backup (`backup_target`/`backup_runs`/`backup_run_snapshots`), mémoire agent (`agent_memory`) |
| Backfill control-plane (legacy, non-live) | `/opt/atelier/data/{apps.json, port-registry.json}` + `/var/lib/atelier/docs-index.sqlite` — importés 1× au 1er boot post-migration, gardés pour rollback |
| Logs structurés | Postgres `atelier_logs` (ingest via `atelier-logging`) |
| Atelier binaire + frontend + runner | `/opt/atelier/{bin/atelier, web/dist, runner, crates/atelier-logging-shipper}` (Medion) — `web/dist/` contient la homepage **et** le sous-build Studio `web/dist/studio/` (servi sous `/studio/{slug}`) |
| Atelier env | `/opt/atelier/.env` (Medion) |
| Docs (source contenu) | `/var/lib/atelier/docs/` (l'index de recherche est en Postgres `doc_entries`) |
| Postgres | Medion 127.0.0.1:5432 (local depuis Atelier) |
| dataverse-secrets.json | `/var/lib/atelier/state/dataverse-secrets.json` (Medion) |
| Git bare repos | `/var/lib/atelier/git/` (Medion) |
| Files data ZFS (hors deploy) | `/ssd_pool/homecloud/data/{files,thumbnails,downloads,films}` — pool zfs Medion, géré hors Atelier, **non** synchronisé par `make deploy` (vestige de l'app files décommissionnée) |
| Sources Atelier (code) | `/home/romain/atelier` (Medion — build en place) |
| Backup restic (off-site) | SMB `files.mynetwk.biz:files/atelier-backup` via rclone |

## Ports & sockets

| Port/socket | Hôte | Service |
|---|---|---|
| 4100 | Medion (0.0.0.0) | Atelier HTTP API + frontend (homepage `/` + Studio `/studio/{slug}`) + `/mcp` + `/apps/{slug}/` proxy |
| /run/atelier.sock | Medion | Atelier IPC |
| 3005-3010 | Medion (0.0.0.0) | Apps : www:3005, home:3007, trader:3008, wallet:3009, myfrigo:3010 (3006 libre) — atteintes en pratique uniquement via le path-proxy |
| 8081 | Medion (127.0.0.1) | code-server@romain (édition source Atelier) — **à la demande, normalement arrêté/disabled** |

> Port 4001 = référence **legacy hr-orchestrator** uniquement ; aucun serveur n'écoute dessus. Le MCP d'Atelier est à `http://127.0.0.1:4100/mcp` (Bearer `MCP_TOKEN`, scope par app via `?project={slug}` ; `?scope=surveillance` restreint à une whitelist read-only pour le scan-agent de surveillance).

## Variables d'environnement Atelier (Medion `/opt/atelier/.env`)

```
# Réellement présentes dans /opt/atelier/.env :
ATELIER_DV_ADMIN_URL=postgres://dataverse_admin:...@127.0.0.1:5432/postgres
ATELIER_DV_HOST=127.0.0.1
ATELIER_APPS_RUNTIME_ROOT=/var/lib/atelier/apps
ATELIER_APPS_SRC_ROOT=/var/lib/atelier/apps
ATELIER_GIT_REPOS_DIR=/var/lib/atelier/git
ATELIER_BUILD_AS_USER=...                # user de build des apps
ATELIER_LOGS_TOKEN=...                    # auth ingestion logs (shipper) + injecté aux apps (tier platform)
MCP_TOKEN=...                             # auth MCP (jamais loggé) — injecté au scan-agent via stdin
ATELIER_ENV_RECONCILE_APPLY=1            # boot-sweep écrit les .env (sinon dry-run/log only)

# Surveillance — scan-agent = Claude Agent SDK (défauts en code) :
# ATELIER_SCAN_MODEL                       unset → défaut abonnement (Opus)
# ATELIER_SCAN_EFFORT=max                  défaut max ; "none" pour omettre (Haiku)
# ATELIER_SCAN_TIMEOUT_SECS=600            timeout par run
# ATELIER_SCAN_MAX_CONCURRENT=3            ratelimit guard (3 = les 3 scans d'une app en parallèle pendant le sweep)
# ATELIER_SCAN_RUNNER=/opt/atelier/runner/src/scan.js   (réutilise ATELIER_AGENT_{NODE_BIN,USER,CLAUDE_CONFIG_DIR})

# Surchargeables (défauts en code, NON listées dans .env aujourd'hui) :
# ATELIER_PRESERVE_PREFIX_SLUGS=www        slugs no-strip du path-proxy (défaut www)
# ATELIER_AGENT_DRAIN_SECS=45              budget de drain agent au shutdown
# ATELIER_DV_TOKEN_MAX_AGE_SECS            rotation HR_DV_TOKEN (défaut 90j)
# ATELIER_APP_UNIT_PREFIX=atelier-app, ATELIER_APP_SLICE=atelier-apps.slice
# Backup : ATELIER_RESTIC_BIN, ATELIER_RCLONE_BIN, ATELIER_PG_DUMPALL_BIN, ATELIER_BACKUP_PG_USER, ATELIER_BACKUP_ENV_FILE
```

---

## Routing des apps (path-proxy)

Les apps sont servies en **même-origine** sous `http://127.0.0.1:4100/apps/{slug}/` (`routes/apps_proxy.rs`, monté top-level). Deux modes :

- **strip** (Vite/Axum) — le préfixe `/apps/{slug}` est retiré avant de proxifier vers l'app.
- **no-strip** (Next.js, ex. `www`) — le préfixe `/apps/www` est **préservé** jusqu'à l'app (requis par `basePath`/`assetPrefix`, configuré dans `next.config.ts`). Slugs no-strip listés via `ATELIER_PRESERVE_PREFIX_SLUGS` (défaut `www`).

Le path-routing **interne** est donc complet et live. Ce qui reste pendant : l'intégration **côté hr-edge** (hostname public + path + auth path-aware), cf. [.claude/rules/path-routing-pending.md](.claude/rules/path-routing-pending.md). Pour atteindre une app : `/apps/{slug}/` en relatif même-origine (gallery Studio, PreviewTab) ; e2e externe via `https://atelier.mynetwk.biz/...` (302 anonyme = sain) ; sans auth en local via `127.0.0.1:4100/apps/{slug}/`.

> Le supervisor **adopte les unités systemd orphelines** au démarrage (commit `b6cc47f`) même si l'état persisté a divergé.

## Studio — agent Claude natif

Le Studio inclut une **UI custom d'édition** (`AgentWorkspace` : explorateur de fichiers, diffs, commits, panneau git — cf. [Frontend](#frontend--control-panel-web-react--vite)) ET un **agent Claude** (chat, raisonnement, plan, approbation interactive). L'agent est un shim Node (`runner/src/runner.js`) piloté par `routes/agent.rs` et le **Claude Agent SDK** (binaire natif linux-x64). _(L'éditeur code-server `atelier-studio.service`/:8443 a été décommissionné le 2026-06-17.)_

> Depuis le 2026-06-21, cette UI Studio est une **app Vite séparée** (entrée `studio.html`, base `/studio/`) servie sous `/studio/{slug}`, ouverte en **onglet navigateur dédié** par app (cf. [Frontend](#frontend--control-panel-web-react--vite)) — elle n'est plus montée inline dans la homepage. Le backend agent (`routes/agent.rs`, runner) est inchangé.

- **Runner** : `/opt/atelier/runner/src/runner.js`, exécuté **en `hr-studio`** via `sudo -n -u hr-studio node runner.js` (process group propre pour reaper le binaire `claude` petit-fils). Reçoit son init JSON sur stdin (dont `MCP_TOKEN` — jamais en env/argv, anti-leak journalctl), émet du NDJSON sur stdout.
- **Auth** : abonnement OAuth via `/var/lib/hr-studio/.claude/.credentials.json`, **PAS** de clé API (le runner échoue si `ANTHROPIC_API_KEY` est présent).
- **Sessions** : persistées incrémentalement par le SDK à `/var/lib/hr-studio/.claude/sessions/{scope}/` (scope = `cwd` par workspace d'app), reprises via `sessionId`.
  - ⚠️ **Gotcha** : `ProtectSystem=strict` + `ReadWritePaths=/var/lib/hr-studio` (dans `atelier.service`) est **critique** — sans lui, EROFS empêche le flush des sessions (non-resumables, transcripts tronqués). Le namespace mount est hérité par les descendants `sudo→node→claude`.
- **Plan-mode gate** (étanche aux mutations MCP) : SDK natif en read-only (`permissionMode:plan`) + allowlist `MCP_READONLY` + interception **bloquante** de `AskUserQuestion`/`ExitPlanMode` via `canUseTool`. `settingSources=['project']` charge **CLAUDE.md + .claude/rules/ + skills** du workspace, et **exclut** les sources user/local (les settings de `hr-studio` contiennent un auto-approve `mcp__studio__*` qui casserait le gate). Le `settings.json` projet n'a **aucun bloc `permissions`** (court-circuiterait `canUseTool`).
- **Dialogues interactifs** : `AskUserQuestion` émet un event `question` et suspend sur Promise jusqu'à réponse sur stdin ; `ExitPlanMode` idem pour l'approbation de plan (transition vers `acceptEdits`/bypass tout en conservant le blocage des questions).
- **Streaming** : EventBus channel `agent` (buffer 2048) diffuse en WebSocket les events NDJSON (`started`, `system`, `assistant_delta`, `thinking_delta`, `tool_use`, `tool_result`, `question`, `plan_review`, `result`, `turn_done`, `done`, `error`). Le front route par `session_id` (fallback `run_id` avant l'event `system`).
- **Introspection** : opérations one-shot `op:list/messages/rename/delete/tag` (timeout 30s) pour la gestion des conversations.
- **Drain au shutdown** : interrupt + EOF à tous les runs live, attente `ATELIER_AGENT_DRAIN_SECS` (45s), puis SIGKILL. `KillMode=mixed` + `TimeoutStopSec=60s` laissent le budget de drain (sinon SIGKILL simultané de `sudo→node→claude` tronque le tour → session non-relançable).

**API** : `POST /api/apps/{slug}/agent/{query,message,answer,plan_decision,set_mode,set_model,interrupt,cancel}` + CRUD conversations. **Debug** : runner à `/opt/atelier/runner/src/runner.js`, binaire natif `runner/node_modules/@anthropic-ai/claude-agent-sdk-linux-x64`, `journalctl -u atelier` (stderr runner), croissance des fichiers de session.

> ⚠️ Ne pas confondre cet **agent Studio (Claude)** interactif (multi-tour, mutations possibles) avec la **surveillance IA** ci-dessous (scan headless **lecture seule**, single-turn) ni avec `hr-orchestrator` (déploiements network). Les deux tournent sur le **même Claude Agent SDK** mais via deux runners distincts (`runner.js` vs `scan.js`).

## Surveillance IA (3 scans/app, Claude Agent SDK)

> **Moteur unique = Claude Agent SDK** (2026-06-25 : driver Codex **entièrement retiré** — plus de `codex.rs`, plus d'`ATELIER_SCAN_DRIVER`/`ATELIER_CODEX_*`, plus de génération d'`AGENTS.md`). Findings via le tool MCP `findings_upsert`, tables/events WebSocket inchangés.

Crate `atelier-watcher` : 3 scans par app, écrivant des **findings catégorisées** via MCP. Findings + runs + mémoire + config en Postgres `atelier_meta`. UI : page `/surveillance` (overview global + **vue live du sweep**) + tab Surveillance per-app. Live via WebSocket (`surveillance:event`, `surveillance:transcript`, `surveillance:sweep`). Modèle hybride **3 scans** (déployé 2026-06-05, commit `6dec754`) ; le champ `kind` discrimine partout :

- **`security`** + **`code_review`** (« Qualité ») = scans **plateforme FIXES** (prompts en code `crates/atelier-watcher/src/prompts/{security,code_review}.md`), gate diff git, tournent pour toutes les apps, **non éditables par l'agent**.
- **`business`** = **seul scan possédé par l'agent**, défini en **données** (table `app_scan` : label/prompt/cadence/gate/gate_sql/categories) via MCP `scan_get`/`scan_set`, **vide par défaut** (run `skipped("blank")`). `gate_sql` SELECT-only, fourni par l'agent et adapté au schéma de SON app (jamais hardcodé plateforme).

**Triage des findings (tous les scans)** : chaque prompt instruit l'agent à **lire d'abord** les findings ouvertes (`findings_list`), puis pour chacune : garder / mettre à jour (`findings_upsert` même fingerprint) / **supprimer** (`findings_delete`, **hard delete** quand la cause a disparu — fichier/fonction supprimé, refactoré, faux positif que le code ne déclenche plus). `findings_resolve` = fix committé ; `findings_dismiss` = faux positif à mémoriser ; `findings_delete` = obsolète (scopé `(slug,kind)` pour l'ownership).

Gates : (1) plafond `MAX_OPEN_FINDINGS = 6` **par (app,kind)** ; (2) diff-aware. **Sweep automatique** (`start_sweep`) : passe app par app, lance les 3 scans **simultanément** (`ATELIER_SCAN_MAX_CONCURRENT=3`), **force** tous les scans (bypass des gates fraîcheur+plafond → le triage/purge tourne partout), single-flight, annulable. Déclenché **manuellement** (`POST /api/surveillance/sweep`) **ou planifié** (scheduler interne `sweep_scheduler` + config singleton `sweep_schedule`, calqué sur le scheduler backup). `git_watcher` (auto-resolve via commit `fix(surveillance:N)`) tourne toujours. Kill = SIGKILL du **groupe de process**. Findings : liste = titre + `summary` ; `plan` = doc de résolution complet (tiroir latéral).

**Progression live (étapes)** : la console live affiche par défaut une **liste d'étapes** (composant `web/src/components/surveillance/ScanStepsView.jsx`), pas le transcript brut (toggle « brut » dans `LiveScanPanel`). Mécanique : l'agent appelle le tool MCP **`scan_progress(step, total, label)`** au début de chaque phase (pur *signpost*, aucun effet de bord — son event `tool_use` circule dans le flux `surveillance:transcript` existant). L'instruction est injectée dans **`build_prompt`** (runner.rs, `PROGRESS_PREAMBLE`) → couvre les 3 kinds **y compris le scan business** (prompt agent-authored). Le front (`buildScanSteps` dans `scanFormat.js`) découpe le transcript aux marqueurs et dérive les métriques par étape (lectures/outils/findings/durée) ; tokens totaux au pied via l'event `result`. `TranscriptLine.ts` (unix ms) permet de calculer les durées même au replay du buffer.

> **Driver Claude** : `crates/atelier-watcher/src/claude.rs` spawn `runner/src/scan.js` en `hr-studio` (`sudo -n -H -u hr-studio … node scan.js`, OAuth abonnement via `/var/lib/hr-studio/.claude`, **jamais de clé API** — même chemin pour les runs manuels ET le sweep planifié). **Lecture seule = 3 couches** : (1) MCP `?scope=surveillance` (whitelist serveur autoritaire — inclut `findings_list`/`findings_delete`), (2) `canUseTool` de scan.js (n'autorise que Read/Glob/Grep + tools MCP), (3) tourne en `hr-studio` (jamais root). `permissionMode:'default'` (PAS `'plan'` qui n'exécute aucun outil) + **`disallowedTools`** (retire Bash/Write/Edit/Task/Skill/… du contexte — garantie dure ; un simple `canUseTool` ne suffit PAS : en `'default'` le SDK ne le consulte pas pour les builtins). Non-pollution du Studio : `op:delete` post-run (`claude.rs::cleanup_session`) — `persistSession:false` est **ignoré** par le binaire natif 0.3.167. Tokens lus depuis `result.usage`. Config via `ATELIER_SCAN_{MODEL,EFFORT,TIMEOUT_SECS,MAX_CONCURRENT}` + `ATELIER_SCAN_RUNNER` (réutilise les chemins `ATELIER_AGENT_*`).

## Remontées plateforme des apps (CLAUDE_ISSUES)

Boucle de feedback **app → Atelier** : quand un chat Claude Code d'app (Studio) bute sur une friction **plateforme** (tool MCP, doc, build/deploy, dataverse, agent) — et **non** un bug interne de l'app — il la remonte au lieu de contourner en silence. Romain consomme ensuite ces remontées en session dev Atelier.

- **Store CENTRAL (2026-06-26)** : les remontées vivent dans le **control-plane Postgres** `atelier_meta.platform_issues` (store [crates/atelier-common/src/issue_store.rs](crates/atelier-common/src/issue_store.rs)), **plus** dans l'arbre source des apps. WHY : la feature concerne des bugs de la **plateforme**, pas des apps — le fichier per-app `{slug}/src/CLAUDE_ISSUES.json` n'avait aucun sens (il polluait le dépôt de l'app). L'ancien fichier a été **rapatrié une fois puis supprimé** au boot (`backfill_from_files`, idempotent).
- **Endpoints** ([crates/atelier-api/src/routes/issues.rs](crates/atelier-api/src/routes/issues.rs), non authentifiés comme `build-event`/`ship`) : **report** `POST /api/apps/{slug}/issues` (slug dans l'URL → colonne `slug` ; le serveur estampe `id`/`ts`/`status:open`) ; **triage dev platform-level** (l'`id` est global, plus de slug) : `GET /api/issues?status=&app=` (liste agrégée, tri serveur) · `PATCH /api/issues/{id}` (`status`/`note`) · `DELETE /api/issues/{id}`. Hook **AppDelete** : purge des issues de l'app (`delete_by_slug`). No-op/vide si Postgres down (mirror `TaskStore`).
- **Surface agent (2026-07-03)** : règle `.claude/rules/report-issues.md` (QUAND/quand NE PAS) + skill `0-report-issue` (SKILL.md seul) qui pointe vers le **tool MCP `issue_report`** (scope projet ET whitelist surveillance — le scan-agent peut aussi remonter). L'ancien `report-issue.sh` (curl+jq) est **supprimé** et purgé des workspaces par la régénération de contexte ; l'endpoint `POST …/issues` reste (surface HTTP du même store). Régénérer le contexte des apps après tout changement (MCP `app.regenerate_context` — ou attendre le boot regen).
- **Côté dev Atelier** : skill `/collect-issues` ([.claude/skills/collect-issues/](.claude/skills/collect-issues/)) interroge `GET /api/issues` (store central), triées par sévérité, pour triage + clôture via `PATCH`/`DELETE /api/issues/{id}`.

> Distinct de la **surveillance IA** (findings générés par scan headless sur l'app) : ici c'est l'agent **interactif** qui signale un souci **de la plateforme**, pas de l'app.

## Notifications plateforme + « pattes » agents (2026-07-03)

Canal **agent → utilisateur** : les agents SDK sont des citoyens de la plateforme, capables de la percevoir, d'agir dessus ET de parler à Romain. Store `atelier_meta.platform_notifications` ([crates/atelier-common/src/notification_store.rs](crates/atelier-common/src/notification_store.rs), no-op si PG down) ; le store **porte le sender** du canal EventBus `notify` (insert + publish indissociables, publish après insert OK) relayé en WS **`notify:event`** (`action: created|read|read_all|deleted` — les mutations sont broadcastées pour la cohérence multi-onglets).

- **`notify_user(title, body?, level?)`** — notification volontaire (kind=`notice`), réservée à ce qui mérite l'attention de Romain (décision, anomalie, fin de tâche longue). Autorisé en **plan-mode** (écrit dans la méta-DB seulement) et en **scope surveillance** (slug explicite).
- **Journal automatique d'actions** (kind=`action`, **né lu** : n'alimente jamais badge/notif) : au dispatch MCP ([routes/mcp.rs](crates/atelier-api/src/routes/mcp.rs) `journal_agent_action`), toute mutation **réussie** d'un agent projet dans la liste curated (start/stop/restart, ship, app.build, env_set/env_delete — clé seule, jamais la valeur —, schema-ops `db_*`, scan_set, app.update) est tracée « Agent {slug} : {action} ». Pas de `dv_*` (trail `_dv_audit`) ni docs/findings. Rétention 30 j (`prune_old_actions` au boot).
- **Nouveaux tools MCP scope projet** : `ship(timeout_secs?)` (réutilise `AppsContext::ship`, même lock BUILD_BUSY + canal badge build ; inspecte `exit_code` — le pipeline renvoie ok_data même en échec), `env_list` (secrets TOUJOURS masqués, aussi en plan-mode), `env_set(key,value,secret?,scope?)`/`env_delete(key)` (wrappent env_ops + reconcile ; `app.update env_vars` = deprecated), `issue_report` (cf. section CLAUDE_ISSUES). Whitelists : 4 points d'enregistrement dans mcp.rs (defs projet + `is_project_simplified_tool` + `is_dispatched_project_tool` + bras du match — **écrire le match d'abord**, les tests de parité ne le couvrent pas) ; `MCP_READONLY` (runner.js, plan-mode) += `notify_user`, `env_list`.
- **HTTP** ([routes/notifications.rs](crates/atelier-api/src/routes/notifications.rs), non authentifié LAN) : `GET /api/notifications?unread=&slug=&limit=` → `{items, unread}` · `POST /{id}/read` · `POST /read-all` · `DELETE /{id}`. Hook AppDelete purge par slug.
- **UI** : cloche `Inbox` + tiroir dans les DEUX builds (header homepage [Layout.jsx](web/src/components/Layout.jsx) + topBar Studio, filtre « cette app ») — [web/src/context/NotificationsContext.jsx](web/src/context/NotificationsContext.jsx) (fetch initial + WS + re-sync epoch/resync, mutations optimistes idempotentes vs écho WS). Notif système PWA si onglet caché et (notice OU warn/error) via `showPlatformNotification` ([web/src/lib/notify.js](web/src/lib/notify.js), ex-`agentNotify.js`) ; badge PWA **agrégé par tranches** (`setBadgeSlice('agent'|'notify', n)`). `sw.js` : `notificationclick` généralisé (`data.target`).
- **Boot context regen** : `AppsContext::regenerate_all_contexts()` appelé au boot (main.rs, après l'env-reconcile sweep) — le contexte généré suit le binaire, idempotent (`write_if_changed`), CLAUDE.md agent-owned protégé. `studio.refresh_all` réutilise la même méthode (fix du root régénéré N×) ; l'ancien `ContextGenerator::refresh_all` (mort, perdait `db_tables`) est supprimé.
- **Doctrine agents** (rules générées workflow.md/mcp-tools.md) : les actions plateforme sont auto-journalisées — `notify_user` JAMAIS pour « j'ai redémarré l'app » ; livraison fin de feature = `0-build` puis tool `ship`.

## Backup (restic + rclone)

Crate `atelier-backup` + `routes/backup.rs`. Backups incrémentaux, dédupliqués, chiffrés via **restic** → SMB `files.mynetwk.biz:files/atelier-backup` via le backend **rclone** de restic. Config + état en Postgres `atelier_meta` (`backup_target` singleton, `backup_runs`, `backup_run_snapshots` — 3 tags/run).

- **3 sources/run** : GIT (`/var/lib/atelier/git/`), PostgreSQL (`pg_dumpall` en `runuser -u postgres`), CONFIG (.env, registres, secrets, docs, `.env` per-app).
- **Scheduler** : boucle Tokio (tick périodique), quotidien ~03:00 (min-age ≈ 20h ; hebdo ≈ 6.5j). Single-flight (409 si déjà en cours). Progress par phases via WebSocket (`backup:live`).
- **Rétention** : `restic forget --group-by host,tags --keep-last <keep> --prune` (défaut keep=7).
- **Secrets** : mot de passe restic auto-généré au 1er run, stocké en Postgres (révélable via `GET /api/backup/repo/password` pour disaster-recovery) ; credentials SMB obscurcis via `rclone obscure` ; transmis aux child-process **par env vars uniquement**.
- **Noop mode** : 503 sur les endpoints si `ATELIER_DV_ADMIN_URL` absent / Postgres injoignable / binaires `restic`/`rclone` manquants.
- **Système backup-only** : aucune restauration automatisée (restauration manuelle = `restic restore` + credentials).

API : `PUT /api/backup/target`, `POST /api/backup/discover`, `GET /api/backup/runs`, `GET /api/backup/repo/password`. État au 2026-06-15 : 11 runs / 33 snapshots (3 tags/run).

## Frontend / control-panel (web/, React + Vite)

> **Deux builds Vite séparés, une seule API (2026-06-21).** Le frontend est scindé en **deux apps Vite distinctes** partageant `web/src/` :
> - **Homepage / panneau de contrôle** — entrée `index.html` (base `/`, → `web/dist/`), servie à `http://127.0.0.1:4100/`. Galerie d'apps (landing), DbExplorer, schema, git, surveillance, backup, tasks. **Ne contient plus le Studio** → bundle nettement plus léger (l'agent, `mermaid`, `katex`, `cytoscape`, `xterm` ne sont QUE dans le Studio).
> - **Studio (per-app)** — entrée `studio.html` (base `/studio/`, sortie `web/dist/studio/studio.html`), servie sous `http://127.0.0.1:4100/studio/{slug}` (nest Axum `nest_service("/studio", ServeDir(dist/studio).fallback(studio.html))` monté **avant** le fallback homepage, cf. [crates/atelier-api/src/lib.rs](crates/atelier-api/src/lib.rs)). Éditeur focalisé sur UNE app (slug dans l'URL) : barre supérieure propre + onglets + agent.
>
> **Ouverture** : depuis la homepage (galerie, sous-menu Sidebar, deep-links surveillance) on ouvre le Studio d'une app dans un **nouvel onglet navigateur focalisé** via `web/src/lib/openStudio.js` (`window.open('/studio/{slug}?tab=…', 'atelier-studio-{slug}')` — `target` nommé → reclic = refocus de l'onglet existant). Le deep-link (`tab`/`kind`) passe par l'URL (un `window.open` ne transporte pas le `state` du router). _(L'ancien Studio inline dans la homepage + la sync cross-PC `studio_state` de l'« app ouverte » ont été retirés ; la sync per-app `agent_open_tabs` est conservée.)_

SPA React + Vite. Panneau de contrôle unifié : galerie des 5 apps (landing), DbExplorer, git history, surveillance, backup, tasks ; le Studio (édition + agent) s'ouvre en **onglet séparé** (`/studio/{slug}`).

- **WebSocket = la convention temps réel** : tous les updates live passent par `/api/ws` (broadcast Axum, `routes/ws.rs`), **jamais de polling front**. Channels : état app, builds, logs, tasks, `surveillance:event`/`transcript`, `backup:live`, `agent`, `agent:open-tabs`. Hook `useWebSocket` (auto-reconnect).
- **Studio (app `/studio/{slug}`)** : barre supérieure propre (statut/contrôles app + lien `/apps/{slug}/` + retour « ← Atelier ») ; tabs (code/preview/db/logs/docs/surveillance/env/settings — l'onglet **Code** rend l'`AgentWorkspace`) ou mode split (`AgentWorkspace` à gauche, tabs à droite). **PreviewTab** = mini-navigateur iframe vers `/apps/{slug}/` (barre d'adresse relative).
- **AgentPanel** + `AgentConversationsContext` : multi-sessions, streaming via le channel `agent`, rendu par type de tool (Read/Write/Bash/Edit/MCP), `ConversationsSplit` (max 3 côte-à-côte).
- **DbExplorer** : CRUD tables/colonnes via endpoints typés (`/apps/{slug}/db/...`), pas de SQL brut.
- **Surveillance** : overview global + détail per-app (3 kinds), console live (`surveillance:transcript`).
- **Git** : heatmap de commits, stats per-commit, diff viewer.
- **Thème** dark/light (pré-paint + localStorage), **PWA** installable (manifest + maskable icons + service worker).

> ⚠️ Le service worker est cache-first : **vérifier visuellement** après tout deploy frontend (le SW peut masquer le résultat).

---

## Build & deploy

### Atelier lui-même (build en place sur Medion)

Source à `/home/romain/atelier`. Build, install, restart **en local** (plus de cross-build/rsync distant).

```bash
make help              # tous les targets
make atelier           # cargo build --release -p atelier
make web               # npm ci (si besoin) + 2 builds Vite : homepage (web/dist) PUIS Studio (web/dist/studio)
make runner            # npm ci --omit=dev du runner + vérifie runner.js + binaire SDK natif
make deploy            # build atelier+web+runner + install /opt/atelier + restart + healthcheck
make logs              # tail journalctl atelier (local)
```

`make deploy` détecte l'hôte : sur Medion → `deploy-local` (build + `sudo install` atomique `.new`+rename + restart + healthcheck `/api/health`) ; ailleurs → fallback `deploy-remote` (build local + rsync/SSH vers `$MEDION`). Le deploy synchronise aussi :

- `web/dist/` → `/opt/atelier/web/dist/` (inclut le sous-build `web/dist/studio/` du Studio — un seul rsync, un seul arbre dist ; `make web` build la homepage **puis** le Studio, ordre impératif car le build homepage vide `dist/`).
- le crate **source** `atelier-logging-shipper` → `/opt/atelier/crates/atelier-logging-shipper/` (path-dep absolu consommé par les apps qui shippent leurs logs ; modifier le shipper impose de rebuild ces apps).
- le **runner** Node → `/opt/atelier/runner/{src,node_modules,package*.json,.npmrc}`. ⚠️ `npm ci` du runner se fait en `--omit=dev` mais **JAMAIS `--omit=optional`** : le binaire natif `@anthropic-ai/claude-agent-sdk-linux-x64` est une optional-dep, sans lui le runner échoue au runtime (le Makefile garde-fou teste sa présence avant deploy).

### Apps HomeRoute

Sources des 5 apps sur Medion (`/var/lib/atelier/apps/<slug>/src/`), éditées via le Studio (UI custom + agent, `https://atelier.mynetwk.biz/` ou `http://127.0.0.1:4100/`). Source = runtime.

```bash
make deploy-app SLUG=home   # build sur Medion + restart via API + healthcheck path-proxy
```

[scripts/deploy-app.sh](scripts/deploy-app.sh) : lit `build_command`/`stack`/`port`/`health_path` depuis l'API → build in-place (`hostname == medion`) → `POST /api/apps/<slug>/control action=restart` → healthcheck via le **path-proxy local** `http://127.0.0.1:4100/apps/<slug><health_path>` (commit `bf1e3a8`, 2026-06-13 ; les hostnames `{slug}.mynetwk.biz` sont morts). Un `3xx` est accepté (les apps `auth_required` redirigent les anonymes vers `/login`).

### Boot ordering & indisponibilité

`atelier.service` a `After=postgresql.service` (**pas** `Requires=`) : un échec Postgres **au boot** fait échouer volontairement le démarrage (control-plane critique), mais une perte Postgres **à chaud** est dégradée gracieusement (écritures dégradées, noop backup, 503 dataverse). `make deploy` : ~5 s d'API down au restart (les apps continuent). `make deploy-app` : 1-3 s d'indispo de l'app concernée. Rollback : `git checkout <commit>` puis `make deploy` (binaire/frontend précédents restent jusqu'au prochain deploy ; historique poussé sur `origin`).

## Règles obligatoires

- **JAMAIS** `cargo run` directement — utiliser `make deploy` (install dans `/opt/atelier`).
- **TOUJOURS** `make deploy` après modification du code Atelier (build en place + install + restart + healthcheck).
- **TOUJOURS** `make deploy-app SLUG=<x>` après modification d'une app (build Medion + restart via API).
- **TOUJOURS** vérifier visuellement après deploy frontend (SW cache-first peut masquer le résultat).
- **TOUJOURS** vérifier le healthcheck dans la sortie du `make deploy*` avant de considérer un deploy réussi.
- **TOUJOURS** tester e2e les endpoints créés/modifiés (cf. `.claude/rules/testing.md`).
- **TOUJOURS** logger structuré via `tracing` (cf. `.claude/rules/logging.md`).
- **TOUJOURS déployer librement** (`make deploy` / `make deploy-app`) sans demander d'autorisation, MAIS **DEMANDER avant de committer** (`git commit`) : proposer le commit en fin de travail cohérent, laisser l'utilisateur décider.
- **JAMAIS** d'attribution Claude dans les commits.

## Crates (workspace `atelier-*`)

Atelier est **autonome** : toutes ses crates vivent sous `crates/` (renommées depuis `hr-*` le 2026-05-30), plus aucun path-dep vers `/nvme/homeroute/`.

| Crate | Rôle |
|---|---|
| `atelier` | binaire principal (entrypoint, bootstrap, backfill control-plane) |
| `atelier-api` | serveur HTTP Axum (routes, WebSocket, MCP, path-proxy, agent) |
| `atelier-apps` | supervisor des apps (lifecycle systemd, ports, adoption d'unités) |
| `atelier-dataverse` | moteur Postgres + passerelle gateway-only + audit |
| `atelier-dvexpr` | dialecte d'expressions de filtre dataverse |
| `atelier-dv-codegen` | génération de code/contexte depuis le schéma dataverse |
| `atelier-docs` | système de docs per-app (index Postgres `doc_entries`) |
| `atelier-git` | bare repos + introspection (log/diff/activity) |
| `atelier-watcher` | surveillance IA (Claude Agent SDK, 3 scans, sweep auto + scheduler, git_watcher) |
| `atelier-backup` | backup restic + rclone + scheduler |
| `atelier-logging` | pipeline de logs structurés (ingest, buffer, broadcast → `atelier_logs`) |
| `atelier-common` | types/utilitaires partagés + bootstrap pool control-plane (`atelier_meta`) |
| `atelier-ipc` | IPC socket Unix (`/run/atelier.sock`) |
| `atelier-logging-shipper` | crate **hors workspace** (`exclude`), path-dep absolu des apps pour shipper leurs logs vers Atelier — déployée en **source** dans `/opt/atelier/crates/` |

Crates supprimées : 4 `hr-flow*` (2026-05-26), `atelier-db` (SQLite legacy) + `atelier-dataverse-migrate` (migration one-shot) (2026-05-30).

## Service naming + autonomie

Atelier est **autonome** : préfixe `atelier-app-`, ne partage ni nom ni path avec hr-orchestrator (qui tourne toujours pour la partie network/registry).

| Concept | hr-orchestrator (Medion) | Atelier (Medion) |
|---|---|---|
| Service principal | `hr-orchestrator.service` | `atelier.service` |
| Apps spawn | (legacy, désactivé) | `atelier-app-{slug}.service` |
| Slice | `hr-apps.slice` (legacy) | `atelier-apps.slice` |
| Apps runtime root | `/opt/homeroute/apps/` (legacy) | `/var/lib/atelier/apps/` |
| Control-plane | `/opt/homeroute/data/apps.json` | Postgres `atelier_meta` |

Override possible via `ATELIER_APP_UNIT_PREFIX` / `ATELIER_APP_SLICE` / `ATELIER_APPS_RUNTIME_ROOT`.

## Workflow d'agent

À chaque fois que tu travailles dans Atelier :

1. Lire `MEMORY.md` global (auto-chargé) et les rules dans `.claude/rules/`.
2. Si la tâche concerne une app HomeRoute existante (`/var/lib/atelier/apps/{slug}/src/`, éditée via Studio), suivre la doctrine **DOC-FIRST** : `mcp__studio__docs_overview` d'abord. L'agent Studio ne charge que le workspace (`settingSources=['project']`) — garder CLAUDE.md + `.claude/rules/` à jour pour son raisonnement.
3. Pour toute fonctionnalité ajoutée à Atelier : doc/commentaires WHY + tests e2e + logging structuré.
4. **Pour toute action runtime** (statut, logs, restart) : passer par l'API Atelier. En local sur Medion : `sudo journalctl -u atelier...` ou `http://127.0.0.1:4100/api/...` ; en externe : `https://atelier.mynetwk.biz/api/...` (⚠️ **pas** `app.mynetwk.biz` — plus de route edge).
