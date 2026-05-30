# Atelier — Plateforme applicative HomeRoute

## Statut migration (2026-05-27)

> ✅ **Studio Atelier + sources apps rapatriés sur Medion** — `atelier-studio.service` (code-server, 127.0.0.1:8443) tourne sur Medion, route hr-edge `codeserver.mynetwk.biz → 127.0.0.1:8443`. Les sources canoniques des 6 apps vivent désormais à `/var/lib/atelier/apps/<slug>/src/` (source = runtime, plus de copie interne).
>
> Précédemment (2026-05-09) : Atelier supervisor + apps rapatriés sur Medion. CloudMaster ne servait plus que le Studio + sources apps.
>
> **Maintenant CloudMaster héberge encore** : (a) le code source d'Atelier (`/nvme/atelier/`) — édité, buildé et déployé via `make deploy` ; (b) le code-server perso (port 9080, `code.mynetwk.biz`).

### Restant à faire (différé)

- **9.2 finition write endpoints Atelier** — list/get/env/control/status implémentés. Manquent : create/update/delete/build/deploy/exec/update_env/regenerate_context/logs/todos. Boutons Studio/DbExplorer correspondants → 404 silencieux. **Symptôme observé** : Files /api/files/upload retourne `dv: gateway error 405`, et le scheduler `home` log toutes les 10s `failed to log command_history: gateway error 405`.
- **Path-routing `app.mynetwk.biz/apps/{slug}`** — but initial de la séparation Studio. Le proxy par chemin est en place dans Atelier (`crates/atelier-api/src/routes/apps_proxy.rs`). `www` (Next.js) est servi en **no-strip** : son préfixe `/apps/www` est préservé jusqu'à l'app (requis par `basePath`/`assetPrefix`). Les slugs no-strip sont listés via `ATELIER_PRESERVE_PREFIX_SLUGS` (défaut `www`) ; les autres apps (Vite/Axum) restent en strip. La généralisation (basePath par app, auth path-aware, bascule des hostnames) reste reportée ; cf. [.claude/rules/path-routing-pending.md](.claude/rules/path-routing-pending.md).

## Système de flux — supprimé (2026-05-26)

Le système `hr-flow` (lib + daemon + macros + callback + 34 TOML répartis sur 6 apps) a été éradiqué le 2026-05-26. Chaque app a été refondue en code natif (Rust ou TS). Voir [docs/refonte/](docs/refonte/) pour le journal détaillé. Les 4 crates `hr-flow*` ont été supprimées du workspace, le daemon `hr-flowd` désinstallé sur Medion, et toutes les routes/MCP tools/UI flow ont été retirées d'Atelier.

---

## Quoi est Atelier

Plateforme applicative autonome (post-rapatriement 2026-05-09). Contient :

- **Apps** : supervisor Tokio des apps locales (lifecycle, ports, logs) — services nommés `atelier-app-{slug}.service` (slice `atelier-apps.slice`).
- **Dataverse** : moteur Postgres avec schéma dynamique, GraphQL, dvexpr.
- **Docs** : système de documentation per-app (FTS5).
- **Git** : bare repos.
- **MCP** : exposition des tools `app.*`, `db.*`, `docs.*`.

Atelier ne contient **pas** : DNS, DHCP, reverse proxy, ACME (ces concerns restent dans `hr-edge` + `hr-netcore` sur Medion). Le **Studio** (code-server pour éditer les apps) tourne désormais aussi sur Medion (`atelier-studio.service`).

## Architecture (post-rapatriement Studio, 2026-05-27)

```
Internet → Cloudflare → Medion (10.0.0.254)
                          ├─ hr-edge (proxy + ACME + auth + tunnel)
                          │   ├─ app.mynetwk.biz       → 127.0.0.1:4100      (Atelier API + frontend)
                          │   ├─ codeserver.mynetwk.biz → 127.0.0.1:8443     (Studio code-server)
                          │   ├─ atelier.mynetwk.biz    → 127.0.0.1:4100     (Atelier)
                          │   └─ code.mynetwk.biz       → 10.0.0.10:9080     (code-server perso CM)
                          ├─ atelier.service (4100) — supervisor + apps API + frontend
                          ├─ atelier-studio.service (127.0.0.1:8443) — code-server pour apps
                          ├─ atelier-app-{files,home,myfrigo,trader,wallet,www}.service
                          ├─ hr-edge.service / hr-orchestrator.service / homeroute.service
                          └─ Postgres-dataverse (5432)

CloudMaster (10.0.0.10)  ← reste allumé
  ├─ /nvme/atelier/  — sources Atelier (édition + make deploy)
  └─ code-server@romain.service (9080, code.mynetwk.biz) — usage perso (édite Atelier, etc.)
```

## Stockage

| Données | Chemin |
|---------|--------|
| Sources canoniques apps (= runtime) | `/var/lib/atelier/apps/{slug}/{src,bin,.env,db.sqlite,runs,todos.json}` (Medion) — édition via Studio |
| Studio code-server user-data | `/var/lib/atelier/studio/code-server/` (Medion, hr-studio:hr-studio 750) |
| Studio user HOME | `/var/lib/hr-studio/` (Medion, user `hr-studio` UID 993) |
| Atelier registry canonical | `/opt/atelier/data/{apps.json, port-registry.json}` (Medion) |
| Atelier binaire + frontend | `/opt/atelier/{bin/atelier,web/dist}` (Medion) |
| Atelier env | `/opt/atelier/.env` (Medion) |
| Docs FTS5 + index | `/var/lib/atelier/{docs, docs-index.sqlite}` (Medion) |
| Postgres-dataverse | Medion 127.0.0.1:5432 (local depuis Atelier) |
| dataverse-secrets.json | `/var/lib/atelier/state/dataverse-secrets.json` (Medion) |
| **Files data (raid0)** | `/ssd_pool/homecloud/data/{files,thumbnails,downloads,films}` (Medion zfs pool) |
| Sources Atelier (code) | `/nvme/atelier/` (CloudMaster — édition + `make deploy`) |

## Ports & sockets

| Port/socket | Hôte | Service |
|---|---|---|
| 4100 | Medion | Atelier HTTP API |
| /run/atelier.sock | Medion | Atelier IPC |
| 3005-3010 | Medion (loopback) | Apps |
| 8443 | Medion (loopback) | atelier-studio.service (code-server) |
| 9080 | CloudMaster | code-server perso (`code.mynetwk.biz`) |

## Variables d'environnement Atelier (Medion `/opt/atelier/.env`)

```
ATELIER_DV_ADMIN_URL=postgres://dataverse_admin:...@127.0.0.1:5432/postgres
ATELIER_APPS_RUNTIME_ROOT=/var/lib/atelier/apps
ATELIER_APPS_SRC_ROOT=/var/lib/atelier/apps
ATELIER_DV_HOST=127.0.0.1
# Defaults: ATELIER_APP_UNIT_PREFIX=atelier-app, ATELIER_APP_SLICE=atelier-apps.slice
```

## Build & deploy

### Atelier lui-même (depuis CloudMaster)

Le code source d'Atelier (`/nvme/atelier/`) vit sur CloudMaster, édité via le code-server perso (`code.mynetwk.biz`).

```bash
make help              # tous les targets
make atelier           # cargo build --release -p atelier (local CM)
make web               # build frontend (web/dist)
make deploy            # build CM + push binary + frontend vers Medion + restart atelier.service
make logs              # tail journalctl atelier sur Medion (via SSH)
```

### Apps HomeRoute (depuis CM ou Studio Medion)

Les sources des 6 apps vivent désormais sur Medion (`/var/lib/atelier/apps/<slug>/src/`). On les édite via le Studio (`codeserver.mynetwk.biz`). Le `make deploy-app` se lance soit depuis CM (mode SSH automatique vers Medion), soit directement sur Medion via un terminal Studio.

```bash
make deploy-app SLUG=files   # build sur Medion (local ou via SSH) + restart via API + healthcheck
```

Le script (`scripts/deploy-app.sh`) détecte `hostname == medion` → build in-place, sinon → SSH vers Medion. Plus de rsync transversal source/runtime (source = runtime depuis le 2026-05-27).

## Règles obligatoires

- **JAMAIS** `cargo run` directement — utiliser `make deploy`.
- **TOUJOURS** `make deploy` après modification du code Atelier (build CM → rsync Medion → restart).
- **TOUJOURS** `make deploy-app SLUG=<x>` après modification d'une app (build Medion + restart via API).
- **TOUJOURS** vérifier visuellement après deploy frontend (SW cache-first peut masquer le résultat).
- **TOUJOURS** tester e2e les endpoints créés/modifiés (cf. `.claude/rules/testing.md`).
- **TOUJOURS** logger structuré (cf. `.claude/rules/logging.md`).
- **JAMAIS** d'attribution Claude dans les commits.

## Crates internes (plus aucun path-dep vers homeroute)

Atelier est **autonome** : toutes ses crates vivent sous `crates/` et portent le préfixe `atelier-*` (renommées depuis `hr-*` le 2026-05-30, en même temps que la purge du code mort hérité de homeroute). Le découplage `homeroute-core` est **terminé** (2026-05-30) — les 3 dernières crates partagées (`atelier-common`, `atelier-ipc`, `atelier-docs`) ont été rapatriées depuis `/nvme/homeroute/crates/shared/` (snapshot du HEAD homeroute) car un refacto en cours côté homeroute avait divergé leur API et cassait le build d'Atelier.

Crates internes (`crates/atelier-XXX`, modifiables localement) :
- `atelier-common`, `atelier-ipc`, `atelier-docs` — internalisées le 2026-05-30.
- `atelier-apps`, `atelier-db`, `atelier-git`, `atelier-dataverse`, `atelier-dataverse-migrate`, `atelier-dvexpr`, `atelier-dv-codegen` — internalisées le 2026-05-09.

Les 4 crates `hr-flow*` ont été supprimées le 2026-05-26. Atelier ne lit plus rien dans `/nvme/homeroute/` : éditer/refondre ces crates ne nécessite plus de garder homeroute compilable.

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
2. Si la tâche concerne une app HomeRoute existante (`/var/lib/atelier/apps/{slug}/src/` sur Medion, éditée via Studio), suivre la doctrine **DOC-FIRST** : `mcp__studio__docs_overview` d'abord.
3. Pour toute fonctionnalité ajoutée à Atelier : doc + tests e2e + logging structuré.
4. **Pour toute action runtime** (statut, logs, restart) : passer par l'API Atelier sur Medion (`https://app.mynetwk.biz/api/...` ou `ssh romain@10.0.0.254 "sudo journalctl -u atelier..."`).
