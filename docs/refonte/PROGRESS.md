# Refonte Atelier — PROGRESS

> **Demande originale (2026-05-26)** :
> 0. Éradiquer le système de flux (analyse per-app + plan spécifique + refonte 1 par 1 en mode autonome).
> 1. Créer/réutiliser un logging précis et généralisé → Postgres (db dédiée sur l'instance dataverse).
>    - Vue globale + vue par app avec filtres propres.
>    - Touche de spécificité pour Atelier.
> Fichiers de suivi hiérarchiques + boucles de vérification.
> Plan complet : [/home/romain/.claude/plans/le-syst-me-de-gestion-cached-koala.md](/home/romain/.claude/plans/le-syst-me-de-gestion-cached-koala.md)

## Statut global

Dernière mise à jour : 2026-05-26

| Phase | Statut | Fichier |
|---|---|---|
| Phase 0 — Préconditions | In-progress | [00-preconditions.md](./00-preconditions.md) |
| Phase 1 — Logging infrastructure | DONE (UI à vérifier navigateur) | [01-logging-infra.md](./01-logging-infra.md) |
| Phase 2.1 — Refonte files | DONE + Reverify J+1 OK | [02-apps/files.md](./02-apps/files.md) |
| Phase 2.2 — Refonte www | DONE + Reverify J+1 OK | [02-apps/www.md](./02-apps/www.md) |
| Phase 2.3 — Refonte home | DONE + Reverify J+1 OK | [02-apps/home.md](./02-apps/home.md) |
| Phase 2.4 — Refonte trader | DONE + Reverify J+1 OK | [02-apps/trader.md](./02-apps/trader.md) |
| Phase 2.5 — Refonte myfrigo | DONE + Reverify J+1 OK | [02-apps/myfrigo.md](./02-apps/myfrigo.md) |
| Phase 2.6 — Refonte wallet | DONE + Reverify J+1 OK | [02-apps/wallet.md](./02-apps/wallet.md) |
| Phase 3 — Teardown infra flow | DONE (atelier deployed healthy) | [03-teardown-flows.md](./03-teardown-flows.md) |
| Sub-phase — Logging shipper | DONE (6/6 apps loguent vers Postgres) | [04-logging-shipper.md](./04-logging-shipper.md) |
| Phase 4 — UI extensions | Pending (utilité réduite, voir notes) | [04-ui-extensions.md](./04-ui-extensions.md) |

## Métriques

| Métrique | Initial | Cible | Actuel |
|---|---|---|---|
| Flows TOML | 34 | 0 | 0 ✅ |
| Crates flow (`hr-flow*`) | 4 | 0 | 0 ✅ |
| Apps refondues | 0 / 6 | 6 / 6 | 6 / 6 ✅ |
| Routes API flow | ~10 | 0 | 0 ✅ |
| LOC supprimées (cumul) | 0 | ~5000+ | ~6000 ✅ |
| Apps loguant vers Postgres | 0 / 6 | 6 / 6 | 6 / 6 ✅ (via atelier-logging-shipper) |

## Branches / tags git

- Tag de référence : `pre-eradication-2026-05-26` (sur main)
- Branche de travail : `eradication-flows`
- Modifs uncommit pré-éradication : stash `pre-eradication-stash-2026-05-26`

## Décisions architecturales

- [ADR-001](./decisions/ADR-001-pas-de-nouveau-moteur.md) — Pas de nouveau moteur, code natif par app
- [ADR-002](./decisions/ADR-002-logging-postgres.md) — Logging en DB Postgres dédiée
- [ADR-003](./decisions/ADR-003-partitioning.md) — Partitionnement journalier
- [ADR-004](./decisions/ADR-004-http-shipping.md) — HTTP shipping pour les apps externes

## Reverify J+1 — DONE 2026-05-27

Vérification 24h post-deploy (commit `23180c2`, tag `flow-eradication-complete-2026-05-26`).

| App | Loopback `/api/health` | Errors DB (24h) | Errors journalctl (24h) | Régression ? |
|---|---|---|---|---|
| files | 200 (`:3006`) | 0 | 0 | Non |
| home | 200 (`:3007`) | 0 | 0 | Non |
| trader | 200 (`:3008`) | 0 | 0 | Non |
| wallet | 200 (`:3009`) | 0 | 0 | Non |
| myfrigo | 200 (`:3010`) | 0 | 0 | Non |
| www | 200 (`:3005/apps/www`) | 0 | 0 | Non |

Note www : pas de route `/api/health` (pré-existant, le `health_path` du registry est obsolète), mais `/apps/www` répond 200 et l'app sert ses routes normalement.

### Métriques DB cohérentes

| App | Tables | Counts |
|---|---|---|
| files | files, folders | 23647 / 5243 |
| home | aquarium_state, aquarium_schedule, command_history, devices | 1 / 24 / 1441 / 2 |
| trader | virtual_portfolios, alerts, recommendations, symbol_configs | 2 / 3321 / 4424 / 34 |
| wallet | transactions, settings, import_logs | 1466 / 5 / 10 |
| myfrigo | recipes, recipe_favorites, recipe_adjustments, recipe_ingredients | 92 / 6 / 1 / 1002 |
| www | contact_requests, legal_contents | 9 / 5 |

Aucun drift. Aucune régression remontée. Schedulers home/trader continuent à tourner (audit `_dv_audit` actif : 6604 entrées home, 9783 entrées trader). `hr-flowd` confirmé `inactive` + unit not found sur Medion.

### Logs Postgres globaux

`66 013` entries · 7 services (atelier 59 397 + 6 apps) · 2 errors total (avant J-1) · **0 dans les dernières 24h**.

## Journal

### 2026-05-27 — Reverify J+1 OK
- 6/6 apps loopback healthy, 0 erreur DB + 0 erreur journalctl sur 24h.
- Métriques DB inchangées (pas de drift), schedulers home/trader actifs.
- Refonte stabilisée. Aucune action corrective requise.

### 2026-05-26 — Lancement
- Audit complet flux + apps + logging existant (3 sub-agents Explore).
- Plan validé par l'utilisateur.
- Structure de suivi créée sous `docs/refonte/`.
- Tag `pre-eradication-2026-05-26` + branche `eradication-flows` créés.
- Stash `pre-eradication-stash-2026-05-26` pour 6 fichiers hr-flow* modifiés (perdables vu que les crates seront supprimées).
- Phase 0 DONE (build `-p atelier` + web OK, services Medion tous actifs).
- Bug pré-existant noté : `cargo build --workspace` casse sur `hr-dataverse-migrate` (variant `FieldType::Money` non couvert). Hors périmètre.
- Note env : `CARGO_HOME=/home/romain/.cargo` (override) requis pour les builds locaux.

### 2026-05-26 — Phase 2.1 files DONE
- 3 TOML supprimés (`bulk_check_hashes`, `check_files_exist`, `ensure_folder_path` — ce dernier jamais appelé).
- 2 fonctions natives dans `services/dataverse_ops.rs` (~85 LOC).
- `flows/` dir + Cargo.toml hr-flow* purgés.
- Build vert 38s, deploy-app succès (CI=true requis pour pnpm sans TTY).
- Smoke tests passent : POST `/api/sync/check` et `/api/files/check-exists`.
- Décision logging shipper différée (sqlx-postgres + reqwest version conflict) → sub-phase post-Phase 3.

### 2026-05-26 — Sub-phase logging shipper DONE
- Nouveau crate standalone `crates/atelier-logging-shipper` (~300 LOC) excluded from workspace pour gérer le conflit reqwest 0.12 (apps) vs 0.13 (atelier core).
- API publique `HttpShipperLayer::from_env(service, app_slug)` + `start(cfg)` — batch tracing events vers `/api/logs/ingest`.
- 5 apps Rust intégrées : files, home, trader, myfrigo, wallet (chacune ~10 LOC dans main.rs : registry + with(env_filter) + with(fmt::layer) + conditional with(shipper)).
- 1 app Next.js (www) : helper TS `lib/atelier-logger.ts` (~140 LOC) avec `log.{trace,debug,info,warn,error}` qui double `console.*` + batch HTTP.
- Env `ATELIER_INGEST_URL` + `ATELIER_LOGS_TOKEN` injectés dans les 6 `.env` canoniques (côté CloudMaster).
- 6 apps déployées via push binaires + restart.
- Vérif `/api/logs/stats` : 11822 logs total, 7 services (atelier + 6 apps), info + warn, 0 error.

### 2026-05-26 — Phase 2.6 wallet DONE (la dernière app)
- 10 TOML supprimés (update_transaction, apply_suggestions, delete_batch, save_settings, import_csv, suggestions, insights, recommendations_monthly, recommendations_health, score_transaction).
- 4 actions custom + 2 connecteurs (dataverse, openrouter) tous portés en code natif.
- 1 nouveau module `services/native.rs` (~600 LOC) avec : helpers (load_all_transactions, strip_md_fences, parse dates), update_transaction (UpdateOutcome), apply_suggestions, delete_batch, save_settings, import_csv (avec audit `import_logs`), compute_risk_score, aggregate_month_stats, suggestions, insights.
- 5 handlers (transactions, ai, settings, recommendations, import) rebranchés direct sur le module natif.
- `mod flows;` + `_internal/flows/{run,replay}` routes + `flow_engine` AppState + `Backend::from_env` dual-mode + deps `hr-flow`/`hr-flow-callback` → tous supprimés.
- `cargo build --release` vert 28s (7 dead-code warnings tolérés).
- Smoke tests sur loopback :3009 : `/api/health` 200, `/api/transactions?limit=2` 200 + 2 transactions, `/api/recommendations/monthly` 200 + JSON complet (savingsRate, dailySpendingVelocity, categoryAlerts), `/api/recommendations/health` 200 + JSON complet (healthScore, spendingTrend), `/api/settings` 200.
- AI endpoints (`/api/ai/suggestions`, `/api/ai/insights`) non testés en smoke (coût OpenRouter), logique préservée du TOML.

**Phase 2 — TERMINÉE.** Toutes les apps refondues, plus aucun TOML, mais l'infra `hr-flow*` reste encore présente dans Atelier (routes, MCP tools, crates) — à supprimer en Phase 3.

### 2026-05-26 — Phase 2.5 myfrigo DONE
- 6 TOML supprimés. Surprise : `create_sync_session` + action `generate_sync_code` étaient morts (handler `sync::create` utilisait déjà `sync_service::create` natif).
- 4 flows actifs (get_recipe_with_details, add/remove/list favorites, save_adjustments) → 4 nouvelles fonctions natives dans `recipe_service` (~150 LOC) + 1 réutilisée (`get_by_uuid`).
- Handler `recipes.rs` allégé : `run_flow` helper + 3 structs `*FlowOutput` supprimés, 5 handlers rebranchés direct sur `recipe_service`.
- `mod flows;`, `register_callbacks` merge, deps `hr-flow`/`hr-flow-callback` → tous supprimés.
- `cargo build --release` vert 31s.
- Smoke tests sur loopback :3010 : `/api/health` 200, `/api/recipes?limit=2` 200 (recettes hydratées), `/api/recipes/favorites` 200 (`{"recipes":[]}`).

### 2026-05-26 — Phase 2.4 trader DONE
- 4 TOML supprimés (portfolio_metrics, purge_old_alerts, backfill_currencies, delete_all_recommendations).
- 1 module `services/dataverse_ops.rs` ~300 LOC avec 4 fonctions natives.
- 4 routes (`portfolio_routes::get_metrics`, `alerts::purge_old`, `config_routes::backfill_currencies`, `recommendations::delete_all_recommendations`) rebranchées.
- `src/server/src/flows/` (mod.rs + actions + connectors + invoke.rs) + `src/flows/` + `lib.rs::pub mod flows` + register_callbacks merge + deps `hr-flow`/`hr-flow-callback` → tous supprimés.
- `cargo clean` requis (cache corrompue après build initial), puis `cargo build --release` vert 1m25s.
- Smoke tests sur loopback :3008 : `/api/health` 200, `/api/portfolios` 200 (2 portfolios), `/api/portfolios/1/metrics` 200 (total_trades=0), `DELETE /api/alerts/old` 200 (`{"deleted":0}` cohérent).
- 2 ops (`backfill_currencies`, `delete_all_recommendations`) non testées en smoke car effet de bord trop large ; logique triviale et build vert.

### 2026-05-26 — Phase 2.3 home DONE
- 8 TOML supprimés (aquarium_feed/schedule/feed_schedule + device proxy/rename + brightness/toggle).
- 2 nouveaux services (`services/aquarium_feed.rs` + `services/device_proxy.rs`) ~200 LOC.
- 3 ops ajoutées à `dv_repo.rs` (`aquarium_schedule::set_all`, `aquarium_feed_schedule::replace_all`, `devices::rename_by_slug`).
- 5 ops réutilisent l'existant `dv_repo::aquarium_state::upsert`.
- Sémantique préservée : si transport ESP32 fail → erreur propagée + pas d'audit `command_history` (idem TOML qui Fail). Si non-2xx → audit `success=false`. Si 2xx → audit `success=true`.
- `state::flow_engine`, `routes::require_flow_engine`/`run_persistence_flow`, init `RemoteEngine` et dep `hr-flow` → tous supprimés.
- `cargo build --release` vert 4.4s. Clippy vert (modulo `too_many_arguments` pré-existant).
- Smoke tests sur loopback :3007 : `/api/health`, `/api/aquarium/status` (avec brightness=93 + schedule 24h + feed_schedules), `/api/devices` (2 devices listés), POST brightness=93 + schedule/toggle persistent OK.
- **Bug deploy détecté** : `deploy-app.sh` exclut `/server/target/` du rsync → les binaires Rust ne sont jamais déployés via `make deploy-app SLUG=`. Workaround : rsync direct manuel du binaire. À fixer hors périmètre.
- **Phase 2.1 (files) ré-validée à juste titre** : l'ancien binaire `bin/files` de mai 9 tournait encore (avec hr-flowd). Push manuel du nouveau binaire + re-smoke tests : `dataverse_ops` natif fonctionne réellement maintenant.

### 2026-05-26 — Phase 2.2 www DONE
- 3 TOML supprimés (handle_contact_request, open_contact_request, get_or_create_legal_page).
- 2 services TS dans `lib/services/` (contact.ts ~120 LOC, legal.ts ~13 LOC).
- 3 routes (`/api/contact`, `/api/admin/contact-requests/[id]`, `/api/legal/[page]`) rebranchées sur les services.
- `lib/flow/` + `app/%5Fflow/` + `flows/` purgés. Dep `@homeroute/flow-action` retirée de `package.json`.
- `npx tsc --noEmit` + `npm run build` verts.
- `CI=true make deploy-app SLUG=www` succès (rsync + restart).
- Smoke tests sur loopback :3005 (le proxy public www.mynetwk.biz est cassé indépendamment, cf. path-routing-pending) : `GET /api/contact-types` 200, `GET /api/legal/mentions-legales` 200, `POST /api/contact` (object=meeting) 201 → entry id=17 créée puis cleanée.

### 2026-05-26 — Phase 1 DONE (end-to-end validé)
- Crate `atelier-logging` créé (10 fichiers, ~1100 LOC) + migration SQL.
- Wiring complet : `routes/logs.rs`, `ApiState.logs`, `ws.rs` (émet `log:entry`), bootstrap `atelier/main.rs`, frontend Logs.jsx.
- `make deploy` succès, healthy 2s.
- Bootstrap DDL OK : DB `atelier_logs` créée, partitions today/+1/+2 idempotemment, indices + functions en place.
- 241 entries en 9s — Atelier loggue activement toutes les requêtes DV.
- `/api/logs?service=atelier`, `/api/logs/stats` retournent du contenu structuré.
- `ATELIER_LOGS_TOKEN` injecté dans `/opt/atelier/.env` (sera lu au prochain restart, utile pour Phase 2 apps externes).
- Reste à vérifier dans le navigateur : page /logs Live mode, filtres scope/app_slug.
- Note touche Atelier-spécifique : segment "Tous / Atelier core / Apps" dans Logs.jsx, distingue visuellement la plateforme des apps métier.
