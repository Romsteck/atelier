# Plan — Extraction Atelier (couche applicative hors homeroute)

> ⚠️ **HISTORIQUE** — décision révisée : Atelier puis ses sources ont été rapatriés sur Medion ; **CloudMaster est décommissionné (2026-05-31)**. Voir `CLAUDE.md` pour l'état courant.

## Context

Le repo `/nvme/homeroute/` héberge aujourd'hui deux produits qui n'ont rien en commun fonctionnellement : un **routeur réseau** (DNS, DHCP, reverse proxy, ACME, monitoring hosts) et une **plateforme applicative** (apps supervisées, Dataverse Postgres, moteur de flows, Studio, docs, MCP). Les deux se contraignent mutuellement : impossible de redémarrer l'edge librement à cause des apps, impossible de déployer la plateforme en entreprise sans embarquer DNS/DHCP, releases/tests imbriqués, frontend monolithique.

Décision : extraire la couche applicative dans un nouveau projet **Atelier** sur `/nvme/atelier/`, déployé sur **CloudMaster** (10.0.0.10), accessible via `app.mynetwk.biz`. Migration progressive feature-par-feature, validation parallèle, puis cutover. Homeroute reste sur Medion (10.0.0.254) avec netcore, edge, et un thin API réseau.

**Décisions figées** (validées par l'utilisateur) :
1. Atelier tourne sur **CloudMaster** (pas Medion). Code-server du Studio aussi.
2. Atelier est un **nouveau projet** (frontend from scratch, pas de copie de `web/`).
3. **`hr-registry` reste dans homeroute** (gestion agents/hosts, pas applicatif).
4. Migration **parallèle** : Atelier validé, puis suppression du code applicatif de homeroute.
5. Endpoint public : `app.mynetwk.biz` → reverse-proxy hr-edge (Medion) → CloudMaster:4100.

## Architecture cible

```
Internet → Cloudflare → Medion (10.0.0.254)
                          └─ hr-edge (443/80, ACME, auth, tunnel, reverse proxy)
                                ├─ {slug}.mynetwk.biz   → 127.0.0.1:port-app  (apps existantes pendant transition)
                                ├─ proxy.mynetwk.biz    → 127.0.0.1:4000      (homeroute thin API)
                                ├─ studio.mynetwk.biz   → 10.0.0.10:8443      (code-server CloudMaster, déjà en place)
                                └─ app.mynetwk.biz      → 10.0.0.10:4100      (Atelier, NOUVEAU)
                          └─ hr-netcore (DNS, DHCP, adblock, ipv6)
                          └─ homeroute (4000) — thin API réseau uniquement après cutover

CloudMaster (10.0.0.10)
  └─ atelier (4100) — apps, dataverse, flows, studio, docs, MCP
  └─ hr-studio.service (8443) — code-server (déjà en place)
```

## Architecture intermédiaire (parallel run)

Pendant les Phases 0–8, **les deux stacks coexistent** :
- Homeroute (Medion) : hr-orchestrator continue à tourner, supervise tous les processus apps existants. Frontend `web/` actuel reste tel quel.
- Atelier (CloudMaster) : reçoit features progressivement, en lecture seule au début. Lit la même donnée (`/opt/homeroute/apps/` est déjà sur CloudMaster comme source canonique).
- Une feature migrée = accessible via les **deux** endpoints simultanément. Pas de cassure pour l'utilisateur.

## Phases

### Phase 0 — Bootstrap Atelier (CloudMaster)

**Objectif** : binaire `atelier` qui boote, écoute `:4100` HTTP + `/run/atelier.sock`, répond `/health`.

**Étapes** :
1. `git init /nvme/atelier/`, branch `main`.
2. `/nvme/atelier/Cargo.toml` workspace : members `["crates/atelier", "crates/atelier-api"]`, `[workspace.dependencies]` recopié depuis `/nvme/homeroute/crates/Cargo.toml` (axum 0.8, tokio, rusqlite 0.39, sqlx, etc. — gel des versions).
3. Path-deps vers homeroute (à la racine) :
   ```toml
   hr-common = { path = "/nvme/homeroute/crates/shared/hr-common" }
   hr-ipc    = { path = "/nvme/homeroute/crates/shared/hr-ipc" }
   hr-docs   = { path = "/nvme/homeroute/crates/shared/hr-docs" }
   ```
4. `crates/atelier/src/main.rs` : tracing-subscriber, env `ATELIER_HTTP_ADDR=0.0.0.0:4100`, `ATELIER_IPC_SOCK=/run/atelier.sock`, axum router minimal `GET /health → "ok"`, task tokio `UnixListener` (handler vide pour l'instant).
5. `crates/atelier-api/src/{lib.rs,state.rs,routes/health.rs}` : `pub fn router() -> Router<ApiState>`, `ApiState` struct vide.
6. `/nvme/atelier/CLAUDE.md` (zéro downtime, docs-first, deploy chain), `/nvme/atelier/.claude/rules/`, `/nvme/atelier/.mcp.json`.
7. Build CloudMaster : `cargo build --release --manifest-path /nvme/atelier/Cargo.toml`.
8. systemd unit `/nvme/atelier/systemd/atelier.service` (à installer dans `/etc/systemd/system/` sur CloudMaster, analogue à `/nvme/homeroute/systemd/homeroute.service`) :
   ```
   [Service]
   Type=simple
   ExecStart=/opt/atelier/bin/atelier
   WorkingDirectory=/opt/atelier
   EnvironmentFile=/opt/atelier/.env
   Restart=always
   RestartSec=5
   ReadWritePaths=/run /var/lib/atelier /opt/atelier/data
   ```
9. Deploy chain :
   ```bash
   rsync -a /nvme/atelier/target/release/atelier 10.0.0.10:/opt/atelier/bin/
   ssh root@10.0.0.10 'systemctl restart atelier && journalctl -u atelier -n 50'
   ```

**Vérif** : `curl http://10.0.0.10:4100/health` → `ok`. `ls -l /run/atelier.sock`. `journalctl -u atelier` propre.

**Critère de passage** : `/health` répond, socket IPC créé, redémarrage propre 3 fois consécutifs.

---

### Phase 1 — Route edge + endpoint public

**Objectif** : `https://app.mynetwk.biz/health` répond depuis Internet.

**Étapes** :
1. **Avant** d'ajouter la route edge : confirmer que Phase 0 répond sur `10.0.0.10:4100`. Sinon 502 visible côté user.
2. Sur Medion, appeler `EdgeClient::set_app_route` via le helper qui existe déjà côté hr-orchestrator (ou écrire un `atelier-bootstrap` one-shot binaire) :
   - `domain="app.mynetwk.biz"`, `app_id="atelier"`, `host_id="cloudmaster"`, `target_ip="10.0.0.10"`, `target_port=4100`, `auth_required=false` (Atelier gère sa propre auth via session edge plus tard si besoin), `local_only=false`.
3. hr-edge persiste dans `/var/lib/server-dashboard/rust-proxy-config.json` automatiquement et propage au DNS local via `DnsRouteSync`.
4. Vérifier l'émission ACME : `mcp__hr-edge__acme_status` ou `journalctl -u hr-edge | grep app.mynetwk.biz`. Si wildcard `*.mynetwk.biz` actif côté Cloudflare, la résolution publique fonctionne immédiatement.
5. `dig app.mynetwk.biz` (depuis l'extérieur) doit retourner l'IP publique de Medion (via Cloudflare proxy).

**Vérif** :
- `curl -v https://app.mynetwk.biz/health` (depuis Internet) → 200 `ok`, TLS valide.
- `journalctl -u hr-edge` montre `proxy app.mynetwk.biz → 10.0.0.10:4100`.

**Risques** :
- *Cert ACME pas émis* → 526. Mitigation : attendre 30s, vérifier wildcard.
- *Atelier crashed entre deux requêtes* → 502. Mitigation : `Restart=always` dans systemd.

**Critère de passage** : `curl https://app.mynetwk.biz/health` retourne 200 depuis Internet, sans erreur TLS.

---

### Phase 2 — Migration de Docs (première feature, read-only)

**Objectif** : `/api/docs/*` servi par Atelier en read-only, frontend Atelier minimal pour les visualiser. Choix de docs en premier : `hr-docs` est déjà une lib autonome `/nvme/homeroute/crates/shared/hr-docs`, FTS5 SQLite local, lecture seule par défaut, aucun couplage runtime apps. La donnée canonique vit dans `/opt/homeroute/apps/` qui est **déjà sur CloudMaster**.

**Étapes** :
1. Dans `crates/atelier-api/src/state.rs` : ajouter `pub docs_index: Option<Arc<hr_docs::Index>>` (copier le pattern de [/nvme/homeroute/crates/api/hr-api/src/state.rs:51](/nvme/homeroute/crates/api/hr-api/src/state.rs#L51)).
2. Copier [/nvme/homeroute/crates/api/hr-api/src/routes/docs.rs](/nvme/homeroute/crates/api/hr-api/src/routes/docs.rs) → `/nvme/atelier/crates/atelier-api/src/routes/docs.rs`. Adapter `use crate::state::ApiState`.
3. Boot Atelier : initialiser l'index FTS5 au démarrage en lisant `ATELIER_DOCS_DIR=/opt/homeroute/apps` (defaut). Rebuild de l'index au boot — suffit pour Phase 2 (pas d'écritures).
4. `crates/atelier-api/src/lib.rs` : `.nest("/api/docs", routes::docs::router())`.
5. Frontend Atelier minimal : créer `/nvme/atelier/web/` (Vite + React + TS, from scratch). Page unique `Docs.tsx` qui consomme `/api/docs`, `/api/docs/{app}/overview`, `/api/docs/{app}/{type}/{name}`. `npm run build` → `web/dist/`.
6. Servir le frontend depuis Atelier via `tower_http::services::ServeDir` monté sur `/`.
7. Build + rsync (binaire + `web/dist/`) + restart + healthcheck.

**Vérif** :
- `curl https://app.mynetwk.biz/api/docs` → liste JSON des apps documentées.
- `curl https://app.mynetwk.biz/api/docs/<slug>/overview` → JSON avec body markdown.
- Naviguer `https://app.mynetwk.biz/` → liste docs s'affiche.
- Côté homeroute : `curl https://proxy.mynetwk.biz/api/docs` continue à fonctionner (zéro régression).
- Comparer un même `/{slug}/overview` côté homeroute et côté Atelier — donnée identique.

**Risques** :
- *Index FTS5 obsolète si écriture côté Medion entre deux boots Atelier* → docs nouvelles invisibles côté Atelier. Mitigation : timer systemd `atelier-rebuild-index.timer` (toutes les 5 min), ou inotify watcher (Phase ultérieure).
- *Path docs différent* : env `ATELIER_DOCS_DIR` configurable.

**Critère de passage** : Atelier sert docs read-only ; les deux côtés affichent la même donnée pendant 24h sans divergence.

---

### Phase 3+ — Migration progressive (ordre raisonné)

Critères : autonomie de la lib, absence d'état runtime partagé, read-only avant read-write.

| # | Feature | Crates / Routes | Notes critiques |
|---|---------|-----------------|-----------------|
| 3 | **Store** | `store.rs` | Catalogue apps, lecture sur fichiers `manifest.yaml`. Aucun runtime. |
| 4 | **Git** | `hr-git` + `git.rs` | Bare repos `/opt/homeroute/repos/`, smart HTTP. État sur disque. |
| 5 | **Flows read-only** | `hr-flow`, `hr-flow-macros`, `flows.rs` GET only | Lecture définitions + statuts. Pas d'exécution. |
| 6 | **Flows read-write** | idem + POST/PATCH | Activer triggers. Si un flow déclenche `apps.start`, il faut router vers le supervisor (encore sur Medion en Phase 6). Solution : tunneler IPC orchestrator via TCP local-only (10.0.0.254:4001) protégé par token, ou exposer endpoint hr-orchestrator interne. |
| 7 | **Dataverse** | `hr-dataverse`, `hr-dvexpr`, `hr-dataverse-migrate`, `hr-dv-codegen`, `dv.rs`, `apps_db.rs` | Postgres-dataverse externe. Vérifier en pré-requis où il tourne (probablement Medion). Atelier s'y connecte via LAN. Pas de migration de la base — juste du code. |
| 8 | **Tasks** | `tasks.rs` | Système async ; à porter une fois orchestrator ciblé. |
| 9 | **Apps lifecycle** | `hr-apps`, `hr-orchestrator` (binaire), `hr-db` + `apps.rs` | LE plus complexe : supervisor Tokio, sockets process, base SQLite stateful. → **Phase finale**. |

À chaque phase 3–8 :
1. Copier la lib si autonome (path-dep d'abord, déplacer physiquement à la fin) ; sinon extraire de hr-orchestrator.
2. Copier la route correspondante de `hr-api` vers `atelier-api`.
3. Ajouter au router Atelier dans `crates/atelier-api/src/lib.rs`.
4. Recréer la(les) page(s) frontend correspondante(s) dans `/nvme/atelier/web/`.
5. Deploy chain (build CloudMaster → rsync → restart → curl healthcheck → comparaison parité avec homeroute).
6. Watch 48h en parallèle, comparer payloads JSON random pour détecter divergences.

**Critère générique** : feature accessible sur `app.mynetwk.biz`, donnée identique côté homeroute, aucune régression observée pendant 48h.

---

### Phase finale — Cutover des Apps

**Décision ouverte à trancher avant Phase 9** (pas maintenant) : où tournent les processus apps après cutover ?

- **Option A** (supervision distante) : Atelier sur CloudMaster supervise des process qui tournent sur Medion via un agent `hr-app-runner` exposant un IPC. Garde les apps proches de l'edge.
- **Option B** (rapatriement) : les processus apps migrent sur CloudMaster aussi. Atelier les supervise localement. Archi plus simple, mais perd le placement réseau.

**Étapes du cutover** (une fois l'option choisie) :
1. `systemctl stop hr-orchestrator` sur Medion (gel des écritures côté homeroute). Apps continuent à tourner si supervisées par systemd ou processus detached — vérifier d'abord.
2. Snapshot SQLite : `cp /var/lib/homeroute/orchestrator.db /backup/orchestrator-cutover-$(date +%s).db`.
3. Rsync DB → CloudMaster : `/var/lib/atelier/orchestrator.db`.
4. Atelier reprend la supervision : `hr-apps` (intégré dans Atelier) prend le relais, reprend les PIDs (Option A : via runner Medion ; Option B : redémarre les apps localement, downtime court par app).
5. Réécrire toutes les routes hr-edge `{slug}.mynetwk.biz` pour pointer vers les nouveaux targets (script idempotent : `ListAppRoutes` → loop → `SetAppRoute`).
6. Watch 24h chaque app health.
7. **Suppression du code applicatif dans homeroute** :
   - `crates/orchestrator/{hr-orchestrator,hr-apps,hr-db,hr-git,hr-flow,hr-flow-macros,hr-dataverse,hr-dvexpr,hr-dataverse-migrate,hr-dv-codegen,hr-container}` → suppression complète.
   - `crates/api/hr-api/src/routes/{apps,apps_db,dv,flows,docs,store,git,tasks}.rs` → suppression.
   - Pages `web/src/pages/{Studio,AppDetail,DbExplorer,SchemaPage,Store,FlowsStats,Git}.jsx` → suppression.
   - `crates/Cargo.toml` workspace members : retirer les crates supprimés.
   - Conserver dans homeroute : `hr-registry`, netcore, edge (et hr-common, hr-ipc, hr-docs partagés).
8. Rebuild homeroute, redeploy Medion, vérifier que netcore + edge + thin API tournent.
9. Supprimer `hr-orchestrator.service` de Medion.

**Vérif cutover** : toutes apps répondent sur leurs domaines, MCP `atelier.apps_list` liste apps avec statut running, `journalctl -u atelier` propre 24h, `journalctl -u homeroute` propre 24h.

**Risques** :
- Perte de données SQLite pendant rsync → stop hr-orchestrator avant rsync (transactionnel).
- Routes edge mal repointées → script idempotent + dry-run + diff avant apply.
- Path-deps cassées si homeroute supprime `hr-common`/`hr-ipc`/`hr-docs` → ces crates **restent** dans `homeroute/crates/shared/`. Atelier continue à les référencer par path. Refactor en repo `homeroute-core` séparé = travail futur, hors scope de ce plan.

**Critère final** : 0 process orchestrator sur Medion, 100 % du trafic applicatif sur CloudMaster, dashboard homeroute (hosts, dns, energy, etc.) fonctionne normalement.

---

## Fichiers critiques à modifier

### Côté homeroute (lecture pour copier, modifications minimales)
- [crates/Cargo.toml](/nvme/homeroute/crates/Cargo.toml) — workspace deps version reference
- [crates/shared/hr-common/](/nvme/homeroute/crates/shared/hr-common/) — path-dep depuis Atelier
- [crates/shared/hr-ipc/src/edge.rs](/nvme/homeroute/crates/shared/hr-ipc/src/edge.rs) — `EdgeClient::set_app_route` pour Phase 1
- [crates/shared/hr-docs/](/nvme/homeroute/crates/shared/hr-docs/) — path-dep, Phase 2
- [crates/api/hr-api/src/routes/docs.rs](/nvme/homeroute/crates/api/hr-api/src/routes/docs.rs) — copié en Phase 2
- [crates/api/hr-api/src/state.rs](/nvme/homeroute/crates/api/hr-api/src/state.rs) — pattern de référence pour `ApiState`
- [crates/api/hr-api/src/routes/](/nvme/homeroute/crates/api/hr-api/src/routes/) — autres routes copiées Phases 3–8
- [systemd/homeroute.service](/nvme/homeroute/systemd/homeroute.service) — template pour `atelier.service`

### Côté Atelier (création)
- `/nvme/atelier/Cargo.toml` (workspace racine)
- `/nvme/atelier/crates/atelier/src/main.rs` (binaire)
- `/nvme/atelier/crates/atelier-api/src/{lib.rs,state.rs,routes/}` (lib API)
- `/nvme/atelier/web/` (frontend Vite/React from scratch, Phase 2+)
- `/nvme/atelier/systemd/atelier.service`
- `/nvme/atelier/CLAUDE.md`, `/nvme/atelier/.claude/rules/{deploy-chain,zero-downtime,docs-first,testing,logging}.md`, `/nvme/atelier/.mcp.json`
- `/nvme/atelier/Makefile` (`make atelier`, `make web`, `make deploy-cloudmaster`)

### Côté infra (sur Medion + CloudMaster)
- `/var/lib/server-dashboard/rust-proxy-config.json` (route `app.mynetwk.biz`, ajoutée par IPC en Phase 1)
- `/etc/systemd/system/atelier.service` (CloudMaster, Phase 0)
- `/opt/atelier/{bin,data}/` (CloudMaster, runtime Atelier)

---

## Contraintes transverses

- **Zéro downtime** (mémoire `feedback_no_downtime.md`) : à chaque phase 2–8, homeroute continue à servir la feature en parallèle. Cutover par feature, pas big-bang.
- **Deploy chain** (mémoire `feedback_deploy_chain.md`) : build CloudMaster → rsync `/opt/atelier/bin/atelier` → `systemctl restart atelier` → `curl /health` → `journalctl -u atelier`.
- **Test après deploy** (mémoire `feedback_test_after_deploy.md`) : à chaque phase, comparer payloads identiques entre homeroute et Atelier.
- **Logging** (mémoire `feedback_comprehensive_logging.md`) : Atelier logue tout dès Phase 0 (tracing-subscriber, structured logging).
- **Docs-first** (mémoire `homeroute-docs.md`) : le système docs migre en Phase 2 ; pour toute nouvelle feature ajoutée à Atelier, doc écrite avant code (règle dans `/nvme/atelier/CLAUDE.md`).
- **Pas d'attribution Claude** dans les commits (mémoire `feedback_no_attribution.md`).

---

## Vérification end-to-end finale (post-cutover)

1. `curl https://app.mynetwk.biz/api/health` → 200 (Atelier).
2. `curl https://proxy.mynetwk.biz/api/health` → 200 (homeroute thin).
3. Toutes les apps répondent sur leurs `{slug}.mynetwk.biz`.
4. MCP : `mcp__atelier__apps_list` retourne les apps avec statut `running`.
5. MCP : `mcp__atelier__docs_overview` retourne overview pour chaque app.
6. Frontend Atelier `https://app.mynetwk.biz/` affiche : Studio, Apps, Docs, Dataverse, Flows, Store, Git.
7. Frontend homeroute `https://proxy.mynetwk.biz/` affiche : Dashboard réseau, Hosts, DNS, ReverseProxy, Certs, etc. (sans onglets app).
8. `journalctl -u atelier --since "24 hours ago" | grep -i error` → vide.
9. `journalctl -u homeroute --since "24 hours ago" | grep -i error` → vide.
10. `systemctl status hr-orchestrator` (Medion) → `not-found` (service supprimé).

---

## Plan suivant (post-cutover) — `hr-flowd` daemon multi-stack

**À la fin de cette migration**, le travail enchaîne directement sur le plan **`hr-flowd` daemon** : [/home/romain/.claude/plans/peaceful-spinning-mountain.md](/home/romain/.claude/plans/peaceful-spinning-mountain.md).

Résumé : transformer `hr-flow` (aujourd'hui une lib Rust embeddable, donc les apps NextJS ne peuvent pas l'utiliser) en daemon partagé `hr-flowd` accessible via HTTP. Toutes les apps — Rust comme NextJS — branchent leurs actions custom via callbacks HTTP. Plan en 7 phases (daemon → RemoteEngine → callback NextJS → bascule Wallet → roll-out apps Rust → roll-out apps NextJS → scaffold automation). Concerne directement Atelier puisque `hr-flow` migre dans Atelier (Phase 5/6 du présent plan).

**Action obligatoire en Phase 0** :
- Créer `/nvme/atelier/CLAUDE.md` avec une section **"Plan en cours / Plan suivant"** qui pointe explicitement vers `/home/romain/.claude/plans/peaceful-spinning-mountain.md` afin que le prochain agent qui ouvre Atelier sache d'emblée que ce travail est le next step.
- Créer aussi `/nvme/atelier/.claude/rules/next-plan.md` (always-on) qui résume en 5 lignes : "Une fois la migration depuis homeroute terminée (cutover Phase 9), enchaîner sur le plan hr-flowd daemon — rendre les flows utilisables depuis n'importe quelle stack via callbacks HTTP. Détails dans `/home/romain/.claude/plans/peaceful-spinning-mountain.md`."
- Lors de la migration de `hr-flow` (Phase 5 du présent plan), **garder en tête** que la cible n'est plus la lib embedded mais un daemon. Concrètement : ne pas refactorer `hr-flow` dans Atelier de manière qui rendrait l'extraction du daemon plus difficile (pas de couplage fort à `ApiState` ou au runtime des apps).

## Décisions à prendre plus tard (hors scope de ce plan)

1. **Phase 9 — Option A vs B** : runtime apps sur Medion (agent runner) ou sur CloudMaster (rapatriement). À trancher après Phase 8.
2. **Repo `homeroute-core`** : extraire `hr-common`, `hr-ipc`, `hr-docs` dans un repo séparé. Décision : après cutover stable.
3. **Postgres-dataverse** : où tourne-t-il post-cutover ? Si actuellement sur Medion, peut rester ; si à déplacer sur CloudMaster, faire dans la phase Dataverse.
4. **Auth** : hr-edge `forward_auth` continue à protéger `app.mynetwk.biz` (route `auth_required=true` plus tard). Atelier hérite gratuitement des sessions de hr-auth. Reconfirmer ce pattern avant Phase 6.
