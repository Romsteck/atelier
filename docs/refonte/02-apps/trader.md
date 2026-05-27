# Refonte trader

## État

- Statut : DONE (deployé + smoke tests partiels passent)
- Démarré : 2026-05-26
- Terminé : 2026-05-26
- Stack : Rust/Axum + Vite client (embedded)
- Branche atelier : eradication-flows
- Branche app : pas créée (modifs directes sur `/opt/homeroute/apps/trader/src/server/`)

## Inventaire

### Flows TOML existants (4)

| TOML | Appelé par | Décision |
|---|---|---|
| `portfolio_metrics.toml` (146 lignes) | `portfolio_routes::get_metrics` | Service Rust natif (fold over trades + snapshots) |
| `purge_old_alerts.toml` | `alerts::purge_old` (DELETE /api/alerts/old) | Service Rust natif (list + delete) |
| `backfill_currencies.toml` | `config_routes::backfill_currencies` | Service Rust natif (list + Finnhub HTTP + update + sleep 200 ms) |
| `delete_all_recommendations.toml` | `recommendations::delete_all_recommendations` | Service Rust natif (cascade purge paginée) |

### Mapping flow → fonction native

| Flow | Fonction Rust | Fichier |
|---|---|---|
| portfolio_metrics | `dataverse_ops::portfolio_metrics(dv, portfolio_id) -> PortfolioMetricsAgg` | `services/dataverse_ops.rs` |
| purge_old_alerts | `dataverse_ops::purge_old_alerts(dv, cutoff_rfc3339) -> i64` | idem |
| backfill_currencies | `dataverse_ops::backfill_currencies(dv, http, key) -> BackfillResult` | idem |
| delete_all_recommendations | `dataverse_ops::delete_all_recommendations(dv) -> i64` | idem |

## Suppression — DONE

- [x] `src/server/src/flows/` supprimé (mod.rs + actions/ + connectors/ + invoke.rs)
- [x] `src/flows/` (4 TOML) supprimés
- [x] `lib.rs` : `pub mod flows;` retiré
- [x] `routes/mod.rs` : `merge(crate::flows::register_callbacks())` retiré + import retiré
- [x] `Cargo.toml` : deps `hr-flow` + `hr-flow-callback` retirées
- [x] `cargo build --release` vert (1m25s après cargo clean)
- [x] Commentaires "Thin wrapper over X flow on hr-flowd" remplacés par descriptions natives

## Intégration logging — DIFFÉRÉE

Même décision que les autres apps Rust : sub-phase post-Phase 3 avec `atelier-logging-shipper` léger. trader continue de loguer stdout → journalctl.

## Vérification (deploy) — DONE

- [x] Autorisation deploy reçue
- [x] `CARGO_HOME=/home/romain/.cargo CI=true make deploy-app SLUG=trader` succès build + rsync
- [x] **Push binaire manuel** `target/release/trader-server` vers Medion (le script `deploy-app.sh` exclut `/server/target/`, idem home)
- [x] Restart via API succès
- [x] App active sur Medion (PID 8231, port 3008)
- [x] Loopback `/api/health` → `{"status":"ok","timestamp":...}` (200)
- [x] Loopback `/api/portfolios` → 200 + 2 portfolios listés
- [x] Loopback `/api/alerts?limit=2` → 200 + alertes listées
- [x] Loopback `/api/portfolios/1/metrics` → 200 + JSON métriques (total_trades=0, ratios=null, cohérent pour portfolio sans trade) — preuve `portfolio_metrics` natif fonctionne
- [x] Loopback `DELETE /api/alerts/old` → `{"deleted":0}` (200) — preuve `purge_old_alerts` natif fonctionne (rien à purger dans le 7-day window)
- [ ] `POST /api/config/symbols/backfill-currencies` non testé : nécessite une clé Finnhub valide et écrirait dans symbol_configs (effet de bord trop large pour smoke).
- [ ] `DELETE /api/recommendations` non testé : effet de bord destructeur, n'est pas appelé par défaut. La logique paginée + cascade est triviale (cf. dataverse_ops::delete_all_recommendations).

## Reverify J+1

- [ ] Pas d'erreur 24h
- [ ] Métriques portfolio cohérentes (lazy : appelé par la page Portfolio dans le frontend)
- [ ] Schedulers continuent à tourner (TieredScheduler, DailyScheduler, LiveTrader, PersistentAutoTrader, history_logger) — non impactés par la refonte
- [ ] Pas de régression remontée

## Notes

- `portfolio_metrics.toml` faisait 146 lignes TOML. La version Rust native (`portfolio_metrics`) tient en ~50 lignes — `for` natif + `min_by(partial_cmp)` au lieu de filter/sort/take chaînés.
- `backfill_currencies` : sleep 200 ms inter-appels Finnhub préservé (`tokio::time::sleep`), retry logic retirée (le `max_retries=3` du TOML n'était pas implémenté côté daemon de toute façon, simple best-effort).
- `delete_all_recommendations` : pas de `$count` côté `dv-trader`, on pagine jusqu'à batch vide (équivalent fonctionnel à `while count > 0`).
