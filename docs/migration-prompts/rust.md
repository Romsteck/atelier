# Prompt de migration — apps Rust (axum-vite / axum) vers `hr-flowd` callback

> Copie tout ce qui est sous le séparateur dans la conversation de l'agent de l'app cible.

---

Migration vers hr-flow (mode callback / daemon partagé).

**Contexte plateforme** : `hr-flow` ne s'embarque plus dans chaque app — un daemon partagé `hr-flowd` (Medion `127.0.0.1:4002`, loopback only) orchestre tous les flux. Chaque app expose ses actions/connecteurs custom via HTTP sous `POST /_flow/action/{name}` et `POST /_flow/connector/{name}/{op}`. Wallet a validé le pilote ; tu en bénéficies sans passer par la phase embedded.

**Étapes — fais-les dans l'ordre, vérifie après chaque.**

### 1. Audit (rends-moi la liste avant de coder)

Repère dans `server/src/routes/` toutes les routes/handlers qui chaînent **≥ 2 étapes** : appel DB + transformation, appel externe + écriture, boucle/condition portant sur des données métier, etc. Renvoie-moi la liste classée par priorité (haut = simple à migrer / fort impact métier), avec une phrase métier par route.

### 2. Intégration plateforme (une fois la liste validée)

- `server/Cargo.toml` : ajoute
  ```toml
  hr-flow = { path = "../../../../../../nvme/atelier/crates/hr-flow" }
  hr-flow-callback = { path = "../../../../../../nvme/atelier/crates/hr-flow-callback" }
  ```
  (Le path est relatif depuis `server/Cargo.toml` ; ajuste s'il y a un niveau d'écart.)

- Crée `server/src/flows/mod.rs` :
  ```rust
  use std::sync::Arc;
  use axum::Router;
  use hr_flow::Connector;

  pub mod actions;     // tu y mettras tes #[flow_action] async fn
  pub mod connectors;  // tes connecteurs custom (Connector trait)

  pub fn register_callbacks() -> Router<Arc<crate::AppState>> {
      let mut router = hr_flow_callback::router();
      // Connecteurs custom — exemple :
      // router = router.with_connector("openrouter", Arc::new(connectors::openrouter::Client::from_env()?));
      // Actions custom — chaque mount_<fn> est généré par #[flow_action] :
      // router = router.with_action(actions::scoring::mount_compute_risk_score);
      router.into_router()
  }
  ```
  Tu rempliras les `with_action` / `with_connector` au fur et à mesure. Pas de `FlowEngineBuilder` ici, pas de `JsonRunStore` non plus — c'est le daemon qui orchestre et persiste.

- `server/src/main.rs` : merge le sous-router callback dans le router principal.
  ```rust
  let api = Router::new()
      .route("/api/health", get(health::check))
      // ... routes existantes ...
      .merge(flows::register_callbacks())
      .with_state(state.clone())
      // ... layers + fallback ...
  ;
  ```
  **IMPORTANT** : appelle `merge` AVANT le premier `.with_state(...)` (axum 0.8 exige que les routers soient state-compatibles avant fixation du state).

- Variables d'env requises côté `.env` de l'app (canonical sur CloudMaster `/opt/homeroute/apps/<slug>/.env`) :
  ```
  HR_FLOW_TOKEN=<token>
  ```
  Le token est généré par l'utilisateur (32 bytes hex) ; il doit ÉGALEMENT être ajouté à `apps.json` côté Medion sur l'entrée de l'app, sous `flow_callback_url` (`http://127.0.0.1:<port>`) et `flow_callback_token`. Vérifie avec `mcp__atelier__app.get_app(slug=...)` ou `mcp__homeroute__app.regenerate_flow_token(slug=...)` (selon le tool disponible).

- Crée le dossier `flows/` à la racine `server/src/flows/` (relatif à cwd, qui est `<slug>/src/`).

- Ajoute `flows/` au `build_artefact` de l'app (sinon les TOML ne montent pas en prod via rsync).

- Build : `make app-build SLUG=<ton-slug>` (ou via Studio).

### 3. Migration par lot

Prends 1 ou 2 routes simples de la liste, transforme-les en flux TOML sous `server/src/flows/<nom>.toml` (format plat, parent / parent_branch ; cf. skill `flow-build`). Le handler garde un wrapper mince qui :

- POST `http://127.0.0.1:4100/api/apps/<slug>/flows/<nom>/run` avec `{ "input": ... }`, OU
- Utilise `hr_flow::RemoteEngine::from_env(slug)?.run(name, input).await` (depuis le code Rust de l'app).

Build, déploie via `make deploy-app SLUG=<slug>`, teste via `mcp__homeroute__flow.run(slug=<slug>, name=<flow>, input=...)`.

Itère sur le lot suivant. Garde la tâche réglée même si la route originelle n'est pas encore décommissionnée — la doctrine est de migrer progressivement, pas de big-bang.

### 4. Escalade plateforme — règle stricte

Si tu rencontres un **bug** ou une **limitation** dans `hr-flow` / `hr-flow-callback` / `hr-flowd` (engine, primitive, expression, connecteur managé, persistence, callback) :

1. **STOP. NE CONTOURNE PAS.** Pas d'action Rust custom pour faire le boulot d'une primitive manquante. Pas de hack qui maquille le bug.
2. **Rapport structuré** dans la conversation, en français :
   - **Sévérité** : `P0` (bloque la migration) / `P1` (workaround moche obligatoire) / `P2` (ergonomie)
   - **Contexte** : quel flux, quel step, quel input réel
   - **Repro minimale** : extrait TOML + ce que tu attends + ce qui se passe
   - **Hypothèse** sur la cause si tu en as une
3. **Attends le correctif**. La plateforme est maintenue dans `/nvme/atelier/crates/hr-flow*` par un agent dédié. Le user transmet, l'agent corrige et redéploie, puis te dit de reprendre.
4. Pas de TODO bidouille pendant l'attente : passe à un autre lot ou autre tâche.

### Démarre par l'étape 1 : l'audit.
