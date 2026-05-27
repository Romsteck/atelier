# Refonte myfrigo

## État

- Statut : DONE (deployé + smoke tests passent)
- Démarré : 2026-05-26
- Terminé : 2026-05-26
- Stack : Rust/Axum + Vite client (PWA)
- Branche atelier : eradication-flows
- Branche app : pas créée (modifs directes sur `/opt/homeroute/apps/myfrigo/src/api/`)

## Inventaire

### Flows TOML existants (6)

| TOML | Appelé par | Décision |
|---|---|---|
| `get_recipe_with_details.toml` | `recipes::get_recipe` | Remplacé par `recipe_service::get_by_uuid` (existait déjà natif) |
| `add_recipe_favorite.toml` | `recipes::add_favorite` | `recipe_service::add_favorite` natif |
| `remove_recipe_favorite.toml` | `recipes::remove_favorite` | `recipe_service::remove_favorite` natif |
| `list_favorite_recipes.toml` | `recipes::get_favorites` | `recipe_service::list_favorites` natif |
| `save_recipe_adjustments.toml` | `recipes::save_adjustments` | `recipe_service::save_adjustments` natif |
| `create_sync_session.toml` | **NON appelé** (handler `sync::create` utilise déjà `sync_service::create` natif depuis le début) | Suppression sèche du TOML + action `generate_sync_code` |

### Action custom `generate_sync_code`

L'action Rust `#[flow_action(name = "generate_sync_code")]` n'était utilisée que par `create_sync_session.toml`, qui n'était lui-même jamais appelé (cf. commentaire pré-existant : "Migration vers `create_sync_session` flow en attente : la primitive `while` n'est pas encore implémentée dans hr-flow"). Suppression complète sans impact.

## Suppression — DONE

- [x] `src/api/src/flows/` (mod.rs + actions/sync_code + connectors/dataverse) supprimé
- [x] `src/flows/` (6 TOML) supprimé
- [x] `main.rs` : `mod flows;` + `merge(flows::register_callbacks())` retirés
- [x] `Cargo.toml` : deps `hr-flow` + `hr-flow-callback` + commentaire dataverse-connector retirés (ainsi que la dep `reqwest` redondante en doublon)
- [x] `handlers/recipes.rs` : `run_flow` helper + 3 structs `*FlowOutput` supprimés, 5 handlers rebranchés sur `recipe_service`
- [x] Commentaire obsolète "Migration vers create_sync_session flow en attente" retiré de `handlers/sync.rs`
- [x] `cargo build --release` vert (31s)
- [x] `grep -rn "hr_flow\|hr-flow\|register_callbacks\|run_flow"` dans src/ vide

## Intégration logging — DIFFÉRÉE

Même décision : sub-phase post-Phase 3 avec `atelier-logging-shipper` léger.

## Vérification (deploy) — DONE

- [x] Autorisation deploy reçue
- [x] `CARGO_HOME=/home/romain/.cargo CI=true make deploy-app SLUG=myfrigo` succès build + rsync sources
- [x] Push binaire manuel `target/release/my-frigo-api` vers Medion (toujours le bug `/server/target/` quirk, ici le binary est à `src/target/release/`, donc même issue d'exclusion via `/target/`)
- [x] Restart via API succès
- [x] Loopback `/api/health` → `{"service":"my-frigo-api","status":"ok","version":"1.0.0"}`
- [x] Loopback `/api/recipes?limit=2` → 200 + recettes hydratées (preuve `recipe_service::get_all` fonctionne)
- [x] Loopback `/api/recipes/favorites` → `{"recipes":[]}` (200, preuve `list_favorites` fonctionne)
- [ ] POST `/api/recipes/{uuid}/favorite` non testé (effet de bord sur table favorites) — logique triviale
- [ ] POST `/api/recipes/{uuid}/adjustments` non testé (effet de bord destructeur) — logique préservée du TOML

## Reverify J+1

- [ ] Pas d'erreur 24h
- [ ] Métriques DB cohérentes (recipes, recipe_favorites, recipe_adjustments, recipe_ingredients)
- [ ] Pas de régression remontée sur la PWA (Recipes, Favoris, Ajustements, Sync)

## Notes

- `get_by_uuid` (déjà natif avant la refonte) répliquait exactement la sortie de `get_recipe_with_details.toml`. Le handler appelait `run_flow` au lieu — gain net en perfs (1 appel direct au lieu de POST loopback Atelier + 3 list DV + compose).
- L'action `generate_sync_code` était classée "action légitime" (fonction pure) dans son header mais n'était mountée que pour un flow non utilisé. Suppression sèche.
- 2 flows morts (`create_sync_session`, `generate_sync_code`) éliminent un anti-pattern : les TOML traînaient sans qu'aucun caller ne les déclenche.
