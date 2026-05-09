# Plan — `hr-flowd` daemon : hr-flow comme plateforme partagée multi-stack

## Context

**Intent long terme** : les flux sont **le** mécanisme général d'orchestration, transformation, transferts et connexions aux sources externes pour toutes les apps HomeRoute, sans distinction de stack. Toute app HomeRoute doit pouvoir, au scaffold, exécuter des flux dès le premier `make app-build`.

**État actuel** : `hr-flow` v1 est une **lib Rust embeddable** — chaque app cible link la crate, registre ses connecteurs/actions custom, lance son `FlowEngine` au boot. Wallet (Rust+Vite) est le pilote validé, 4 autres apps Rust (files/home/trader/myfrigo) sont en cours de migration. Les 5 apps **NextJS** (`aptymus`, `calendar`, `forge`, `padel`, `www`) sont hors du modèle parce qu'on ne peut pas embarquer du Rust dans Node.

**Problème** : ce modèle embedded scinde les apps en deux castes (Rust dedans, NextJS dehors), ce qui contredit l'intent général. Les NextJS représentent à ce jour la moitié des apps et porte des cas réels d'orchestration (catalogue, calendrier, forge, etc.).

**Objectif** : transformer `hr-flow` en **daemon partagé** (`hr-flowd`) accessible via HTTP, avec :
- moteur Rust unique (le code de `hr-flow` actuel, mutualisé)
- connecteurs managés embarqués (`http`, `dataverse`, `homeroute`) — partagés entre toutes les apps
- code custom (actions + connecteurs) écrit dans la stack native de chaque app et invoqué par **callback HTTP** (pas de cross-compilation, pas de port TS du moteur)
- TOML toujours dans `apps/{slug}/src/flows/*.toml` (versionnés avec le code)
- runs toujours dans `apps/{slug}/runs/*.json`
- mode embedded de v1 conservé en parallèle pendant la transition (dual-mode flag-controlled, pas de big-bang)

## Architecture cible

### Composant `hr-flowd`

Nouveau service systemd, hébergé dans le crate **`hr-flow-daemon`** (`crates/orchestrator/hr-flow-daemon/`). Préfère un service séparé plutôt que greffé à `hr-orchestrator` : isolation des crashs (un connecteur tiers qui boucle ne tue pas l'orchestration centrale), redéploiement indépendant.

- Port HTTP local : **4002** (loopback uniquement, pas exposé via hr-edge)
- Boot : lit `apps.json` pour la liste des apps, scanne `apps/*/src/flows/*.toml`, hot-reload sur signal SIGHUP ou via tool MCP
- Pour chaque app, charge le **callback config** : `flow_callback_url` (ex. `http://localhost:3005`) + `flow_callback_token` (HMAC shared) — soit depuis `apps.json`, soit depuis le `.env` de l'app
- Persistance : le `JsonRunStore` actuel reste (un dossier `runs/` par app)
- Auth interne : tout call vers `/run`/`/replay` doit présenter un token HomeRoute (réutilise le pattern Bearer du MCP HTTP sur 4001)

### Surface HTTP du daemon

Routes minimales :

| Méthode | Route | Usage |
|---|---|---|
| `POST` | `/v1/runs` | Body `{ slug, flow_name, input }` → exécute, retourne `RunResult` |
| `POST` | `/v1/runs/{run_id}/replay` | Rejoue un run existant |
| `GET`  | `/v1/runs/{run_id}` | Détail (header + arbre lite) |
| `GET`  | `/v1/runs?slug=&flow_name=&limit=` | Liste filtrée |
| `GET`  | `/v1/definitions?slug=` | Liste des flows définis |
| `POST` | `/v1/_admin/reload?slug=` | Hot-reload des TOML pour un slug |

L'API REST publique de homeroute (`/api/apps/{slug}/flows/*` et `/api/flows/_stats`) est inchangée côté front : elle delegue désormais au daemon plutôt que de scanner directement le filesystem. Garde les mêmes URLs, swap interne.

### Callback custom code (le pivot du modèle multi-stack)

Quand un step a `kind = "action"` ou `kind = "connector"` avec un connecteur **custom** (pas dans la liste managed), le daemon :

1. Trouve la callback URL + token de l'app cible (`apps.json` ou registry)
2. Appelle `POST {callback_url}/_flow/action/{name}` (ou `/connector/{name}/{op}`) avec :
   ```json
   {
     "run_id": "...",
     "step_id": "...",
     "input": {...},
     "params": {...}
   }
   ```
3. Headers : `Authorization: Bearer <token>` + `X-HomeRoute-Flow: 1`
4. Reçoit `{ "output": ... }` ou `{ "error": "..." }`, intègre dans le RunRecord

**Timeout** : 30s par défaut, configurable par step (`step.timeout_ms`). Erreur de callback = step failed avec `error.kind = "callback_timeout"` ou `"callback_5xx"`.

### Compilation des actions custom — principe clé

**Le daemon ne compile rien**. Il orchestre, point. La compilation reste **au niveau de l'app**, dans son toolchain natif :

| Stack | Comment l'action custom est exposée | Toolchain |
|---|---|---|
| NextJS / Node | handler TS dans une route Next | `next build` |
| Rust (axum) | sous-router monté via `hr-flow-callback::router(...)` ; macro `#[flow_action]` étendue génère le wiring HTTP | `cargo build` |
| Autres (Python, Go, …) | endpoint HTTP écrit à la main, ~20 LOC | toolchain native |

Le contrat est **uniforme** : `POST /_flow/action/{name}` (et `/_flow/connector/{name}/{op}`) qui valide le bearer token, lit `{ run_id, step_id, input, params }` et retourne `{ output }` ou `{ error }`. Tout langage de stack web peut le servir.

### Conventions par stack (côté app)

#### NextJS / Node

L'app expose une route catchall `app/api/_flow/[type]/[name]/route.ts` :

```ts
import { handleFlowCallback } from '@homeroute/flow-action';
import { computeScore, enrichProfile } from '@/lib/flow-actions';

export const POST = handleFlowCallback({
  actions: { compute_score: computeScore, enrich_profile: enrichProfile },
  connectors: { /* connecteurs custom optionnels */ },
});
```

`@homeroute/flow-action` est un mini-package npm (publié dans `web/packages/flow-action/`, ou un simple module copié sous `apps/<slug>/src/lib/flow-callback.ts` au scaffold) qui :
- valide le token HMAC
- mesure la durée
- attrape les exceptions et les sérialise au format `{ error: string }`
- expose `handleFlowCallback({ actions, connectors })` qui retourne un `(req: Request) => Response`

**Compilation** : zero ajout — c'est du TS comme le reste de l'app, embarqué dans `next build`. Aucune toolchain Rust requise pour les apps NextJS.

#### Rust

L'app a déjà un router axum. On ajoute un sous-router via la nouvelle crate helper `hr-flow-callback` :

```rust
// dans main.rs
let flow_router = hr_flow_callback::router(app_state.clone())
    .with_action(mount_compute_risk_score)
    .with_action(mount_aggregate_month_stats)
    // …
    .with_connector("openrouter", Arc::new(OpenRouterConnector::from_env()?));

let app = Router::new()
    .merge(flow_router) // expose /_flow/action/* et /_flow/connector/*/*
    .merge(business_router);
```

**La macro `#[flow_action]` est étendue, pas remplacée — backward-compatible.** Aujourd'hui elle génère `register_<fn>(builder)` ; après la phase 2 elle génère **aussi** `mount_<fn>(router)`. Le code annoté `#[flow_action]` ne change pas : la fonction Rust qui calcule le score, l'agrégat, etc. garde sa signature `async fn(input: Value) -> FlowResult<Value>`. Seul le wiring (qui appelle quoi à boot) change. Wallet peut donc tourner embedded ET callback en parallèle pour vérifier la parité avant de couper l'embedded.

#### Python / autres

Mêmes contrats : un endpoint `POST /_flow/action/{name}` qui valide le token, lit le body, retourne `{ output }` ou `{ error }`. Pas de dépendance HomeRoute imposée — un agent peut écrire le handler à la main si l'app n'utilise pas de framework standard.

### Modes coexistants pendant la transition

`hr-flow` (la crate) supporte **deux backends** :
- `EmbeddedEngine` : le `FlowEngine` actuel, in-process. Conservé pour Wallet le temps de la phase de transition.
- `RemoteEngine` : un client HTTP qui pose les requêtes vers `http://localhost:4002/v1/runs`. C'est ce que Wallet utilise quand on bascule.

L'app choisit son backend via la variable d'env **`HR_FLOW_BACKEND=embedded|remote`** lue dans `main.rs`. Pour les NextJS, pas de question : elles ne consomment hr-flow que via `RemoteEngine` (ou un équivalent direct fetch, plus simple).

Avantage : on déploie le daemon, on convertit les apps une par une, on n'a jamais à arrêter Wallet en mode dégradé. Si une régression apparaît en `remote`, l'agent toggle la variable d'env et restart — retour `embedded` instantané, sans recompilation.

### Coût de bascule pour les apps déjà migrées en mode embedded

Wallet (10 flux migrés, ~5 actions custom, 1 connecteur custom `openrouter`) ainsi que les autres apps Rust qui finiraient leur migration avant la phase 4 sont préservées. **Aucune refonte requise.**

**Ce qui ne bouge pas (le travail capitalisé)** :
- Les **TOML** des flux : zéro changement, même format, même emplacement (`server/src/flows/*.toml`)
- Le **corps** des actions Rust annotées `#[flow_action]` (`compute_risk_score`, `aggregate_month_stats`, etc.) : intact
- Le **corps** des connecteurs custom (`OpenRouterConnector::request`, etc.) : intact
- La doctrine, le naming, la rule `flows-first.md`, le skill `flow-build` : tout pareil

**Ce qui change (plumbing minimal, ~50 LOC par app)** :
- `server/src/flows/mod.rs::build_engine()` : remplacé par `register_callbacks()` qui appelle les `mount_<fn>(router)` générés par la macro étendue. ~30 LOC.
- `main.rs` : la ligne qui stockait `Arc<FlowEngine>` dans `AppState` disparaît, remplacée par un `merge` du callback router. ~5 LOC.
- `server/src/flows/internal_routes.rs` (les routes `_internal/flows/run` et `replay` ajoutées pour le MCP du moteur embedded) : **supprimées**. C'est le daemon qui répond directement aux calls MCP. De la dette qui s'efface.
- `Cargo.toml` : la dep `hr-flow` reste pour les types (`FlowError`, `FlowResult`, `Value` helpers), on ajoute `hr-flow-callback`.

**Filet de sécurité** : phase 4 prévoit le dual-mode flag-controlled (`HR_FLOW_BACKEND`) avec **7 jours de cohabitation en prod** avant le cleanup de l'embedded. Si un comportement diverge entre embedded et remote, retour `embedded` immédiat.

## Fichiers critiques

**Nouveaux** :
- [crates/orchestrator/hr-flow-daemon/](crates/orchestrator/hr-flow-daemon/) — binaire + service systemd ; reuse direct du moteur de `hr-flow` via dep crate
- [crates/orchestrator/hr-flow-callback/](crates/orchestrator/hr-flow-callback/) — helper Rust pour exposer les actions/connecteurs custom comme routes axum (consommé par les apps Rust en mode callback)
- [web/packages/flow-action/](web/packages/flow-action/) — mini-lib TS pour les apps NextJS (~100 LOC)
- [systemd/hr-flowd.service](systemd/hr-flowd.service) — service unit
- Templates de scaffold sous [crates/orchestrator/hr-apps/templates/](crates/orchestrator/hr-apps/templates/) pour les apps NextJS et Rust : route `_flow/*` boilerplate + `.env` `HR_FLOW_TOKEN`

**Modifiés** :
- [crates/orchestrator/hr-flow/src/lib.rs](crates/orchestrator/hr-flow/src/lib.rs) — extraction du moteur dans un module réutilisable + ajout du `RemoteEngine` (client HTTP)
- [crates/orchestrator/hr-flow/src/engine.rs](crates/orchestrator/hr-flow/src/engine.rs) — split en trait `Engine` + impl `EmbeddedEngine` + impl `RemoteEngine`
- [crates/orchestrator/hr-flow/src/connector.rs](crates/orchestrator/hr-flow/src/connector.rs) — ajout d'un `RemoteConnector` qui POST vers le callback URL d'une app
- [crates/orchestrator/hr-flow-macros/src/lib.rs](crates/orchestrator/hr-flow-macros/src/lib.rs) — `#[flow_action]` génère AUSSI `mount_<fn>` (en plus de `register_<fn>`)
- [crates/api/hr-api/src/routes/flows.rs](crates/api/hr-api/src/routes/flows.rs) — bascule des handlers REST vers un client HTTP du daemon plutôt que `std::fs::read_dir`. La logique `compute_stats` migre côté daemon (ou reste côté hr-api en lisant les mêmes runs/, à valider à l'implémentation)
- [crates/orchestrator/hr-orchestrator/src/mcp.rs](crates/orchestrator/hr-orchestrator/src/mcp.rs) — les tools `flow.run` / `flow.replay` deviennent des appels au daemon
- [crates/orchestrator/hr-apps/src/types.rs](crates/orchestrator/hr-apps/src/types.rs) — ajout de `flow_callback_url` et `flow_callback_token` dans `Application`
- [crates/orchestrator/hr-apps/src/context.rs](crates/orchestrator/hr-apps/src/context.rs) — la rule `flows-first.md` étendue aussi aux NextJS, le skill `flow-build` documente le pattern callback
- [Makefile](Makefile) — cible `make deploy-flowd`

## Patterns / utilities à réutiliser

- **JsonRunStore** ([crates/orchestrator/hr-flow/src/persist.rs](crates/orchestrator/hr-flow/src/persist.rs)) — déplacé tel quel dans le daemon, paramétré par slug
- **`compute_stats`** ([crates/api/hr-api/src/routes/flows.rs](crates/api/hr-api/src/routes/flows.rs)) — réutilisé côté daemon ou hr-api selon le choix
- **MCP HTTP auth** ([crates/orchestrator/hr-orchestrator/src/mcp.rs](crates/orchestrator/hr-orchestrator/src/mcp.rs)) — pattern Bearer pour authentifier les calls daemon ↔ orchestrator
- **`#[flow_action]`** macro existante — étendue, pas réécrite
- **systemd service template** ([systemd/hr-orchestrator.service](systemd/hr-orchestrator.service)) — modèle pour `hr-flowd.service`
- **port-registry** ([/opt/homeroute/data/port-registry.json](/opt/homeroute/data/port-registry.json)) — réserve 4002 pour `hr-flowd`

## Phasing

### Phase 1 — Daemon `hr-flowd` (sans cassure)

- Crate `hr-flow-daemon` qui réutilise `hr-flow::engine`, expose les routes HTTP `/v1/runs` et al.
- Hot-reload des TOML par slug
- Auth Bearer sur toutes les routes
- Persistance partagée avec hr-flow embedded (même format `runs/*.json`)
- Service systemd + Makefile cible
- **Objectif** : le daemon tourne mais personne ne l'appelle encore. Wallet reste en embedded. Vérification : `curl localhost:4002/v1/definitions?slug=wallet` retourne la même liste que `flow.list_definitions(wallet)` actuel.

### Phase 2 — `RemoteEngine` côté hr-flow + helper `hr-flow-callback`

- Trait `Engine` extrait, impls `EmbeddedEngine` (legacy) + `RemoteEngine` (client HTTP daemon)
- Crate `hr-flow-callback` : helper axum pour Rust, monte `/_flow/action/{name}` et `/_flow/connector/{name}/{op}` à partir des fonctions `#[flow_action]`
- Macro `#[flow_action]` étendue pour générer `mount_<fn>`
- **Objectif** : Wallet peut être patchée en mode `RemoteEngine` (1 ligne dans `main.rs`). Smoke-test : un flux Wallet exécuté via `RemoteEngine` produit un run identique à l'embedded.

### Phase 3 — Callback NextJS + scaffold

- Mini-lib TS `web/packages/flow-action/` (handler factory + token validation)
- Templates de scaffold per-stack dans hr-apps : route Next + handlers `app-info`/`flow-build` adaptés
- Update du skill `flow-build` pour documenter le pattern callback (Rust + Node)
- Update de la rule `flows-first.md` pour inclure les NextJS dans le scope éligible
- Ajout de `flow_callback_url` + `flow_callback_token` dans `apps.json`, génération auto au scaffold + injection dans `.env` de l'app
- **Objectif** : on peut prendre une app NextJS quelconque, la brancher au daemon, exécuter un flux trivial qui appelle un handler TS custom. Smoke-test : `flow.run(www, hello_flow, {name: "test"})` qui appelle une action `greet` en TS et retourne `"Hello test"`.

### Phase 4 — Bascule Wallet en `RemoteEngine`

- `Cargo.toml` Wallet : ajout dep `hr-flow-callback`
- `flows/mod.rs` Wallet : `build_engine()` → `register_callbacks(state)` qui retourne le sous-router axum
- `main.rs` Wallet : drop du `Arc<FlowEngine>` dans `AppState`, merge du sous-router callback
- Suppression de `flows/internal_routes.rs` (le daemon répond aux calls MCP)
- Variable d'env `HR_FLOW_BACKEND=remote` posée dans le `.env` Wallet
- **7 jours de cohabitation en prod** : Wallet en `remote`, l'agent peut toggle vers `embedded` instantanément (restart sans recompil) si une régression apparaît
- Vérification quotidienne via `/flows-stats` : KPIs Wallet identiques (à 5% près sur duration_ms à cause du hop HTTP local)
- Quand stable : retrait du chemin `EmbeddedEngine` de `hr-flow`, suppression de la lecture de `HR_FLOW_BACKEND` dans Wallet

### Phase 5a — Roll-out apps Rust existantes (files / home / trader / myfrigo)

Ces apps n'ont pas encore migré (le rollout `flows-first` initial leur a poussé la rule + skill, mais aucune n'a intégré hr-flow). On saute directement le mode embedded — elles partent en callback mode dès le départ.

Pour chaque app, l'agent app fait :
1. Audit des routes/handlers chaînant ≥ 2 étapes (cf. prompt de migration ci-dessous), priorisation
2. Ajout dep `hr-flow-callback` dans `server/Cargo.toml`
3. Création `server/src/flows/mod.rs` avec `register_callbacks(state) -> Router` (pas de `FlowEngineBuilder`, pas de `JsonRunStore` côté app)
4. `main.rs` : merge du sous-router callback ; pas de `Arc<FlowEngine>` dans `AppState`
5. Génération du `flow_callback_token` (au scaffold ou via tool MCP `app.regenerate_flow_token`), posé dans `.env`
6. Enregistrement de l'app auprès du daemon (`apps.json` mis à jour avec `flow_callback_url` + `flow_callback_token` ; reload daemon via `_admin/reload`)
7. Ajout `flows/` au `build_artefact` (sinon les TOML ne montent pas en prod)
8. Premier flux migré + test via `mcp__homeroute__flow.run`
9. Migration par lot des autres routes orchestrées

### Phase 5b — Roll-out apps NextJS existantes (aptymus / calendar / forge / padel / www)

Première fois que ces apps touchent à hr-flow. Pas de Rust à introduire.

Pour chaque app, l'agent app fait :
1. Audit identique (routes orchestrant ≥ 2 étapes)
2. Ajout du package npm `@homeroute/flow-action` (ou copie du module au scaffold)
3. Création de `app/api/_flow/[type]/[name]/route.ts` (catchall) qui appelle `handleFlowCallback({ actions, connectors })`
4. Génération du token + ajout dans `.env.local` (NextJS)
5. Enregistrement auprès du daemon
6. Création du dossier `flows/` au niveau de la racine app
7. Premier flux migré + test
8. Migration par lot

Pour les apps NextJS, **la rule `flows-first.md` doit être étendue à leur scope** (aujourd'hui gated sur stack Rust). Modif dans `hr-apps::context::is_flows_eligible` à faire en début de phase 5b.

### Phase 6 — Cleanup `EmbeddedEngine`

Pré-requis : 100% des apps tournent en mode callback (Wallet incluse, en `remote` depuis ≥ 7 jours).

- Suppression de `EmbeddedEngine` dans `hr-flow/src/engine.rs`
- `hr-flow` (la crate) devient strictement un client + types partagés (`FlowDef`, `StepDef`, `RunDoc`, `RemoteEngine`)
- Suppression de `flow_callback_token` du `.env` Wallet (garde `HR_FLOW_BACKEND=remote` ou retire la variable si plus de toggle)
- `register_<fn>` retiré de la macro `#[flow_action]` (seul `mount_<fn>` reste)
- Documentation et skill `flow-build` mis à jour pour ne plus mentionner le mode embedded

### Phase 7 — Scaffold automation pour les nouvelles apps

À ce stade le daemon est steady-state. On automatise la création d'apps pour qu'elles soient flow-ready dès leur premier `make app-build`.

- Templates de scaffold per-stack étendus dans [crates/orchestrator/hr-apps/templates/](crates/orchestrator/hr-apps/templates/) :
  - **NextJS** : `app/api/_flow/[type]/[name]/route.ts` + import de `@homeroute/flow-action` + un placeholder `actions = {}` à remplir
  - **Rust (axum, axum-vite)** : `server/src/flows/mod.rs` avec `register_callbacks(state) -> Router` + import de `hr-flow-callback` + sample `#[flow_action]` commenté
  - **Tous** : dossier `flows/` créé avec un `hello.toml` d'exemple (1 step `compose` qui retourne `{ greeting: "Hello {{ input.name }}" }`)
- `POST /api/apps` (création app) :
  - génère un `flow_callback_token` cryptographiquement aléatoire (32 bytes)
  - le pose dans `apps.json` ET dans le `.env` de l'app
  - notifie le daemon via `POST localhost:4002/v1/_admin/reload?slug=<new>`
- Mise à jour de `hr-apps::context::generate_for_app` pour inclure les fichiers callback dans la génération initiale
- Mise à jour de la rule `flows-first.md` pour retirer la mention "État de la migration" (devenue inutile une fois steady-state)

Vérif scaffold : créer une app test `_test-scaffold-flows` en NextJS → vérifier que `hello.toml` tourne immédiatement via `flow.run(_test-scaffold-flows, hello, {name: "test"})` sans aucune intervention manuelle.

## Vérification end-to-end

**Phase 1** :
- `systemctl status hr-flowd` → active
- `curl -s -H "Authorization: Bearer $TOKEN" http://localhost:4002/v1/definitions?slug=wallet | jq` → liste des 10 flows Wallet
- `curl -s -H "Authorization: Bearer $TOKEN" http://localhost:4002/v1/runs?slug=wallet&limit=3 | jq` → 3 runs récents avec mêmes IDs que ce qu'on voit côté Studio

**Phase 2** :
- Wallet patchée pour `RemoteEngine` en local (sur CloudMaster, pas prod) → tous les tests d'intégration Wallet passent
- Une action custom Wallet (`compute_risk_score`) est invoquée par le daemon via `POST http://wallet:3009/_flow/action/compute_risk_score` → run identique à l'embedded (même output, même duration_ms à 5% près)

**Phase 3** :
- `apps/_test-flow-www/` créée (NextJS) avec un flux `hello.toml` (1 step `kind=action` action=greet) et un handler TS minimal
- `flow.run(_test-flow-www, hello, {name: "test"})` → status=success, output={"greeting": "Hello test"}, run visible dans Studio sous le slug `_test-flow-www`
- Cleanup : suppression de l'app de test après validation

**Phase 4** :
- Sur Medion, `journalctl -u wallet --since '1h ago' | grep "hr-flow"` → uniquement des logs `RemoteEngine`, plus aucun `EmbeddedEngine`
- Studio Wallet → tous les runs récents ont la même structure visuelle, durations cohérentes
- Stats `/flows-stats` → KPIs Wallet identiques à avant bascule (à 5% près sur duration_ms à cause du hop HTTP)

**Phase 5a (Rust apps)** :
- `curl localhost:4002/v1/definitions?slug=files` (idem home/trader/myfrigo) → liste non vide après leur premier flux
- `mcp__homeroute__flow.run(slug=files, name=<premier-flux>, input=...)` → status=success
- `/flows-stats` page globale → les 4 apps Rust apparaissent dans le breakdown `per_app`
- `grep -r "FlowEngineBuilder::new()" /opt/homeroute/apps/{files,home,trader,myfrigo}/` → 0 occurrence (toutes en callback direct, jamais embedded)

**Phase 5b (NextJS apps)** :
- Pour chaque NextJS migrée : `curl http://localhost:<port>/api/_flow/action/<name> -H "Authorization: Bearer <token>" -d '{"input": {...}}'` → réponse `{ output: ... }` valide
- `mcp__homeroute__flow.run(slug=www, name=<flux>, input=...)` → run visible dans Studio Flows pour `www`
- `grep -r "@homeroute/flow-action" /opt/homeroute/apps/{aptymus,calendar,forge,padel,www}/` → toutes les apps NextJS ont la dep
- Page `/flows-stats` globale : les 5 NextJS apparaissent dans `per_app`, le breakdown `per_connector` montre les usages cross-stack

**Phase 6** :
- `grep -r "FlowEngineBuilder::new()" /opt/homeroute/apps/` → 0 occurrence partout
- `cargo build -p hr-flow` → la crate compile sans `EmbeddedEngine`
- `systemctl status hr-flowd` → uptime > 7 jours, aucun crash
- Toutes les apps tournent en `HR_FLOW_BACKEND=remote` (ou la variable a disparu)

**Phase 7 (scaffold automation)** :
- Créer une app test `_test-scaffold-flows` via `POST /api/apps` (stack NextJS)
- Vérifier qu'au premier `make app-build` :
  - `apps/_test-scaffold-flows/.env` contient `HR_FLOW_TOKEN`
  - `apps/_test-scaffold-flows/flows/hello.toml` existe
  - `apps/_test-scaffold-flows/app/api/_flow/[type]/[name]/route.ts` existe
- `mcp__homeroute__flow.run(slug=_test-scaffold-flows, name=hello, input={"name": "test"})` → `{ greeting: "Hello test" }` sans aucune intervention
- Cleanup : `DELETE /api/apps/_test-scaffold-flows`
- Refaire le test pour stack Rust+Vite et Rust pur

## Risques + mitigations

- **Crash daemon = arrêt total** des flux. Mitigation : le service systemd a `Restart=always`, un health-check toutes les 10s côté hr-orchestrator (failsafe alerte). À long terme : envisager 2 instances + load balancer si on monte en charge.
- **Latence callback HTTP** : 1ms loopback × steps custom. Pas un problème pour les profils HomeRoute (10–500ms par step en moyenne). Si un cas pathologique apparaît (`for_each` × N steps custom), le daemon supportera un cache local pour les actions pures pures dans une itération future.
- **Sécurité tokens** : token shared par app, posé dans `.env` (mode 0600, owner romain:hr-studio). Rotation au scaffold uniquement (un changement de token nécessite restart app + reload daemon).
- **Schéma des `RunDoc`** : aujourd'hui figé par hr-flow. Si on évolue, le daemon doit savoir lire les runs anciens (rétro-compat). À documenter.
- **Migration Wallet** : risque #1 du projet (10 flux en prod, casser un seul = visibilité utilisateur). Mitigé par : (a) ~50 LOC de plumbing seulement à toucher, (b) dual-mode `HR_FLOW_BACKEND=embedded|remote` flag-controlled (toggle sans recompil), (c) 7 jours de cohabitation prod avant cleanup, (d) les TOML et le corps des actions/connecteurs ne changent pas — la surface fonctionnelle reste identique.

## Procédure de mise à jour des apps existantes

Cette section donne les **prompts de migration** qu'on collera dans la conversation de l'agent de chaque app pour démarrer sa migration. Chaque prompt est self-contained.

### Prompt pour Wallet (phase 4 — bascule embedded → callback)

```
Bascule Wallet du mode embedded vers le mode callback (daemon hr-flowd).

État actuel : Wallet utilise hr-flow comme lib embedded — `FlowEngineBuilder::new()` dans `flows/mod.rs::build_engine()`, `Arc<FlowEngine>` dans `AppState`, routes `/api/_internal/flows/run` et `/replay` dans `flows/internal_routes.rs`.

État cible : Wallet expose `_flow/action/*` et `_flow/connector/*/*`, le daemon orchestre.

Étapes (prends ton temps, vérifie après chaque) :

1. `Cargo.toml` : ajoute `hr-flow-callback = { path = "../../../../crates/orchestrator/hr-flow-callback" }`. Garde la dep `hr-flow` (les types FlowError/FlowResult sont encore utilisés dans les actions).

2. `flows/mod.rs` : remplace `pub fn build_engine() -> FlowEngine` par `pub fn register_callbacks(state: AppState) -> Router<AppState>`. Le corps liste les `mount_<fn>` (générés par `#[flow_action]` étendu) et les connecteurs custom via `hr_flow_callback::router(state).with_action(...).with_connector("openrouter", Arc::new(OpenRouterConnector::from_env()?))`.

3. `main.rs` : retire le `Arc<FlowEngine>` de `AppState`. Merge le sous-router : `let app = Router::new().merge(flows::register_callbacks(state.clone())).merge(business_router)...`. Pose `HR_FLOW_BACKEND=remote` et `HR_FLOW_TOKEN=<token-from-env>` dans le `.env` (le token est déjà dans `apps.json`, lis-le).

4. Supprime entièrement `flows/internal_routes.rs` ainsi que sa réf dans `flows/mod.rs`. Le daemon répond aux calls MCP désormais.

5. Build, déploie via `make app-build SLUG=wallet`. Vérifie via `mcp__homeroute__flow.run(name="score_transaction", input={...})` que l'output et l'arbre de steps sont identiques à ceux observés en mode embedded (les `run_id` récents sont visibles dans Studio Flows).

6. Le daemon doit voir Wallet : `curl localhost:4002/v1/definitions?slug=wallet` retourne les 10 flows.

7. Surveille `/flows-stats` filtré sur Wallet pendant 7 jours. Les KPIs (total runs, success rate, avg duration, total bytes) doivent rester dans une variation de ±5% par rapport à la semaine précédente.

8. Au bout des 7 jours, si tout est vert, j'enchaîne avec la phase 6 (cleanup embedded). Si tu vois une régression, toggle `HR_FLOW_BACKEND=embedded` et restart Wallet (sans recompil) — retour instantané au mode in-process. Tu me remontes le rapport plateforme structuré (cf. règle `flows-first.md`).
```

### Prompt pour les apps Rust en attente (files / home / trader / myfrigo) — phase 5a

```
Migration vers hr-flow (mode callback / daemon).

Plateforme : hr-flow tourne maintenant comme daemon partagé (`hr-flowd` sur localhost:4002). Wallet a validé le pilote, plus aucune app n'embarque le moteur. Tu vas brancher cette app au daemon en exposant un sous-router `_flow/*`.

Avant tout : lis `.claude/rules/flows-first.md` (always-on) en entier, charge le skill `flow-build`. Tu as les sections État de la migration, ⛔ Escalade plateforme, doctrine, naming, format TOML.

Procède en 4 étapes, dans l'ordre :

1. Audit. Repère dans `server/src/routes/` toutes les routes/handlers qui chaînent ≥ 2 étapes (call DB + transformation, call externe + écriture, boucle/condition portant sur des données métier). Renvoie-moi la liste classée par priorité avec une phrase métier par route.

2. Intégration. Une fois la liste validée :
   - `Cargo.toml` : ajoute `hr-flow-callback` (et `hr-flow` pour les types). Pas de `FlowEngineBuilder` à monter — on est en callback mode, pas embedded.
   - Crée `server/src/flows/mod.rs` avec `pub fn register_callbacks(state: AppState) -> Router<AppState>` qui appelle `hr_flow_callback::router(state)` et y greffe tes futurs `mount_<fn>` et connecteurs custom.
   - `main.rs` : merge du sous-router callback dans le router axum principal.
   - Crée le dossier `flows/` à la racine `server/src/flows/` (relatif au cwd du process).
   - Ajoute `flows/` au `build_artefact` de l'app (sinon les TOML ne montent pas en prod).
   - Le `flow_callback_token` est généré par `apps.json` ; vérifie qu'il est dans ton `.env` (sinon : `mcp__homeroute__app.regenerate_flow_token(slug=<ton-slug>)`).
   - Build : `make app-build SLUG=<ton-slug>`.

3. Migration par lot. Prends 1 ou 2 routes simples, transforme-les en flux TOML sous `server/src/flows/*.toml` (format plat, parent/parent_branch). Le handler garde un wrapper mince qui appelle `flow.run` via `RemoteEngine` (cf. skill flow-build pour le pattern). Build, déploie, teste via MCP `flow.run`. Lot suivant.

4. Escalade plateforme : si bug hr-flow ou hr-flow-callback, STOP, rapport structuré, attend le correctif. Pas de workaround.

Démarre par l'étape 1 : l'audit.
```

### Prompt pour les apps NextJS (aptymus / calendar / forge / padel / www) — phase 5b

```
Migration vers hr-flow (mode callback / daemon, stack NextJS).

Plateforme : hr-flow tourne comme daemon partagé. Cette app n'a jamais touché aux flux jusqu'ici — c'est la première fois. Pas de Rust à introduire.

Avant tout : lis `.claude/rules/flows-first.md` (poussée à toutes les apps depuis phase 5b) et charge le skill `flow-build` (variante TS / NextJS).

Procède en 4 étapes :

1. Audit. Repère les API routes Next qui chaînent ≥ 2 étapes (fetch DB + transformation + write, appel externe + cache, etc.). Renvoie-moi la liste classée par priorité.

2. Intégration. Une fois la liste validée :
   - Installe ou copie le package `@homeroute/flow-action` (selon le scaffold initial)
   - Crée `app/api/_flow/[type]/[name]/route.ts` (catchall) qui appelle `handleFlowCallback({ actions, connectors })`. Voir le sample dans le skill.
   - Pose `HR_FLOW_TOKEN=<token>` dans `.env.local` (vérifie qu'il est dans `apps.json` ; sinon `mcp__homeroute__app.regenerate_flow_token`)
   - Crée le dossier `flows/` à la racine de l'app
   - Aucune toolchain Rust à installer

3. Migration par lot. Pour chaque route candidate : crée `flows/<nom>.toml`, écris une action TS dans `lib/flow-actions/<nom>.ts`, importe-la dans le handler catchall, déploie, teste via `mcp__homeroute__flow.run`. Le handler Next d'origine garde un fetch vers l'API homeroute ou appelle directement le `RemoteEngine` côté Node (le skill détaille le pattern).

4. Escalade plateforme identique : STOP + rapport + attendre correctif.

Démarre par l'étape 1 : l'audit.
```

## Procédure de création d'une nouvelle app (steady-state, post-phase 7)

Après la phase 7, créer une nouvelle app HomeRoute la rend **immédiatement** flow-ready, quelle que soit la stack. Le workflow :

1. **Création via Studio** (`POST /api/apps`) : l'utilisateur choisit le slug, le nom, la stack (NextJS / Rust+Vite / Rust). L'API :
   - Génère un `flow_callback_token` aléatoire (32 bytes hex)
   - Pose le token dans `apps.json` (`flow_callback_token`, `flow_callback_url=http://localhost:<port>`)
   - Pose le token dans le `.env` de l'app (`HR_FLOW_TOKEN=…`)
   - Notifie le daemon (`POST localhost:4002/v1/_admin/reload?slug=<new>`)

2. **Templates de scaffold injectés** par `hr-apps::context::generate_for_app` :
   - **NextJS** : `app/api/_flow/[type]/[name]/route.ts` (catchall), `lib/flow-actions/index.ts` (vide), `package.json` avec dep `@homeroute/flow-action`
   - **Rust+Vite / Rust** : `server/src/flows/mod.rs` avec `register_callbacks` + import `hr-flow-callback`, `Cargo.toml` avec dep
   - **Tous** : dossier `flows/` avec un `hello.toml` (un seul step `compose` qui retourne `Hello {{ input.name }}`)
   - **Tous** : `flows/` ajouté à `build_artefact` (apps Rust uniquement — pour les NextJS, tout est déjà inclus dans `next build`)

3. **Rule + skill poussés** : `flows-first.md` (always-on, sans la section "État de la migration" qui n'a plus de sens en steady-state) + skill `flow-build` adapté à la stack.

4. **Premier `make app-build SLUG=<new>`** : l'app build et démarre. `mcp__homeroute__flow.run(slug=<new>, name="hello", input={"name": "world"})` retourne `{"greeting": "Hello world"}` immédiatement, sans aucune intervention manuelle.

5. **L'utilisateur peut alors ouvrir Studio** → tab Flows → l'app a déjà un flux `hello` listé et un run récent visible.

C'est l'**objectif final** : un workflow fluide où la création d'une app est indissociable de la capacité à orchestrater. Les flux sont le mécanisme général de HomeRoute, pas une feature optionnelle.
