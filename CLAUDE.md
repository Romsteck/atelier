# Atelier — Plateforme applicative HomeRoute

## Statut migration (2026-05-09)

> ✅ **Atelier rapatrié sur Medion** — supervisor + 6 apps running (`atelier-app-*.service`) tournent désormais sur Medion. CloudMaster ne fait plus que servir `code-server` (Studio) pour l'édition des sources.
>
> Précédemment : Phase 9 cutover Medion→CloudMaster + 9.5 slim (10 crates internalisés `crates/hr-XXX`). Voir [memory/project_atelier_cutover_done.md](/home/romain/.claude/projects/-nvme-homeroute/memory/project_atelier_cutover_done.md).
>
> Détail rapatriement : [docs/plan-rapatriement.md](docs/plan-rapatriement.md) (plan local, source `/home/romain/.claude/plans/le-but-de-la-encapsulated-moler.md`).

### Restant à faire (différé)

- **9.2 finition write endpoints Atelier** — list/get/env/control/status implémentés. Manquent : create/update/delete/build/deploy/exec/update_env/regenerate_context/logs/todos. Boutons Studio/DbExplorer correspondants → 404 silencieux. **Symptôme observé** : Files /api/files/upload retourne `dv: gateway error 405`, et le scheduler `home` log toutes les 10s `failed to log command_history: gateway error 405`.
- **Refacto `homeroute-core`** — découpler `hr-common`/`hr-ipc`/`hr-docs` qu'Atelier path-dep encore vers `/nvme/homeroute/crates/shared/`.
- **Path-routing `app.mynetwk.biz/apps/{slug}`** — but initial de la séparation Studio. Reporté ; cf. [.claude/rules/path-routing-pending.md](.claude/rules/path-routing-pending.md).

## Plan suivant — `hr-flowd` daemon multi-stack

📌 **Plan complet dans le repo** : [docs/plan-hr-flowd.md](docs/plan-hr-flowd.md) (copie locale, source originale `/home/romain/.claude/plans/peaceful-spinning-mountain.md`)

Transformer `hr-flow` (lib Rust embeddable, donc inutilisable côté NextJS) en daemon partagé `hr-flowd` accessible via callbacks HTTP par toutes les apps quelle que soit leur stack. **Explicitement reporté** par l'utilisateur ; à reprendre quand le rapatriement sera stable.

⚠️ **Pendant les évolutions de `hr-flow` ici** : ne pas refactorer de façon qui rendrait l'extraction du daemon plus difficile. Pas de nouveau couplage fort à `ApiState` ou au runtime des apps.

---

## Quoi est Atelier

Plateforme applicative autonome (post-rapatriement 2026-05-09). Contient :

- **Apps** : supervisor Tokio des apps locales (lifecycle, ports, logs) — services nommés `atelier-app-{slug}.service` (slice `atelier-apps.slice`).
- **Dataverse** : moteur Postgres avec schéma dynamique, GraphQL, dvexpr.
- **Flows** : moteur d'orchestration TOML.
- **Docs** : système de documentation per-app (FTS5).
- **Git** : bare repos.
- **Store** : catalogue Flutter.
- **MCP** : exposition des tools `app.*`, `db.*`, `docs.*`, `flow.*`.

Atelier ne contient **pas** : DNS, DHCP, reverse proxy, ACME (ces concerns restent dans `hr-edge` + `hr-netcore` sur Medion). `code-server` (Studio) reste sur CloudMaster.

## Architecture (post-rapatriement, 2026-05-09)

```
Internet → Cloudflare → Medion (10.0.0.254)
                          ├─ hr-edge (proxy + ACME + auth + tunnel)
                          │   ├─ {slug}.mynetwk.biz → 127.0.0.1:port-app  (Medion loopback)
                          │   ├─ app.mynetwk.biz   → 127.0.0.1:4100      (Atelier)
                          │   ├─ studio.mynetwk.biz → 10.0.0.10:8443     (code-server, CloudMaster)
                          │   └─ proxy.mynetwk.biz  → 127.0.0.1:4000     (homeroute network API)
                          ├─ atelier.service (4100) — supervisor + apps API + frontend
                          ├─ atelier-app-{files,home,myfrigo,trader,wallet,www}.service
                          ├─ hr-edge.service / hr-orchestrator.service / homeroute.service
                          └─ Postgres-dataverse (5432)

CloudMaster (10.0.0.10)
  ├─ hr-studio.service (8443) — code-server (édition sources)
  └─ /opt/homeroute/apps/{slug}/src/ — sources canoniques (édition + build)
```

## Stockage

| Données | Chemin |
|---------|--------|
| Sources canoniques des apps | `/opt/homeroute/apps/{slug}/src/` (CloudMaster, édition via code-server) |
| Apps runtime (artefacts compilés) | `/var/lib/atelier/apps/{slug}/{src,bin,.env,db.sqlite,runs}` (Medion) |
| Atelier registry canonical | `/opt/atelier/data/{apps.json, port-registry.json}` (Medion) |
| Atelier binaire + frontend | `/opt/atelier/{bin/atelier,web/dist}` (Medion) |
| Atelier env | `/opt/atelier/.env` (Medion) |
| Docs FTS5 + index | `/var/lib/atelier/{docs, docs-index.sqlite}` (Medion) |
| Postgres-dataverse | Medion 127.0.0.1:5432 (local depuis Atelier) |
| dataverse-secrets.json | `/var/lib/atelier/state/dataverse-secrets.json` (Medion) |
| **Files data (raid0)** | `/ssd_pool/homecloud/data/{files,thumbnails,downloads,films}` (Medion zfs pool) |

## Ports & sockets

| Port/socket | Hôte | Service |
|---|---|---|
| 4100 | Medion | Atelier HTTP API |
| /run/atelier.sock | Medion | Atelier IPC |
| 3005-3010 | Medion (loopback) | Apps |
| 8443 | CloudMaster | code-server (hr-studio.service) |

## Variables d'environnement Atelier (Medion `/opt/atelier/.env`)

```
ATELIER_DV_ADMIN_URL=postgres://dataverse_admin:...@127.0.0.1:5432/postgres
ATELIER_APPS_RUNTIME_ROOT=/var/lib/atelier/apps
ATELIER_APPS_SRC_ROOT=/var/lib/atelier/apps
ATELIER_DV_HOST=127.0.0.1
# Defaults: ATELIER_APP_UNIT_PREFIX=atelier-app, ATELIER_APP_SLICE=atelier-apps.slice
```

## Build & deploy (depuis CloudMaster)

```bash
make help              # tous les targets
make atelier           # cargo build --release -p atelier (local)
make web               # build frontend (web/dist)
make deploy            # build all + push binary + frontend vers Medion + restart atelier.service
make deploy-app SLUG=x # build + rsync app x vers Medion + restart via API
make logs              # tail journalctl atelier sur Medion (via SSH)
```

Build et édition des sources : **CloudMaster**. Runtime + supervisor : **Medion**. Pas de rsync inverse — Medion ne renvoie rien à CloudMaster.

## Règles obligatoires

- **JAMAIS** `cargo run` directement — utiliser `make deploy`.
- **TOUJOURS** `make deploy` après modification du code Atelier (build CM → rsync Medion → restart).
- **TOUJOURS** `make deploy-app SLUG=<x>` après modification d'une app (build CM → rsync Medion → restart via API).
- **TOUJOURS** vérifier visuellement après deploy frontend (SW cache-first peut masquer le résultat).
- **TOUJOURS** tester e2e les endpoints créés/modifiés (cf. `.claude/rules/testing.md`).
- **TOUJOURS** logger structuré (cf. `.claude/rules/logging.md`).
- **JAMAIS** d'attribution Claude dans les commits.

## Path-deps vers homeroute (résiduel)

Atelier consomme encore ces crates partagées de homeroute :

```toml
hr-common = { path = "/nvme/homeroute/crates/shared/hr-common" }
hr-ipc    = { path = "/nvme/homeroute/crates/shared/hr-ipc" }
hr-docs   = { path = "/nvme/homeroute/crates/shared/hr-docs" }
```

Ne jamais modifier ces crates depuis Atelier — leur source de vérité reste dans `/nvme/homeroute/`. Refacto `homeroute-core` = travail futur.

Les **10 crates applicatives** (`hr-apps`, `hr-db`, `hr-git`, `hr-flow`, `hr-flow-macros`, `hr-dataverse`, `hr-dataverse-migrate`, `hr-dvexpr`, `hr-dv-codegen`) ont été internalisées dans Atelier (`crates/hr-XXX`) le 2026-05-09 — modifiables localement.

## Service naming + autonomie

Atelier est **autonome** : ses services portent le préfixe `atelier-app-` et ne partagent ni nom ni path avec hr-orchestrator (qui continue à tourner sur Medion pour la partie network/registry).

| Concept | hr-orchestrator (Medion) | Atelier (Medion) |
|---|---|---|
| Service principal | `hr-orchestrator.service` | `atelier.service` |
| Apps spawn | (legacy, désactivé) | `atelier-app-{slug}.service` |
| Slice | `hr-apps.slice` (legacy) | `atelier-apps.slice` |
| Apps runtime root | `/opt/homeroute/apps/` (legacy) | `/var/lib/atelier/apps/` |
| Registry | `/opt/homeroute/data/apps.json` | `/opt/atelier/data/apps.json` |

Override possible via env vars `ATELIER_APP_UNIT_PREFIX` / `ATELIER_APP_SLICE` / `ATELIER_APPS_RUNTIME_ROOT` (utilisé pendant la fenêtre de transition CM→Medion).

## Workflow d'agent

À chaque fois que tu travailles dans Atelier :

1. Lire `MEMORY.md` global (auto-chargé) et les rules dans `.claude/rules/`.
2. Si la tâche concerne une app HomeRoute existante (`/opt/homeroute/apps/{slug}/` sur CloudMaster), suivre la doctrine **DOC-FIRST** : `mcp__studio__docs_overview` d'abord.
3. Pour toute fonctionnalité ajoutée à Atelier : doc + tests e2e + logging structuré.
4. **Pour toute action runtime** (statut, logs, restart) : passer par l'API Atelier sur Medion (`https://app.mynetwk.biz/api/...` ou `ssh romain@10.0.0.254 "sudo journalctl -u atelier..."`). Pas d'accès direct à `/opt/homeroute/apps/` côté Medion (n'existe plus depuis le rapatriement).
