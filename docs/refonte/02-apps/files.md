# Refonte files

## État

- Statut : DONE (deployé + smoke tests passent)
- Démarré : 2026-05-26
- Terminé : 2026-05-26
- Branche atelier : eradication-flows
- Branche app : pas créée (modifs directes sur `/opt/homeroute/apps/files/src/` — pas de repo git séparé identifié)

## Inventaire

### Flows TOML existants (3)

| TOML | Appelé par | Réel | Décision |
|---|---|---|---|
| `bulk_check_hashes.toml` | `routes/sync.rs:32` (`sync_check`) | OUI | Refonte en fonction native |
| `check_files_exist.toml` | `routes/sync.rs:75` (`check_exists`) | OUI | Refonte en fonction native |
| `ensure_folder_path.toml` | aucun (commentaire "bloqué par bug plateforme" dans `folders.rs:327`) | NON | Supprimer tel quel, jamais utilisé |

### Mapping flow → fonction native

| Flow | Fonction Rust cible | Fichier | Statut |
|---|---|---|---|
| bulk_check_hashes | `services::dataverse_ops::bulk_check_hashes(dv, hashes)` | `services/dataverse_ops.rs` | [x] |
| check_files_exist | `services::dataverse_ops::check_files_exist(dv, folder_id, candidates)` | `services/dataverse_ops.rs` | [x] |
| ensure_folder_path | supprimé (jamais appelé — version native déjà dans `routes/folders.rs::ensure_path`) | n/a | [x] |

## Avant

- [x] Build app référence vert (38s)
- [x] App active sur Medion durant la refonte (deploy-app gère le restart atomique)

## Intégration logging — DIFFÉRÉE

- [ ] Décision : repoussée à une sub-phase ultérieure. Raison : intégrer `atelier-logging` en path-dep dans chaque app tirerait `sqlx-postgres` (heavy) + conflit reqwest 0.12 vs 0.13. Solution propre : créer un thin crate `atelier-logging-shipper` séparé qui ne dépend que de `tracing` + `reqwest`. À faire après Phase 3 (teardown) pour ne pas doubler le rsync.
- En attendant : files continue de loguer stdout → journalctl (capté par `journalctl -u atelier-app-files`).

## Suppression — DONE

- [x] `src/server/src/flows/` supprimé (mod.rs, runner.rs, actions/)
- [x] `src/flows/*.toml` supprimés (3 fichiers + dossier)
- [x] `Cargo.toml` : `hr-flow`, `hr-flow-callback` retirés
- [x] `main.rs` : `.merge(flows::register_callbacks())` retiré, `mod flows` retiré
- [x] `cargo build --release` vert (38s, 13 warnings dead_code pré-existants, 0 erreur)
- [x] `grep -rn "hr_flow\|hr-flow\|run_flow\|runFlow" src/` vide

## Vérification (deploy) — DONE

- [x] Autorisation deploy reçue
- [x] `CI=true make deploy-app SLUG=files` succès (build CM + rsync Medion + restart)
- [x] App active sur Medion (PID 645077, 78 MB)
- [x] Loopback `http://10.0.0.254:3006/api/health` → 200
- [x] Smoke test 1 : POST `/api/sync/check` → `{"existing_hashes":[]}` (fonction `bulk_check_hashes` native fonctionne)
- [x] Smoke test 2 : POST `/api/files/check-exists` → `{"existing":[]}` (fonction `check_files_exist` native fonctionne)
- [ ] Healthcheck `https://files.mynetwk.biz/api/health` retourne 404 depuis Medion lui-même — possiblement quirk hr-edge sans rapport (loopback 200 OK). À investiguer hors périmètre.
- [ ] Logs visibles dans `/logs?app_slug=files` — N/A (shipper différé)

## Reverify J+1

- [ ] Pas d'erreur 24h
- [ ] Métriques DB cohérentes
- [ ] Pas de régression remontée

## Notes

Le script `scripts/deploy-app.sh` exécute `pnpm install --frozen-lockfile` pour le frontend Vite. Sans TTY (lancé en non-interactif), pnpm refuse de nettoyer node_modules → erreur `ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY`. Solution : `CI=true make deploy-app SLUG=files` (signale à pnpm qu'on est en CI). À noter dans la rule deploy-chain ou à fixer dans le script.


## Reverify J+1

- [ ] Pas d'erreur 24h
- [ ] Métriques DB cohérentes (pas de drift sur table `files`)
- [ ] Pas de régression remontée
