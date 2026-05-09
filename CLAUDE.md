# Atelier — Plateforme applicative HomeRoute

## Statut migration (2026-05-09)

> ✅ **Phase 9 cutover + 9.5 cleanup terminés** — apps migrées de Medion vers CloudMaster, hr-orchestrator slim sur Medion (network-only), code applicatif (10 crates) déplacé dans `crates/hr-XXX` localement.
>
> Détails : [memory/project_atelier_cutover_done.md](/home/romain/.claude/projects/-nvme-homeroute/memory/project_atelier_cutover_done.md), [memory/project_homeroute_post_cleanup.md](/home/romain/.claude/projects/-nvme-homeroute/memory/project_homeroute_post_cleanup.md)
>
> Plan d'extraction complet (copie locale) : [docs/plan-extraction.md](docs/plan-extraction.md)

### Restant à faire (différé)

- **9.2 finition write endpoints Atelier** — list/get/env/control/status implémentés. Manquent : create/update/delete/build/deploy/exec/update_env/regenerate_context/logs/todos. Boutons Studio/DbExplorer correspondants → 404 silencieux.
- **Refacto homeroute-core** — découpler `hr-common`/`hr-ipc`/`hr-docs`/`hr-acme` qu'Atelier path-dep encore vers `/nvme/homeroute/crates/shared/` et `/edge/`.

## Plan suivant — `hr-flowd` daemon multi-stack

📌 **Plan complet dans le repo** : [docs/plan-hr-flowd.md](docs/plan-hr-flowd.md) (copie locale, source originale `/home/romain/.claude/plans/peaceful-spinning-mountain.md`)

Transformer `hr-flow` (lib Rust embeddable, donc inutilisable côté NextJS) en daemon partagé `hr-flowd` accessible via callbacks HTTP par toutes les apps quelle que soit leur stack. **Explicitement reporté** par l'utilisateur ; à reprendre quand le cutover sera stable et 9.5 décidé.

⚠️ **Pendant les évolutions de `hr-flow` ici** : ne pas refactorer de façon qui rendrait l'extraction du daemon plus difficile. Pas de nouveau couplage fort à `ApiState` ou au runtime des apps. Cf. [memory/project_atelier_hrflowd_pending.md](/home/romain/.claude/projects/-nvme-homeroute/memory/project_atelier_hrflowd_pending.md).

---

## Quoi est Atelier

Plateforme applicative extraite de HomeRoute. Contient :

- **Apps** : supervisor Tokio des apps locales (lifecycle, ports, logs)
- **Dataverse** : moteur Postgres avec schéma dynamique, GraphQL, dvexpr
- **Flows** : moteur d'orchestration TOML
- **Studio** : code-server intégré + frontend de gestion
- **Docs** : système de documentation per-app (FTS5)
- **Git** : bare repos
- **Store** : catalogue Flutter
- **MCP** : exposition des tools `app.*`, `db.*`, `docs.*`, `flow.*`

Atelier ne contient **pas** : DNS, DHCP, reverse proxy, ACME, monitoring hosts (ces concerns restent dans homeroute sur Medion).

## Architecture (atteinte 2026-05-09)

```
Internet → Cloudflare → Medion (10.0.0.254)
                          ├─ hr-edge (proxy + ACME + auth + tunnel)
                          │   ├─ {slug}.mynetwk.biz → 10.0.0.10:port-app (CloudMaster)
                          │   ├─ proxy.mynetwk.biz   → Medion:4000 (homeroute network API)
                          │   └─ app.mynetwk.biz     → 10.0.0.10:4100 (Atelier)
                          ├─ hr-netcore (DNS, DHCP, adblock, ipv6)
                          └─ homeroute (4000) network API + Postgres (5432)

CloudMaster (10.0.0.10)
  ├─ atelier.service (4100) — supervisor + apps API + frontend
  ├─ hr-app-{slug}.service (transient, 3001-3010) — apps spawn par Atelier
  └─ hr-studio.service (8443) — code-server (Studio iframe)
```

## Stockage

| Données | Chemin |
|---------|--------|
| Sources des apps + runtime | `/opt/homeroute/apps/{slug}/{src,bin,.env,db.sqlite,runs}` (CloudMaster local) |
| Atelier registry canonical | `/opt/atelier/data/{apps.json, port-registry.json}` |
| Atelier runtime | `/opt/atelier/{bin,web/dist}` |
| Docs FTS5 + sync mirror | `/var/lib/atelier/{docs, docs-index.sqlite}` |
| Atelier env | `/opt/atelier/.env` (contient ATELIER_DV_ADMIN_URL + ATELIER_APPS_RUNTIME_ROOT) |
| Postgres-dataverse | Medion 10.0.0.254:5432 (LAN, accédé par apps + Atelier) |
| dataverse-secrets.json | `/var/lib/atelier/state/dataverse-secrets.json` (snapshot pré-cutover) |

## Ports & sockets

| Port/socket | Service |
|---|---|
| 4100 | Atelier HTTP API |
| /run/atelier.sock | Atelier IPC |
| 8443 | code-server (hr-studio.service) |

## Build & deploy

```bash
make atelier      # cargo build --release
make web          # frontend Vite (Phase 2+)
make deploy       # build + restart systemd local + healthcheck
```

Atelier tourne sur la même machine que la dev (CloudMaster) — pas de rsync cross-host.

## Règles obligatoires

- **JAMAIS** `cargo run` directement — utiliser `make deploy`
- **TOUJOURS** `make deploy` après modification du code Rust
- **TOUJOURS** vérifier visuellement après deploy frontend (SW cache-first peut masquer le résultat)
- **TOUJOURS** tester e2e les endpoints créés/modifiés (cf. `.claude/rules/testing.md`)
- **TOUJOURS** logger structuré (cf. `.claude/rules/logging.md`)
- **JAMAIS** d'attribution Claude dans les commits

## Path-deps vers homeroute (résiduel)

Atelier consomme encore ces crates partagées de homeroute (refacto `homeroute-core` = travail futur) :

```toml
hr-common = { path = "/nvme/homeroute/crates/shared/hr-common" }
hr-ipc    = { path = "/nvme/homeroute/crates/shared/hr-ipc" }
hr-docs   = { path = "/nvme/homeroute/crates/shared/hr-docs" }
```

Ne jamais modifier ces crates depuis Atelier — leur source de vérité reste dans `/nvme/homeroute/`. Si un changement est nécessaire, le faire dans homeroute, valider que homeroute build, puis re-build Atelier.

Les **10 crates applicatives** (`hr-apps`, `hr-db`, `hr-git`, `hr-flow`, `hr-flow-macros`, `hr-dataverse`, `hr-dataverse-migrate`, `hr-dvexpr`, `hr-dv-codegen`) ont été déplacées dans Atelier (`crates/hr-XXX`) le 2026-05-09 — modifiables localement sans coordination homeroute.

## Workflow d'agent

À chaque fois que tu travailles dans Atelier :

1. Lire `MEMORY.md` global de l'utilisateur (auto-chargé) et les rules dans `.claude/rules/`.
2. Si la tâche concerne une app HomeRoute existante (`/opt/homeroute/apps/{slug}/`), suivre la doctrine **DOC-FIRST** : `mcp__homeroute__docs_overview` d'abord (pendant la phase parallèle, le MCP docs est encore servi par hr-orchestrator sur Medion).
3. Pour toute fonctionnalité ajoutée à Atelier : doc + tests e2e + logging structuré.