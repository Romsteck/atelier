# Rapatriement Studio Atelier + sources apps : CloudMaster → Medion (2026-05-27)

## Quoi a bougé

- **Service code-server (Studio)** : `hr-studio.service` (CM, 10.0.0.10:8443) → `atelier-studio.service` (Medion, 127.0.0.1:8443, alias `hr-studio.service`).
- **Sources canoniques des 6 apps** : `/opt/homeroute/apps/<slug>/src/` (CM) → `/var/lib/atelier/apps/<slug>/src/` (Medion). Source = runtime, plus de copie interne.
- **User-data Studio** : `/opt/homeroute/data/code-server/` (CM) → `/var/lib/atelier/studio/code-server/` (Medion).
- **HOME du user `hr-studio`** (UID 993, GID 984) : `/var/lib/hr-studio/` (créé sur Medion, transféré de CM).
- **Route hr-edge** `codeserver.mynetwk.biz` → `127.0.0.1:8443` (au lieu de `10.0.0.10:8443`).
- **scripts/deploy-app.sh** : refactor — build sur Medion (local si lancé là, sinon SSH depuis CM). Plus de rsync transversal. Healthcheck via Atelier API `/status` + loopback HTTP.
- **CLAUDE.md** + `.claude/rules/deploy-chain.md` + `.claude/rules/docs-first.md` : MAJ paths + topologie.

## Quoi reste sur CM

- `/nvme/atelier/` (code source Atelier) — édité + `make deploy` inchangé.
- `code-server@romain.service` (port 9080, `code.mynetwk.biz`) — usage perso, non touché.
- Snapshot `/var/backups/cm-studio-snapshot-2026-05-27.tar.gz` (sources + user-data figés avant migration, ~860M).

## Snapshots / rollback

- **Snapshot CM (pre-migration)** : `/var/backups/cm-studio-snapshot-2026-05-27.tar.gz` (CM, 860M, sha256 disponible).
- **Snapshot Medion (post-migration)** : `/var/backups/medion-studio-postmigration-2026-05-27.tar.gz` (Medion, 680M, sha256).
- **Modifs apps non commitées** : stashées dans chaque repo, message `pre-migration medion 2026-05-27`. `git stash list` dans chaque `/var/lib/atelier/apps/<slug>/src/` pour les voir.
- **Procédure de rollback** : cf. `.claude/rules/deploy-chain.md` section Rollback.

## À faire dans session séparée — ✅ FAIT

Refactor du code Atelier pour nettoyer le drapeau `SourcesLocation` (per-app) qui est devenu non-sens depuis que toutes les apps tournent au même endroit. Voir le prompt prêt-à-coller en fin de ce document.

> ✅ Terminé : enum `SourcesLocation` + champ `sources_on` supprimés. Les fonctions SSH/rsync vers CloudMaster (`scaffold_on_cloudmaster`, `cleanup_cloudmaster_src`, `regen_context_on_cloudmaster`, `apply_rules_acl_remote`) éradiquées. Pipelines `build()` et `ship()` collapse en exec local par défaut, avec gating optionnel via env var `ATELIER_BUILD_HOST` (vide = local). Helper `bind_git_remote_on_cloudmaster` renommé `bind_git_remote_for_slug` + désormais wiré dans `AppCreate`. DTOs `App*` rapatriés dans `crates/atelier-api/src/mcp/dto.rs` (suite à leur suppression côté hr-ipc). `hr-dataverse-migrate` default `apps_root` migré vers `/var/lib/atelier/apps`.

### Prompt pour la session refactor

> **Contexte** : depuis le rapatriement Studio Atelier (2026-05-27), les sources des 6 apps vivent sur Medion à `/var/lib/atelier/apps/<slug>/src/`. Le code Atelier porte encore un drapeau per-app `sources_on: cloudmaster|medion` dans `crates/hr-apps/src/types.rs` (enum `SourcesLocation`). Ce drapeau est ignoré en pratique (commenté "deprecated") mais le code dans `crates/atelier-api/src/mcp/apps_ops.rs` continue de brancher sur sa valeur pour décider de faire un SSH/rsync vers CloudMaster (logique morte post-rapatriement).
>
> **But** : éradiquer le drapeau `sources_on` et le code associé. Le seul layout supporté est désormais "build local Medion à `ATELIER_APPS_SRC_ROOT/<slug>/src/`". Ajout d'un env var `ATELIER_BUILD_HOST` (vide = local, sinon `user@host` pour SSH) au cas où on veut un futur build-host distant, mais sans le multiplier en data per-app.
>
> **À faire** :
> 1. `crates/hr-apps/src/types.rs` : supprimer enum `SourcesLocation` + champ `sources_on` dans `struct Application` + assignation dans `Application::new`. Serde tolère les champs inconnus par défaut donc les vieux `apps.json` désérialisent sans erreur (le champ devient ignoré).
> 2. `crates/atelier-api/src/mcp/apps_ops.rs` :
>    - Supprimer constantes `BUILD_HOST`, `SSH_KEY`, `GIT_API_BASE` (sauf si encore utilisées pour le push initial du repo — vérifier).
>    - Supprimer fonctions `scaffold_on_cloudmaster`, `cleanup_cloudmaster_src`, `regen_context_on_cloudmaster` + tous leurs call sites.
>    - Remplacer toutes les branches `match app.sources_on { SourcesLocation::Medion => ..., SourcesLocation::CloudMaster => ... }` par la branche locale uniquement (anciennement `Medion`).
>    - Lire `ATELIER_BUILD_HOST` env var (default = "local"/vide). Si défini et non vide, faire SSH ; sinon exec local. Wrapper utility `exec_at_build_host(&self, cmd: &str) -> Result<Output>`.
>    - Supprimer `sources_location_to_str` + ses call sites.
> 3. `crates/atelier-api/src/mcp/scaffold.rs` : adapter aux nouvelles signatures.
> 4. `crates/hr-apps/src/context.rs` ligne 1611 : retirer le hardcode `/opt/homeroute/apps/...` dans `Application::src_dir()` si pertinent.
> 5. `crates/hr-dataverse-migrate/src/main.rs:42` : default `apps_root` → `/var/lib/atelier/apps`.
> 6. `apps.json` Medion : optionnel — nettoyer les champs `sources_on` à la main ou laisser (ignorés par serde).
> 7. Vérifier compile + lancer tests existants : `cargo check -p atelier`, `cargo test -p hr-apps`, `cargo test -p atelier-api`.
> 8. `make deploy` Atelier + tester MCP `app.build` sur 1 app via `mcp__atelier__app_build` ou via une route HTTP correspondante.
> 9. MAJ docs : `CLAUDE.md` + `docs/refonte/2026-05-27-studio-medion.md` (ce fichier) — marquer le TODO comme fait.
>
> **Estimation** : 1h-1h30 (lecture apps_ops.rs ~2400 lignes, refacto ~10 endroits, tests).
>
> **Garde-fous** :
> - Ne pas casser `Application::deserialize` sur les vieux `apps.json`.
> - Ne pas casser le scaffold de nouvelles apps : la branche locale (ancienne `Medion`) doit pouvoir scaffolder en remplaçant SSH par exec local.
> - Tester chaque MCP touché (`app.create`, `app.delete`, `app.build`, `app.regenerate_context`).
