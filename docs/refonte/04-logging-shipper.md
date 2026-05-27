# Sub-phase — Logging shipper (Phase 4 partielle)

## État

- Statut : DONE
- Démarré : 2026-05-26
- Terminé : 2026-05-26

## Objectif

Permettre aux 6 apps (5 Rust + 1 Next.js) de loguer vers le service de logging centralisé d'Atelier (`/api/logs/ingest`), sans tirer `sqlx-postgres` (qui était la raison pour laquelle la sub-phase a été différée Phase 1).

## Livrables

### 1. Crate Rust standalone `atelier-logging-shipper`

- Chemin : `/nvme/atelier/crates/atelier-logging-shipper/`
- **Standalone** (`[workspace]` vide + exclu du workspace Atelier racine via `exclude = [...]`) pour éviter le conflit reqwest 0.12 (apps) vs 0.13 (atelier core).
- Deps minimales : `tokio` (rt + sync + macros + time), `tracing`, `tracing-subscriber`, `serde`, `serde_json`, `chrono`, `reqwest = "0.12"` (rustls-tls, json).
- API publique :
  - `HttpShipperLayer::from_env(service, app_slug)` — lit `ATELIER_INGEST_URL` + `ATELIER_LOGS_TOKEN`, retourne `None` si manquant.
  - `HttpShipperLayer::start(cfg)` — spawn la tâche batch.
  - `HttpShipperConfig` — config tunable (batch_size default 200, batch_interval default 5s).
- Le layer batche les events tracing dans un canal mpsc, drain toutes les 5s ou 200 entries, POST `Vec<RawIngestEntry>` avec bearer auth. Best-effort : drop silencieux sur send failure (jamais de crash app).
- Types `RawIngestEntry`, `LogLevel`, `LogCategory`, `LogSource` dupliqués (matches `atelier-logging/types.rs`).

### 2. Helper TypeScript `atelier-logger.ts` (www)

- Chemin : `/opt/homeroute/apps/www/src/lib/atelier-logger.ts`
- Pure fonction `log.{trace,debug,info,warn,error}(message, fields?)` qui double l'output (`console.*` + batch HTTP).
- Batch toutes les 5s ou 100 entries, POST sur `/api/logs/ingest` avec bearer.
- Fallback silencieux à `console.*` only si `ATELIER_INGEST_URL` / `ATELIER_LOGS_TOKEN` manquants.

## Intégration apps — DONE

| App | Stack | service | app_slug | Statut |
|---|---|---|---|---|
| files | Rust/Axum | app-files | files | ✅ logs en DB |
| home | Rust/Axum | app-home | home | ✅ logs en DB |
| trader | Rust/Axum | app-trader | trader | ✅ logs en DB |
| myfrigo | Rust/Axum | app-myfrigo | myfrigo | ✅ logs en DB |
| wallet | Rust/Axum | app-wallet | wallet | ✅ logs en DB |
| www | Next.js | app-www | www | ✅ logs en DB |

### Pattern apps Rust

Pour chaque app Rust : `tracing_subscriber::fmt()` remplacé par `registry().with(env_filter).with(fmt::layer()).with(shipper).init()` (le shipper conditionnellement attaché si `from_env` retourne `Some`).

### Pattern app Next.js (www)

L'helper `log.*` remplace `console.error/warn/info` dans les routes API. Le batch est partagé au niveau module-level dans le serveur Node (singleton).

### Env injectés

`ATELIER_INGEST_URL=http://127.0.0.1:4100` + `ATELIER_LOGS_TOKEN=<token>` dans chaque `.env` canonique côté CloudMaster (`/opt/homeroute/apps/{slug}/.env`). Le `make deploy-app` rsync l'env vers Medion.

## Vérification end-to-end — DONE

`/api/logs/stats` retourne :
- 11822 logs totaux (atelier+apps)
- 7 services : atelier (9667), app-files (1968), app-trader (169), app-home (13), app-myfrigo (2), app-wallet (2), app-www (1)
- by_level : info (9669), warn (2153), 0 error (preuve système sain)

Échantillon de log app-www :
```json
{"app_slug":"www","level":"warn","service":"app-www","category":"system",
 "message":"contact business error",
 "fields":{"httpStatus":400,"message":"contact_type 'nonexistent_type' not found"}}
```

## Décisions

- Le crate `atelier-logging-shipper` est **standalone hors workspace** (vs path-dep depuis workspace) car :
  - Le workspace Atelier utilise `reqwest = "0.13"`.
  - Les apps utilisent `reqwest = "0.12"` (différence de version trop loin pour cargo unify).
  - Solution standard : `exclude` côté workspace racine + `[workspace]` vide dans le crate enfant pour qu'il agisse comme un workspace single-crate.

- Le mode batch best-effort (drop silencieux sur réseau down) est volontaire : un service de logging ne doit jamais crash l'app appelante.

- Pas de `tracing-subscriber::fmt()` retiré côté apps : on garde `fmt::layer()` pour que `journalctl` reste utile (deux sorties parallèles).

## Notes

- Le helper TS `atelier-logger.ts` n'est pour l'instant utilisé que dans `/api/contact/route.ts`. Les 2 autres routes (legal/[page], admin/contact-requests/[id]) peuvent être migrées plus tard (ce sont juste des `console.error` à remplacer par `log.error`).
- Le shipper batch interval est 5s : un log peut prendre jusqu'à 5s pour apparaître dans `/api/logs`. Pour le rendre instantané il faudrait baisser l'interval (impact perf POST).
- Pas de retry sur send failure : si Atelier est down momentanément, les logs sont perdus. Ce trade-off est explicite (pas de queue persistante v1).
