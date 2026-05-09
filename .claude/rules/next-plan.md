# 🚧 Migration en attente — `hr-flowd` daemon multi-stack

> **Cutover Phase 9 terminé le 2026-05-09. La transformation hr-flowd reste explicitement reportée — à reprendre quand 9.5 cleanup sera décidé et l'écosystème stable.**

📄 Plan complet (copie locale dans le repo) : [/nvme/atelier/docs/plan-hr-flowd.md](/nvme/atelier/docs/plan-hr-flowd.md)
📄 Source originale : [/home/romain/.claude/plans/peaceful-spinning-mountain.md](/home/romain/.claude/plans/peaceful-spinning-mountain.md)

## Statut actuel (mis à jour 2026-05-09)

- **Phase 5 d'Atelier livrée en viewer read-only uniquement** (commit `2fb1056`).
  Routes `/api/apps/:slug/flows*` + `/api/flows/_stats` reproduites depuis
  homeroute, sans couplage au runtime `hr-flow` (uniquement `parse_flow_toml`,
  fonction pure).
- **Cutover Phase 9 terminé** (commits `5f980cb` + `719169e`) : apps migrées
  vers CloudMaster, hr-orchestrator stoppé sur Medion.
- **La transformation hr-flowd est explicitement reportée** par l'utilisateur.

## Quoi (résumé)

Transformer `hr-flow` (aujourd'hui lib Rust embeddable, donc inutilisable depuis
les 5 apps NextJS aptymus/calendar/forge/padel/www) en **daemon partagé
`hr-flowd`** accessible via callbacks HTTP par toutes les apps quelle que soit
leur stack (Rust ou NextJS). Plan en 7 phases :

1. Daemon `hr-flowd` (port 4002, HTTP /v1/runs|/replay|/definitions)
2. `RemoteEngine` côté hr-flow + helper `hr-flow-callback` (sous-router axum)
3. Mini-lib npm `@homeroute/flow-action` (handler factory NextJS)
4. Bascule Wallet `EmbeddedEngine` → `RemoteEngine` avec dual-mode flag
5a. Roll-out apps Rust (files / home / trader / myfrigo)
5b. Roll-out apps NextJS (aptymus / calendar / forge / padel / www)
6. Cleanup `EmbeddedEngine`
7. Scaffold automation (apps flow-ready au premier `make app-build`)

## Garde-fous pendant les évolutions intermédiaires

Pendant tout changement touchant à `hr-flow`/`hr-flow-macros`/Wallet :

- ❌ **Ne pas créer de nouveau couplage embedded.** Pas de `Arc<FlowEngine>` qui
  voyage dans les ApiState ou les builders d'apps en plus.
- ❌ **Ne pas refactorer hr-flow** d'une façon qui rendrait l'extraction du
  daemon plus douloureuse (ex : faire dépendre le moteur de types Atelier).
- ✅ **Garder `parse_flow_toml` comme seul point d'entrée** côté Atelier ; tout
  appel d'exécution doit passer par les routes HTTP existantes (qui pointeront
  vers le daemon une fois extrait).
- ✅ Les TOML restent dans `apps/{slug}/src/flows/`, les runs dans
  `apps/{slug}/runs/` — schéma stable, pas de changement de layout.

## Trigger de reprise

Quand le cutover Phase 9 est terminé (hr-orchestrator stoppé sur Medion, Atelier
seul propriétaire des apps), **annoncer dans la conversation** :

> "Le cutover est stable depuis N jours. On peut maintenant attaquer hr-flowd
> (peaceful-spinning-mountain.md, Phase 1)."
