# Phase 3 — Teardown infra flow

## État

- Statut : DONE
- Démarré : 2026-05-26
- Terminé : 2026-05-26
- Branche : eradication-flows

## Pré-requis

- [x] Les 6 apps refondues + déployées (Phase 2.1-2.6)
- [x] Smoke tests passent sur chaque app
- [x] Plus aucun appel `run_flow` / `engine.run` / `hr-flow` dans les 6 apps (vérifié par grep)

## 3.1 Daemon `hr-flowd` sur Medion — DONE

- [x] `sudo systemctl stop hr-flowd && sudo systemctl disable hr-flowd`
- [x] `sudo rm /etc/systemd/system/hr-flowd.service`
- [x] `sudo rm /opt/atelier/bin/atelier-flowd`
- [x] `sudo rm -rf /etc/atelier-flowd /opt/atelier/bin/sync-runs.sh`
- [x] `sudo rm -rf /var/lib/atelier/apps/*/runs`
- [x] `sudo systemctl daemon-reload`
- [x] `systemctl status hr-flowd` → "could not be found"
- [x] `pgrep -af hr-flowd` → vide
- [x] `atelier-sync-runs.timer` et `.service` n'existaient pas (skip)

## 3.2 Crates Atelier — DONE

- [x] `rm -rf crates/hr-flow{,-callback,-daemon,-macros}`
- [x] `Cargo.toml` workspace `members` purgé (4 lignes retirées)
- [x] `Cargo.toml` workspace `dependencies` purgé (4 lignes retirées)
- [x] `crates/atelier-api/Cargo.toml` : dep `hr-flow` retirée

## 3.3 Atelier API — DONE

- [x] `crates/atelier-api/src/routes/flows.rs` supprimé
- [x] `crates/atelier-api/src/clients/flowd.rs` supprimé
- [x] `clients/mod.rs` : `pub mod flowd;` + `pub use flowd::FlowdClient;` retirés
- [x] `lib.rs` : `.merge(routes::flows::router())` retiré
- [x] `routes/mod.rs` : `pub mod flows;` retiré
- [x] `routes/mcp.rs` : 6 match arms `"flow.*"` + 6 fonctions `tool_flow_*` + `flow_dirs_for` + définitions tools dans `tool_definitions()` → supprimés
- [x] `routes/apps.rs` : route `/{slug}/regenerate_flow_token` + fn `regenerate_flow_token` retirés
- [x] `routes/ws.rs` : commentaire obsolète "hr-flow task lifecycle" nettoyé
- [x] `mcp/apps_ops.rs` : bloc `?flows` auto-append dans `resolve_artefacts` retiré
- [x] `hr-apps/src/types.rs` : `flow_callback_url` + `flow_callback_token` marqués `#[serde(default, skip_serializing)]` pour tolérer les apps.json legacy sans les ré-écrire
- [x] `hr-apps/src/registry.rs` : auto-provisioning des champs flow dans `upsert` retiré ; `generate_flow_token` + `default_callback_url` retirés ; 2 tests flow_callback supprimés
- [x] `hr-apps/src/lib.rs` : `pub use registry::{default_callback_url, generate_flow_token, ...}` → `pub use registry::AppRegistry`
- [x] `hr-apps/src/context.rs` : `is_flows_eligible`, `render_flows_first_rule`, `render_flows_first_rule_rust/next`, `render_flow_build_skill` supprimés ; `flows-first.md` ajouté à `OBSOLETE_RULE_FILES` (nettoyage auto au prochain context refresh des apps) ; section "Côté flows hr-flow — connecteur dataverse" du `db.md` retirée

## 3.4 Frontend — DONE

- [x] `web/src/pages/FlowsStats.jsx` supprimé
- [x] `web/src/components/flows/` (FlowsTab.jsx + FlowsStatsView.jsx) supprimé
- [x] `web/src/App.jsx` : import + route `/flows-stats` retirés
- [x] `web/src/pages/Studio.jsx` : import FlowsTab, tab "flows", `Workflow` icon, rendering conditionnel retirés
- [x] `web/src/components/Sidebar.jsx` : entrée Sidebar "Flow Stats" + import `Workflow` retirés
- [x] `npm run build` vert (web/dist 8s)

## 3.5 Makefile + scripts — DONE

- [x] Cibles `flowd`, `deploy-flowd`, `logs-flowd` supprimées de `Makefile`
- [x] `scripts/deploy-flowd.sh` supprimé
- [x] `scripts/sync-runs.sh` supprimé
- [x] Aide `help` mise à jour

## 3.6 Documentation — DONE

- [x] `CLAUDE.md` : section "Plan suivant hr-flowd" remplacée par "Système de flux — supprimé (2026-05-26)"
- [x] `CLAUDE.md` : "Flows : moteur d'orchestration TOML" retiré de la liste des concerns
- [x] `CLAUDE.md` : `flow.*` retiré de la liste des MCP tools
- [x] `CLAUDE.md` : 10 crates → 6 crates (hr-flow* retirées)
- [x] `docs/plan-hr-flowd.md` → archivé en `docs/refonte/archive-plan-hr-flowd-OBSOLETE.md`
- [x] `.claude/rules/next-plan.md` supprimé (référençait hr-flowd)

## 3.7 Env vars — DONE

- [x] `/opt/atelier/.env` sur Medion : `ATELIER_FLOW_TOKEN` retiré (`HR_FLOWD_URL` + `HR_FLOWD_TIMEOUT_MS` n'existaient pas)

## Build + Deploy — DONE

- [x] `cargo build --release -p atelier` vert (57s, 2 dead_code warnings sur fonctions Atelier-internes non liées au flow)
- [x] `cd web && npm run build` vert (8s)
- [x] `make deploy` succès (rsync binaire + web/dist + restart atelier.service)
- [x] Healthcheck `http://10.0.0.254:4100/api/health` → 200 en 2s

## Vérification end-to-end — DONE

- [x] `/api/health` → 200
- [x] `/api/flows/_stats` → 404 (preuve route supprimée)
- [x] `/api/logs?limit=2` → 200 + JSON
- [x] Atelier service actif sur Medion
- [x] Plus aucun process `hr-flowd`/`atelier-flowd`

## Reste : crates partagées homeroute

Atelier dépend toujours de `hr-common`, `hr-ipc`, `hr-docs` via path-deps vers `/nvme/homeroute/crates/shared/`. **Non concerné** par la refonte flow (ces crates n'ont jamais embarqué de logique flow).

## Notes

- L'invariant des fichiers `apps.json` legacy est préservé : les champs `flow_callback_url`/`flow_callback_token` sont tolérés par `serde(default)` mais ne sont plus sérialisés. Au prochain restart d'Atelier, les apps.json reseront propres (skip_serializing évite la ré-écriture des champs).
- Les commits sont à faire post-Phase 4 ou immédiatement, au choix.
