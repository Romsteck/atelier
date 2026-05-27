# Refonte wallet

## État

- Statut : DONE (deployé + smoke tests passent)
- Démarré : 2026-05-26
- Terminé : 2026-05-26
- Stack : Rust/Axum + Vite client
- Branche atelier : eradication-flows
- Branche app : pas créée (modifs directes sur `/opt/homeroute/apps/wallet/src/server/`)

## Inventaire

### Flows TOML existants (10)

| TOML | Appelé par | Décision |
|---|---|---|
| `update_transaction.toml` | `transactions::update` | `native::update_transaction` (Result<UpdateOutcome>) |
| `apply_suggestions.toml` | `transactions::bulk_categorize` + `ai::apply_suggestions` | `native::apply_suggestions` |
| `delete_batch.toml` | `transactions::delete_batch` | `native::delete_batch` |
| `save_settings.toml` | `settings::save` | `native::save_settings` |
| `import_csv.toml` | `import::upload_csv` | `native::import_csv` (avec audit `import_logs`) |
| `recommendations_monthly.toml` | `recommendations::monthly` | inline : `load_all_transactions` + `compute_monthly_recommendations` (déjà natif dans le crate) |
| `recommendations_health.toml` | `recommendations::portfolio_health` | inline : `load_all_transactions` + `compute_portfolio_health` (déjà natif) |
| `suggestions.toml` | `ai::suggestions` | `native::suggestions` (load + filter + sort + take 50 + OpenRouter + parse) |
| `insights.toml` | `ai::insights` | `native::insights` (load + aggregate_month_stats + OpenRouter + parse) |
| `score_transaction.toml` | jamais appelé directement (endpoint `_internal/flows/run` debug uniquement) | suppression sèche |

### Actions custom (4 utilisées + 1 dead)

| Action | Décision |
|---|---|
| `compute_risk_score` | Port en fonction Rust pure (`native::compute_risk_score`) — restera disponible mais plus appelée (dead-code warning toléré) |
| `aggregate_month_stats` | Port en fonction Rust pure (`native::aggregate_month_stats`) — utilisée par `native::insights` |
| `dedup_by_reference` | Inliné dans `native::import_csv` (HashSet de références) |
| `compute_monthly_recommendations` | Déjà natif dans `routes::recommendations::compute_monthly_recommendations` — réutilisé direct |
| `compute_portfolio_health` | Déjà natif dans `routes::recommendations::compute_portfolio_health` — réutilisé direct |

### Connecteurs custom (2)

| Connecteur | Décision |
|---|---|
| `dataverse` | Remplacé par appels directs au client typé `dv-wallet` |
| `openrouter` | Wrapper autour de `routes::openrouter::chat_completion` existant — utilisé directement par `native::suggestions/insights` |

## Suppression — DONE

- [x] `src/server/src/flows/` (mod.rs + actions/* + connectors/* + internal_routes.rs) supprimé
- [x] `src/flows/` (10 TOML) supprimé
- [x] `main.rs` : `mod flows;`, route `_internal/flows/run`, route `_internal/flows/replay`, `flow_engine` field, `Backend::from_env` + dual-mode → tous supprimés
- [x] `AppState` simplifié : juste `dv: Option<DvClient>` (avant : + `flow_engine: Option<Arc<FlowEngine>>`)
- [x] `Cargo.toml` : deps `hr-flow` + `hr-flow-callback` retirées
- [x] `cargo build --release` vert (28s, 7 dead-code warnings tolérés : code utilitaire inutilisé après refonte)
- [x] `grep -rn "hr_flow\|hr-flow\|register_callbacks\|engine\.run\|flow_engine\|FlowEngine"` dans src/ vide (les 2 derniers commentaires obsolètes nettoyés)

## Vérification (deploy) — DONE

- [x] Autorisation deploy reçue
- [x] `CARGO_HOME=/home/romain/.cargo CI=true make deploy-app SLUG=wallet` succès build + rsync sources
- [x] Push binaire manuel `target/release/wallet-server` vers Medion (même bug `/server/target/` exclusion)
- [x] Restart via API succès
- [x] App active sur Medion (port 3009)
- [x] Loopback `/api/health` → `{"status":"ok"}` (200)
- [x] Loopback `/api/transactions?limit=2` → 200 + 2 transactions sérialisées
- [x] Loopback `/api/recommendations/monthly?month=2026-05` → 200 + JSON complet (savingsRate, dailySpendingVelocity, categoryAlerts, recurringChargesRatio)
- [x] Loopback `/api/recommendations/health` → 200 + JSON complet (healthScore=33, spendingTrend, scoreComponents)
- [x] Loopback `/api/settings` → 200 + settings retournés (openrouter_api_key + ai_model)
- [ ] `POST /api/ai/suggestions` non testé en smoke : appellerait OpenRouter (effet de bord coûteux). Logique : load_all_transactions + filter + sort + take 50 + chat_completion + parse JSON.
- [ ] `POST /api/ai/insights` non testé : idem.
- [ ] `POST /api/transactions/{id}` update non testé : effet de bord sur DV. Logique : get + update + retry 1× via `update_transaction`.
- [ ] `POST /api/import` non testé : nécessite un upload de CSV.

## Intégration logging — DIFFÉRÉE

Même décision : sub-phase post-Phase 3 avec `atelier-logging-shipper` léger.

## Reverify J+1

- [ ] Pas d'erreur 24h
- [ ] Métriques DB cohérentes (transactions, settings, import_logs)
- [ ] Pas de régression remontée (Transactions, Settings, Categorize via IA, Insights, Recommendations, Import)

## Notes

- Wallet était le plus complexe : 10 flows + 4 actions + 2 connecteurs custom + mode dual `embedded`/`remote`. Tout passe en code Rust natif en 1 module (`services/native.rs`, ~600 LOC).
- Le connecteur `openrouter` n'était qu'un wrapper autour de la fn existante `routes::openrouter::chat_completion` — pas de logique à réimplémenter, juste appel direct.
- Le mode dual `Backend::from_env` (env `HR_FLOW_BACKEND=embedded|remote`) disparaît : il n'a jamais servi en prod (rapport Phase 4 dual-mode pilot).
- Les actions `compute_monthly_recommendations` et `compute_portfolio_health` étaient déjà des wrappers JSON autour de fns Rust pures dans `routes::recommendations` — réutilisées direct.
- `score_transaction.toml` n'était appelé que via l'endpoint debug `/api/_internal/flows/run` (jamais en prod) — suppression sèche.
- `compute_risk_score` reste en code Rust pur dans `native.rs` au cas où — utile pour test, mais générera un warning dead_code.
