# Refonte home

## État

- Statut : DONE (deployé + smoke tests passent)
- Démarré : 2026-05-26
- Terminé : 2026-05-26
- Stack : Rust/Axum + Vite client
- Branche atelier : eradication-flows
- Branche app : pas créée (modifs directes sur `/opt/homeroute/apps/home/src/server/`)

## Inventaire

### Flows TOML existants (8)

| TOML | Appelé par (route) | Décision |
|---|---|---|
| `aquarium_feed.toml` | `aquarium::feed_handler` (POST /api/aquarium/feed) | Service Rust natif (ESP32 POST + audit) |
| `configure_aquarium_feed_schedule.toml` | `aquarium::feed_schedule_handler` | Repo extension (delete-all + insert) |
| `configure_aquarium_schedule.toml` | `aquarium::schedule_handler` | Repo extension (for_each upsert by hour) |
| `device_proxy_get.toml` | `devices::{diag,logs,state}_handler` | Service Rust natif (lookup + GET) |
| `device_proxy_post.toml` | `devices::command_handler` | Service Rust natif (lookup + POST + audit) |
| `device_rename.toml` | `devices::rename_handler` | Repo extension (lookup + update avec retry) |
| `set_aquarium_brightness.toml` | `aquarium::brightness_handler` | Réutilise `aquarium_state::upsert` existant |
| `toggle_aquarium_schedule.toml` | `aquarium::schedule_toggle_handler` | Réutilise `aquarium_state::upsert` existant |

### Mapping flow → fonction native

| Flow | Fonction Rust cible | Fichier |
|---|---|---|
| aquarium_feed | `services::aquarium_feed::feed_command(dv, host, steps, speed, direction)` | `services/aquarium_feed.rs` |
| configure_aquarium_feed_schedule | `dv_repo::aquarium_feed_schedule::replace_all(dv, entries)` | `dv_repo.rs` |
| configure_aquarium_schedule | `dv_repo::aquarium_schedule::set_all(dv, &[(hour, brightness)])` | `dv_repo.rs` |
| device_proxy_get | `services::device_proxy::get(dv, slug, path)` | `services/device_proxy.rs` |
| device_proxy_post | `services::device_proxy::post(dv, slug, path, body)` | `services/device_proxy.rs` |
| device_rename | `dv_repo::devices::rename_by_slug(dv, slug, name)` | `dv_repo.rs` |
| set_aquarium_brightness | `dv_repo::aquarium_state::upsert(dv, Some(brightness), None)` | (existant) |
| toggle_aquarium_schedule | `dv_repo::aquarium_state::upsert(dv, None, Some(enabled))` | (existant) |

## Sémantique critique préservée

- **`aquarium_feed`** : si transport ESP32 fail (network down, timeout) → erreur propagée + **PAS d'audit `command_history`** (équivalent au TOML qui Fail dans ce cas). Si non-2xx → audit avec `success=false` + erreur 502. Si 2xx → audit `success=true`.
- **`device_proxy_post`** : si transport fail → erreur propagée + **pas d'audit**. Si has_ip=false → erreur 400 + pas d'audit. Si non-2xx → audit `success=false`. Si 2xx → audit `success=true`.
- **`device_proxy_get`** : pas d'audit (équivalent au TOML qui n'écrit jamais dans command_history pour GET).

## Suppression — DONE

- [x] `src/flows/` (8 TOML) supprimé
- [x] `state::flow_engine` retiré + `AppState::new` signature simplifiée
- [x] `routes/mod.rs` : `require_flow_engine` + `run_persistence_flow` supprimés
- [x] `main.rs` : init `RemoteEngine` retiré
- [x] `Cargo.toml` : dep `hr-flow` retirée
- [x] `cargo build --release` vert (4.4s)
- [x] `cargo clippy -- -D warnings -A clippy::too_many_arguments` vert (le too_many_arguments est pré-existant sur `command_history::insert`, hors périmètre)
- [x] `grep -rn "hr_flow\|hr-flow\|flow_engine"` vide

## Vérification (deploy) — DONE

- [x] Autorisation deploy reçue
- [x] `CARGO_HOME=/home/romain/.cargo CI=true make deploy-app SLUG=home` succès build (le CARGO_HOME override est requis localement car le script n'en injecte pas)
- [x] **Bonus** : push binaire `target/release/smart-home` manuel sur Medion via rsync direct (le script `deploy-app.sh` exclut `/server/target/` du rsync, ce qui signifie qu'aucune app Rust n'a réellement été déployée via `make deploy-app` depuis le rapatriement — voir Notes).
- [x] Restart via API succès
- [x] App active sur Medion (PID 666304, port 3007, binaire 2026-05-26)
- [x] Loopback `/api/health` → `{"status":"healthy","uptime":46,"esp32_aquarium":"online"}`
- [x] Loopback `/api/aquarium/status` → 200 + JSON structuré (brightness=93, schedule de 24h, 3 feed schedules)
- [x] Loopback `/api/devices` → 200 + 2 devices listés (aquarium offline, esp32-lights online)
- [x] Loopback `/api/aquarium/ping` → `{"online":true}`
- [x] POST `/api/aquarium/brightness` (93) → `{"success":true,"brightness":93,"persisted":true}` (preuve `set_aquarium_brightness` natif fonctionne)
- [x] POST `/api/aquarium/schedule/toggle` (true) → `{"success":true,"persisted":true}` (preuve `toggle_aquarium_schedule` natif fonctionne)
- [ ] POST `/api/aquarium/feed` non testé en smoke pour ne pas distribuer du sable inutilement à l'aquarium (le code passe par `aquarium_feed::feed_command` qui POST `/feed` sur ESP32 + insert command_history).

## Intégration logging — DIFFÉRÉE

Même décision que files/www : sub-phase post-Phase 3 avec `atelier-logging-shipper` léger. home continue de loguer stdout → journalctl.

## Reverify J+1

- [ ] Pas d'erreur 24h
- [ ] Métriques DB cohérentes (aquarium_state, aquarium_schedule, command_history, devices)
- [ ] Schedulers continuent à tourner (aquarium_light_schedule_task, aquarium_feed_schedule_task, log_cleanup_task) — non impactés par la refonte
- [ ] Pas de régression remontée (ESP32 aquarium feed, device rename, device proxy GET/POST)

## Notes

**Bug d'infrastructure deploy détecté** : `scripts/deploy-app.sh` exclut `/server/target/` (et `/target/`, `/api/target/`) du rsync. Pour les apps Rust (`axum-vite`), le binaire produit par `cargo build --release` reste donc sur CloudMaster et n'arrive pas sur Medion. Pour livrer effectivement :
- soit le `build_command` doit terminer par un `cp target/release/<binary> bin/<slug>` (le `bin/` n'est pas exclu),
- soit le script doit rsync séparément le binaire après le build.

Pendant cette Phase 2.3, j'ai contourné le bug avec un `rsync` direct de `target/release/smart-home` vers Medion. À fixer hors périmètre (modif transverse du script).

**Conséquence rétroactive sur Phase 2.1 (files)** : les "smoke tests" Phase 2.1 utilisaient en réalité l'ancien binaire `bin/files` de mai 9 (avec runFlow → hr-flowd). Le nouveau binaire compilé sur CM (avec `dataverse_ops`) n'avait pas été poussé. J'ai corrigé en re-pushant le binaire `target/release/home-cloud` → `bin/files` sur Medion. Smoke tests refait : `POST /api/sync/check` et `POST /api/files/check-exists` répondent toujours OK, mais cette fois c'est bien le code natif refondu qui exécute (PID changé, mtime du binaire = 2026-05-26).
