# Atelier — Plateforme applicative HomeRoute

## Plan en cours / Plan suivant

> 🚧 **Migration en cours** : Atelier est en cours d'extraction depuis `/nvme/homeroute/`.
> Plan complet : [/home/romain/.claude/plans/purring-gathering-hopper.md](/home/romain/.claude/plans/purring-gathering-hopper.md)
>
> 📌 **Plan suivant (post-cutover, OBLIGATOIRE à enchaîner)** :
> [/home/romain/.claude/plans/peaceful-spinning-mountain.md](/home/romain/.claude/plans/peaceful-spinning-mountain.md)
>
> Une fois la migration terminée (cutover Phase 9 du plan en cours), enchaîner sur ce second plan : transformer `hr-flow` (lib Rust embeddable, donc inutilisable côté NextJS) en daemon partagé `hr-flowd` accessible via callbacks HTTP par toutes les apps quelle que soit leur stack.
>
> ⚠️ **Pendant la migration de hr-flow vers Atelier (Phase 5/6 du plan en cours)** : ne pas refactorer `hr-flow` de façon qui rendrait l'extraction du daemon plus difficile. Pas de couplage fort à `ApiState` ou au runtime des apps.

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

## Architecture (cible)

```
Internet → Cloudflare → Medion (10.0.0.254)
                          └─ hr-edge (homeroute) — proxy + ACME + auth + tunnel
                                └─ app.mynetwk.biz → CloudMaster:4100 (Atelier)
CloudMaster (10.0.0.10)
  └─ atelier (4100 HTTP, /run/atelier.sock IPC) — ce service
  └─ hr-studio.service (8443) — code-server pour le Studio
```

## Stockage

| Données | Chemin |
|---------|--------|
| Sources des apps | `/opt/homeroute/apps/{slug}/src/` (déjà sur CloudMaster) |
| Runtime Atelier | `/opt/atelier/` (binaire) |
| DB Atelier (SQLite) | `/var/lib/atelier/orchestrator.db` |
| Docs index FTS5 | `/var/lib/atelier/docs-index.sqlite` |
| .env | `/opt/atelier/.env` |
| Postgres-dataverse | externe (LAN, à confirmer Phase 7) |

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

## Path-deps vers homeroute

Atelier consomme aujourd'hui ces crates de homeroute via path-dep (refacto en repo séparé `homeroute-core` = travail futur) :

```toml
hr-common = { path = "/nvme/homeroute/crates/shared/hr-common" }
hr-ipc    = { path = "/nvme/homeroute/crates/shared/hr-ipc" }
hr-docs   = { path = "/nvme/homeroute/crates/shared/hr-docs" }
```

Ne jamais modifier ces crates depuis Atelier — leur source de vérité reste dans `/nvme/homeroute/`. Si un changement est nécessaire, le faire dans homeroute, valider que homeroute build, puis re-build Atelier.

## Workflow d'agent

À chaque fois que tu travailles dans Atelier :

1. Lire `MEMORY.md` global de l'utilisateur (auto-chargé) et les rules dans `.claude/rules/`.
2. Si la tâche concerne une app HomeRoute existante (`/opt/homeroute/apps/{slug}/`), suivre la doctrine **DOC-FIRST** : `mcp__homeroute__docs_overview` d'abord (pendant la phase parallèle, le MCP docs est encore servi par hr-orchestrator sur Medion).
3. Pour toute fonctionnalité ajoutée à Atelier : doc + tests e2e + logging structuré.