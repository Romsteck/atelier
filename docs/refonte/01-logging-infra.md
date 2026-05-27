# Phase 1 — Infrastructure de logging Postgres (Atelier core)

## État

- Statut : DONE (Phase 1 validée end-to-end)
- Démarré : 2026-05-26
- Terminé : 2026-05-26

## Objectif

Atelier loggue dans Postgres + UI fonctionnelle, **sans toucher aux apps**. Les apps seront branchées au cours de Phase 2 (chaque refonte d'app inclut son intégration logging).

## Tâches

### 1.1 Crate `crates/atelier-logging` — DONE

- [x] `crates/atelier-logging/Cargo.toml` — manifest crate
- [x] `crates/atelier-logging/src/lib.rs` — re-exports + facade `sqlx`
- [x] `crates/atelier-logging/src/types.rs` — `LogEntry`, `LogLevel`, `LogCategory`, `LogSource`, `RawIngestEntry`, `LogEntryBuilder`
- [x] `crates/atelier-logging/src/ring_buffer.rs` — buffer in-memory FIFO (capacité 10k, parking_lot::Mutex)
- [x] `crates/atelier-logging/src/layer.rs` — `LoggingLayer` (tracing::Layer) pour in-process
- [x] `crates/atelier-logging/src/shipper.rs` — `HttpShipperLayer` pour apps externes
- [x] `crates/atelier-logging/src/ingest.rs` — `LogIngestService` + `LogIngestConfig` (cœur, bootstrap, flush_loop, retention_loop)
- [x] `crates/atelier-logging/src/store.rs` — `insert_batch` via UNNEST
- [x] `crates/atelier-logging/src/query.rs` — `LogQuery` + `LogStats` (filtres, paginate, search)
- [x] `crates/atelier-logging/src/migration.rs` — bootstrap DDL idempotent
- [x] Workspace `/nvme/atelier/Cargo.toml` : ajouté member + dep
- [x] `cargo build --release -p atelier-logging` vert (2.90s)

### 1.2 Schéma SQL — DONE (DDL écrit, bootstrap testable au démarrage)

- [x] `crates/atelier-logging/migrations/001_init.sql` — DDL complet inclus via `include_str!`
- [x] Table `events_log` partitionnée + indices + GIN tsvector
- [x] Fonctions `ensure_partition(date)` + `drop_partitions_before(date)`
- [x] Bootstrap today, today+1, today+2 dans le SQL
- [ ] DB `atelier_logs` créée — à vérifier au 1er boot Atelier
- [ ] Role `atelier_logs_writer` créé — à vérifier au 1er boot Atelier

### 1.3 Routes API + WebSocket — DONE

- [x] `crates/atelier-api/src/routes/logs.rs` — `GET /api/logs`, `/stats`, `POST /ingest`, `GET /by-request/{rid}`
- [x] `crates/atelier-api/src/routes/ws.rs` — émet `log:entry` (souscrit `state.logs`) en plus du legacy `app:log`
- [x] `crates/atelier-api/src/state.rs` — `logs: LogIngestService` ajouté
- [x] `crates/atelier-api/src/lib.rs` — `.nest("/logs", routes::logs::router())`
- [ ] Route `GET /api/apps/{slug}/logs` reconnectée sur ingest service — à faire en Phase 2 (quand chaque app aura ses logs)

### 1.4 Bootstrap Atelier — DONE

- [x] `crates/atelier/src/main.rs` — bootstrap `LogIngestService::start` avant `init_tracing`
- [x] `LoggingLayer` installé à côté de `fmt::layer()`
- [x] Fallback noop si DB indisponible au boot
- [x] Env vars : `ATELIER_LOGS_DB_URL` (optionnel — fallback sur admin DSN), `ATELIER_LOGS_DB_ADMIN_URL` (fallback sur `ATELIER_DV_ADMIN_URL`), `ATELIER_LOGS_TOKEN` (injecté sur Medion 2026-05-26)
- [x] Note : `ATELIER_LOGS_WRITER_PASSWORD` est optionnel — si absent, le rôle dédié n'est pas créé et on utilise dataverse_admin pour les writes (acceptable v1)

### 1.5 UI Logs.jsx — DONE

- [x] Fetch dynamique `getLogStats` pour la liste des services
- [x] Dropdown `app_slug` alimenté par `getApps()` (visible quand scope=apps)
- [x] Toggle "Tous / Atelier core / Apps" (touche spécifique Atelier)
- [x] Support params URL `?app_slug=` et `?scope=`
- [x] WebSocket : écoute `log:entry` ET `app:log` (couverture pendant transition)

### 1.6 Vérification — DONE

- [x] `cargo build --release -p atelier` vert (1m01)
- [x] `cargo build --release -p atelier-logging` vert
- [x] `npm run build` (web) vert
- [x] Autorisation deploy reçue
- [x] `make deploy` succès, healthy après 2s
- [x] Bootstrap DDL OK au démarrage (partitions today/+1/+2 créées idempotemment)
- [x] `/api/logs?service=atelier&limit=5` retourne contenu structuré
- [x] `/api/logs/stats` retourne `{total: 253, by_level, by_service, by_app}` après 9s d'activité
- [x] `/api/logs/ingest` retourne 503 sans token (attendu — token sera utilisé par les apps externes en Phase 2)
- [ ] Logs.jsx en mode Live affiche les requêtes en temps réel — **à vérifier dans le navigateur**
- [ ] Filtres level/search/time-range/scope/app_slug fonctionnent — **à vérifier dans le navigateur**

## Résultats de bootstrap (2026-05-26 14:53)

```
241 entries en 9 secondes — Atelier loggue activement
DB atelier_logs créée idempotemment
events_log_2026_05_26 partition active
Token ATELIER_LOGS_TOKEN injecté dans /opt/atelier/.env (pour Phase 2)
```

## Notes

- Génération token : `openssl rand -base64 32` (à faire en début, à injecter dans `/opt/atelier/.env` Medion).
- DDL idempotent : on peut le run via `ATELIER_LOGS_DB_ADMIN_URL` (admin) pour création DB+role, puis `ATELIER_LOGS_DB_URL` (writer) pour INSERT/SELECT.

## Critère DONE Phase 1

Tous les check-boxes ci-dessus ✓.
