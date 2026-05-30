# Plan — Rapatriement Atelier sur Medion

> ⚠️ **HISTORIQUE** — ce plan documente le rapatriement du binaire/Studio en gardant le source Atelier + son build sur CloudMaster. Depuis le **2026-05-31**, le source et le build ont aussi été rapatriés sur Medion (`/home/romain/atelier`) et **CloudMaster est décommissionné**. Voir `CLAUDE.md` pour l'état courant.

## Contexte

**But initial de l'extraction Studio → Atelier** (rappelé par l'utilisateur) :
1. Solution autonome et transportable (pas un sous-pan d'homeroute)
2. Servir les apps via path-based routing (`https://app.mynetwk.biz/apps/{slug}`) au lieu de `{slug}.mynetwk.biz`

**Pivot demandé maintenant** : revenir sur la décision d'héberger Atelier sur CloudMaster. Atelier doit être **sur Medion** (qui héberge déjà Postgres dataverse, hr-edge, et les data du raid0). code-server reste sur CloudMaster pour le dev.

**Ce que cette session fait** : la migration de relocalisation. Le path-routing `app.mynetwk.biz/apps/{slug}` est explicitement reporté à un plan ultérieur (réponse utilisateur).

**Décisions cadrantes (déjà prises) :**

- **IP Medion** : `10.0.0.254` (interface principale, double-homed `.254` + `.20` confirmé par l'utilisateur — code et `.env` continuent à utiliser `.254`).
- **Sources et build sur CloudMaster** : code-server édite `/opt/homeroute/apps/{slug}/src/` sur CloudMaster, `cargo build` / `npm run build` y restent. Atelier Medion reçoit uniquement les **artefacts** (binaire + dist/ + .env + db.sqlite legacy + structure minimale `src/`).
- **Path-routing** : reporté → `{slug}.mynetwk.biz → 127.0.0.1:port` (loopback Medion) reste en place après migration.
- **Apps stopped** (aptymus, calendar, forge, padel — NextJS) : **non migrées**. Sources restent sur CloudMaster, registry Atelier sur Medion ne contient que les 6 running. Si on veut les ressusciter un jour, on les migrera à ce moment-là.
- **App `files`** : data canoniques = `/ssd_pool/homecloud/data/` sur Medion (déjà). Le `.env` Files doit être réécrit pour pointer vers ces paths absolus, pas vers `data/storage` relatif au CWD.

**État courant constaté :**

| Apps running (6 — à migrer) | Ports | Stack | Frontend | Binaire (CM) |
|---|---|---|---|---|
| files   | 3006 | axum + vite     | `src/web/dist/` | `src/bin/files` (19 MB) |
| home    | 3007 | axum + vite     | `src/client/dist/` | `src/server/target/release/smart-home` (12 MB) |
| myfrigo | 3010 | axum (workspace)| `src/api/static/` | `src/bin/myfrigo` (17 MB) |
| trader  | 3008 | axum + vite     | embedded dans binaire | `src/bin/trader` (17 MB) |
| wallet  | 3009 | axum + vite     | `src/client/dist/` | `src/bin/wallet` (16 MB) |
| www     | 3005 | next.js         | `src/.next/standalone/` | `node .next/standalone/server.js` |

Total à transférer ≈ 14 GB (en filtrant les sources / target).

## Architecture cible

```
Internet → Cloudflare → Medion (10.0.0.254)
                          ├─ hr-edge
                          │   ├─ {slug}.mynetwk.biz → 127.0.0.1:{3005-3010}  (loopback Medion)
                          │   ├─ app.mynetwk.biz   → 127.0.0.1:4100          (Atelier Medion)
                          │   ├─ studio.mynetwk.biz → 10.0.0.10:8443         (code-server, inchangé)
                          │   └─ proxy.mynetwk.biz  → 127.0.0.1:4000         (homeroute, inchangé)
                          ├─ atelier.service (4100)
                          ├─ hr-app-{files,home,myfrigo,trader,wallet,www}.service
                          └─ Postgres-dataverse (5432)

CloudMaster (10.0.0.10)
  ├─ hr-studio.service (8443)        ← code-server (édition sources)
  ├─ /opt/homeroute/apps/{slug}/src/ ← sources canoniques (build local)
  └─ Makefile : make deploy-app slug=<x> (build + rsync vers Medion + restart via API Atelier)
```

## Phases

### Phase 0 — Préparation (sans impact runtime)

Sur CloudMaster (lectures + write des configs locales) :

0.1. Snapshot de l'état actuel (dans `/var/log/atelier-migration/`) :
- `cp /opt/atelier/data/apps.json` + `port-registry.json` + `dataverse-secrets.json`
- `systemctl list-units 'hr-app-*' > services-cloudmaster.txt`
- `du -sh /opt/homeroute/apps/* > sizes.txt`
- `for slug in files home myfrigo trader wallet www; do cp /opt/homeroute/apps/$slug/.env env-$slug.txt; done`

0.2. Vérifier prérequis Medion via SSH `romain@10.0.0.254` :
- `systemctl is-enabled atelier.service` (doit retourner *not-found* — sinon il y a déjà un atelier qui tourne)
- `df -h /opt /var/lib /ssd_pool` (besoin ~15 GB libre dans `/opt`)
- `ls /ssd_pool/homecloud/data/` (confirmer files/, thumbnails/, downloads/, films/)
- Vérifier que les ports `4100, 3005-3010` sont libres : `ss -tlnp | awk '{print $4}' | grep -E ':(4100|300[5-9]|3010)$'` → doit être vide

### Phase 1 — Stage Atelier sur Medion (sans démarrage)

But : préparer Atelier sur Medion en parallèle, encore éteint. Aucun impact runtime.

1.1. Sur CloudMaster : `make atelier && make web` (rebuild propre du binaire + frontend).

1.2. Créer la structure sur Medion :
```
ssh romain@10.0.0.254 "sudo install -d -o root /opt/atelier/{bin,data,web/dist}
                       sudo install -d -o root /var/lib/atelier/{state,docs,store,git/repos,apps}
                       sudo install -d -o root /opt/homeroute/apps"
```

1.3. Rsync artefacts Atelier CloudMaster → Medion :
```
rsync -av /opt/atelier/bin/atelier               romain@10.0.0.254:/tmp/atelier-stage/
rsync -av /opt/atelier/web/dist/                 romain@10.0.0.254:/tmp/atelier-stage/web-dist/
rsync -av /opt/atelier/data/{apps.json,port-registry.json}  romain@10.0.0.254:/tmp/atelier-stage/data/
rsync -av /var/lib/atelier/state/dataverse-secrets.json     romain@10.0.0.254:/tmp/atelier-stage/state/
rsync -av /nvme/atelier/systemd/                 romain@10.0.0.254:/tmp/atelier-stage/systemd/
```

Puis sur Medion : `sudo cp` aux bons emplacements (`/opt/atelier/bin/atelier`, `/opt/atelier/web/dist/`, `/opt/atelier/data/`, `/var/lib/atelier/state/`, `/etc/systemd/system/atelier.service`).

1.4. Écrire `/opt/atelier/.env` sur Medion (adapté à la nouvelle topologie) :
```
ATELIER_DV_ADMIN_URL=postgres://dataverse_admin:f325052bef550223c5c7cbe74a93b16b@127.0.0.1:5432/postgres
ATELIER_APPS_RUNTIME_ROOT=/opt/homeroute/apps
ATELIER_APPS_SRC_ROOT=/opt/homeroute/apps
ATELIER_DV_HOST=127.0.0.1
```
> Changement vs CloudMaster : `10.0.0.254` → `127.0.0.1` partout (Atelier est désormais co-localisé avec Postgres et les apps).

1.5. **Filtrer `apps.json`** pour ne garder que les 6 apps running. Stocker la version filtrée dans `/opt/atelier/data/apps.json` Medion. Mettre les apps NextJS stopped de côté (sauvegarde dans `/var/lib/atelier/state/apps-stopped-archived.json` sur Medion pour pouvoir les ressusciter plus tard).

1.6. **Ne pas démarrer atelier.service encore** (Phase 4).

### Phase 2 — Stage des artefacts apps sur Medion

But : pré-positionner sur Medion tout ce dont les 6 apps running ont besoin pour démarrer, sans toucher à CloudMaster.

Pour chaque slug ∈ {files, home, myfrigo, trader, wallet, www} :

2.1. **Créer le squelette** `/opt/homeroute/apps/{slug}/{src,bin,runs}` sur Medion.

2.2. **Rsync les artefacts** (binaire + assets dist + structure src minimale + .env + db.sqlite legacy si présent), en **excluant** les sources et caches lourds :
```
rsync -av --delete \
  --include='/src/' \
  --include='/src/bin/' --include='/src/bin/**' \
  --include='/src/web/dist/***' \
  --include='/src/client/dist/***' \
  --include='/src/api/' --include='/src/api/static/***' --include='/src/api/Cargo.toml' \
  --include='/src/server/target/release/smart-home' \
  --include='/src/flows/***' \
  --include='/src/.next/standalone/***' --include='/src/.next/static/***' \
  --include='/src/public/***' --include='/src/package.json' --include='/src/node_modules/***' \
  --include='/.env' --include='/db.sqlite' \
  --exclude='*' \
  /opt/homeroute/apps/{slug}/  romain@10.0.0.254:/opt/homeroute/apps/{slug}/
```
> Le filtre est par-app (la liste include exacte diffère selon le stack). Préparer dans `scripts/stage-app.sh` un dispatcher per-stack.

2.3. **Réécrire les `.env`** sur Medion pour adapter les hostnames :
- `HR_DV_BASE_URL=http://127.0.0.1:4100/api/dv/...` → inchangé (Atelier sera local)
- `DATABASE_URL=...@10.0.0.254:5432/...` → `...@127.0.0.1:5432/...`

2.4. **Cas spécial `files`** : réécrire son `.env` pour pointer vers le raid0 :
```
STORAGE_PATH=/ssd_pool/homecloud/data/files
THUMBNAILS_PATH=/ssd_pool/homecloud/data/thumbnails
TORRENT_DOWNLOAD_DIR=/ssd_pool/homecloud/data/downloads
TORRENT_FILMS_DIR=/ssd_pool/homecloud/data/films
DATABASE_URL=postgres://app_files:...@127.0.0.1:5432/app_files
```
> Les data CloudMaster locales (`/opt/homeroute/apps/files/src/data/`, `/data/{downloads,films}` côté CloudMaster) sont **abandonnées** au profit du raid0 Medion. Avant la bascule, faire un diff (Phase 4.0 ci-dessous) pour repérer une éventuelle divergence.

2.5. Pour `files` : copier le SQLite legacy si plus récent que celui de Medion :
- Sur CloudMaster : `stat -c '%Y' /opt/homeroute/apps/files/db.sqlite`
- Via SSH Medion : `stat -c '%Y' /ssd_pool/homecloud/data/app.db`
- Si CloudMaster plus récent → rsync `db.sqlite` → `/ssd_pool/homecloud/data/app.db`
- Si Medion plus récent → laisser tel quel (mais signaler à l'utilisateur).

2.6. Ne pas démarrer les apps encore.

### Phase 3 — Préparer les routes hr-edge (sans bascule)

3.1. Lister les routes courantes : `curl -s http://10.0.0.254:4000/api/edge/routes | jq .` (ou lire `/opt/homeroute/data/app-routes.json` directement sur Medion).

3.2. Construire le mapping cible (à appliquer en Phase 4) :
```
files.mynetwk.biz   → 127.0.0.1:3006
home.mynetwk.biz    → 127.0.0.1:3007
myfrigo.mynetwk.biz → 127.0.0.1:3010
trader.mynetwk.biz  → 127.0.0.1:3008
wallet.mynetwk.biz  → 127.0.0.1:3009
www.mynetwk.biz     → 127.0.0.1:3005
app.mynetwk.biz     → 127.0.0.1:4100
studio.mynetwk.biz  → 10.0.0.10:8443  (inchangé)
```

3.3. Préparer un script `scripts/swap-edge-routes.sh` qui POST chacune de ces routes sur Medion via `/api/edge/routes`. À tester en dry-run AVANT la fenêtre de bascule.

### Phase 4 — Bascule (fenêtre de downtime ~5-10 min)

> Ordre strict, à exécuter à la suite. Sortie du downtime quand 4.6 valide.

4.0. **Sync delta final** (avant freeze) :
- Re-rsync les `.env` + `db.sqlite` per-app pour capter d'éventuels changements depuis Phase 2.
- Pour Files : si les data locales CloudMaster (`/opt/homeroute/apps/files/src/data/`, `/data/downloads`, `/data/films`) ont du contenu, faire un dernier diff vs `/ssd_pool/homecloud/data/` côté Medion. Si divergence inattendue, **pause et demande utilisateur** avant de continuer.

4.1. **Stop apps + atelier sur CloudMaster** :
```
sudo systemctl stop 'hr-app-*'
sudo systemctl stop atelier
sudo systemctl stop atelier-sync-{state,docs,git,store,runs}.timer
```

4.2. **Bascule des routes hr-edge** sur Medion : exécuter `scripts/swap-edge-routes.sh` (Phase 3.3).

4.3. **Démarrer Atelier sur Medion** :
```
ssh romain@10.0.0.254 "sudo systemctl daemon-reload && sudo systemctl enable --now atelier.service"
```
- Vérifier `journalctl -u atelier --since now -f` pour le log de boot. Le supervisor doit re-attacher (Phase 9.4 de l'ancien plan : `attach_existing_units` qui scanne et marque `running` les services qu'il retrouve — ici aucun ne doit exister, état initial = stopped pour les 6).

4.4. **Démarrer les 6 apps via l'API Atelier Medion** :
```
for slug in files home myfrigo trader wallet www; do
  curl -s -X POST http://10.0.0.254:4100/api/apps/$slug/control -d '{"action":"start"}' -H 'content-type: application/json'
done
```

4.5. **Vérifier health** des 6 apps :
```
for slug in files home myfrigo trader wallet www; do
  curl -s -o /dev/null -w "$slug %{http_code}\n" https://$slug.mynetwk.biz/api/health
done
```
Toutes doivent répondre 200. Si l'une échoue : `journalctl -u hr-app-$slug -n 100`.

4.6. **Vérifier Atelier UI** : `curl https://app.mynetwk.biz/api/health` → 200 ; ouvrir le frontend dans un navigateur, vérifier que les 6 apps apparaissent en `running` et que les graphes/logs streament.

### Phase 5 — Validation post-bascule (24-48 h)

5.1. **Smoke tests fonctionnels** sur chaque domaine (à planifier 1×/jour pendant 48h) :
- files : upload + thumbnail + listing → vérifier que le fichier atterrit dans `/ssd_pool/homecloud/data/files/` et pas ailleurs.
- home : dashboard rend, devices listés.
- myfrigo : produit ajouté + listing.
- trader : positions/orders chargent.
- wallet : transactions listent.
- www : pages publiques chargent.

5.2. **Surveiller les logs** : `ssh romain@10.0.0.254 "journalctl -u 'hr-app-*' -u atelier --since '1 hour ago' | grep -iE 'error|warn'"` → ne doit rien remonter d'inattendu.

5.3. **Postgres** : vérifier que les apps écrivent bien dans le Postgres local (DSN `127.0.0.1:5432`) et pas via le réseau (`ss -tnp | grep 5432` côté Medion ne doit montrer que des connexions locales pour les apps).

### Phase 6 — Cleanup CloudMaster

À exécuter **après** confirmation de stabilité (≥48 h post-bascule, signal explicite de l'utilisateur).

6.1. **Désactiver Atelier sur CloudMaster** :
```
sudo systemctl disable --now atelier.service
sudo systemctl disable --now atelier-sync-{state,docs,git,store,runs}.timer
```

6.2. **Supprimer les unit files atelier de CloudMaster** :
```
sudo rm /etc/systemd/system/atelier{,-sync-*}.{service,timer}
sudo systemctl daemon-reload
```

6.3. **Conserver `/opt/atelier/`** (binaire + data + état) **archivé** dans `/var/backups/atelier-cloudmaster-$(date +%F).tar.gz` puis supprimer `/opt/atelier/`. Garder `/var/lib/atelier/` jusqu'à validation finale (1 mois) au cas où — c'est petit.

6.4. **Apps NextJS stopped** : laisser leurs sources dans `/opt/homeroute/apps/{aptymus,calendar,forge,padel}/` sur CloudMaster (intactes, code-server peut continuer à les éditer). Ne pas migrer ; non visibles depuis Atelier Medion.

6.5. **Apps running** : leurs sources `/opt/homeroute/apps/{slug}/src/` restent sur CloudMaster (édition via code-server). Le futur `make deploy-app slug=<x>` (Phase 7) re-builds + rsync vers Medion.

### Phase 7 — Adapter le workflow de déploiement

But : maintenant que les apps tournent sur Medion mais que les sources sont sur CloudMaster, redéfinir `make deploy-app` pour qu'il fasse un push depuis CloudMaster vers Medion.

7.1. Créer `scripts/deploy-app.sh` (sur CloudMaster, dans le repo Atelier) :
```
slug=$1
# 1. Build local (cargo build --release ou npm run build dans src/)
# 2. Rsync artefacts (mêmes filtres que Phase 2.2) vers Medion
# 3. POST http://10.0.0.254:4100/api/apps/$slug/control -d '{"action":"restart"}'
# 4. Healthcheck https://$slug.mynetwk.biz/api/health
```

7.2. Ajouter une cible Makefile `deploy-app` qui appelle ce script.

7.3. Documenter dans le CLAUDE.md d'Atelier : « pour déployer une app, `make deploy-app slug=<x>` depuis CloudMaster ».

### Phase 8 — Mise à jour documentation

8.1. Mettre à jour `/nvme/atelier/CLAUDE.md` :
- Section "Architecture (atteinte 2026-05-09)" → reécrire avec le nouveau topo (Atelier sur Medion).
- "Stockage" : retirer les chemins CloudMaster, mentionner `/ssd_pool/homecloud/data/` pour Files.
- "Build & deploy" : remplacer `make deploy` par `make deploy-app slug=<x>` (push vers Medion).
- "Workflow d'agent" : ajouter une note "Atelier tourne sur Medion désormais ; pour intervenir, SSH Medion ou utiliser l'API distante via app.mynetwk.biz".

8.2. Ajouter une section "État après rapatriement" avec date et description du nouveau setup.

8.3. Mettre à jour `.claude/rules/zero-downtime.md` : la règle ne s'applique plus (plus de phase parallèle).

8.4. Supprimer ou neutraliser `.claude/rules/deploy-chain.md` (les commandes `make deploy` sur CloudMaster ne suffisent plus — adapter au nouveau workflow).

8.5. Garder `.claude/rules/next-plan.md` (hr-flowd reporté) intact.

8.6. Ajouter une nouvelle rule `.claude/rules/path-routing-pending.md` qui mentionne le but initial (path-routing `/apps/{slug}`) comme phase ultérieure, avec les pré-requis identifiés (modif hr-edge, basePath NextJS, etc.).

## Fichiers / chemins critiques à toucher

### Sur CloudMaster (sources)

- [/nvme/atelier/scripts/](/nvme/atelier/scripts/) — créer `stage-app.sh`, `swap-edge-routes.sh`, `deploy-app.sh`
- [/nvme/atelier/Makefile](/nvme/atelier/Makefile) — ajouter cible `deploy-app`, retirer `deploy` local-only
- [/nvme/atelier/CLAUDE.md](/nvme/atelier/CLAUDE.md) — réécriture de l'architecture
- [/nvme/atelier/systemd/](/nvme/atelier/systemd/) — vérifier que `ReadWritePaths` dans `atelier.service` est compatible Medion (`/ssd_pool` n'est PAS dans la liste actuelle, à ajouter si Atelier doit y lire/écrire — mais a priori non, c'est l'app `files` qui y accède directement)
- [/nvme/atelier/.claude/rules/](/nvme/atelier/.claude/rules/) — mises à jour zero-downtime, deploy-chain ; ajout path-routing-pending

### Sur CloudMaster (à arrêter / désinstaller en Phase 6)

- `/etc/systemd/system/atelier.service`
- `/etc/systemd/system/atelier-sync-{state,docs,git,store,runs}.{service,timer}`
- `/opt/atelier/` (archivable)

### Sur Medion (à créer / déployer)

- `/etc/systemd/system/atelier.service`
- `/opt/atelier/{bin/atelier, web/dist/, data/{apps.json,port-registry.json}, .env}`
- `/var/lib/atelier/{state, docs, store, git/repos, apps}`
- `/opt/homeroute/apps/{files,home,myfrigo,trader,wallet,www}/{bin,src/{client|web|api}/dist|static, .env, db.sqlite, runs}`

### Code à ne PAS modifier dans cette session

- Les crates path-deps `hr-common`, `hr-ipc`, `hr-docs`, `hr-apps`, `hr-flow` etc. : aucune modif ici. Le supervisor de `hr-apps` est déjà compatible (lit `ATELIER_APPS_RUNTIME_ROOT`).
- `hr-edge` / `hr-proxy` : pas de changement code, juste injection de routes via API.

## Vérification end-to-end

À exécuter en fin de Phase 4 :

```bash
# 1. Atelier répond
curl -sf https://app.mynetwk.biz/api/health | jq '.status'

# 2. Les 6 apps répondent
for slug in files home myfrigo trader wallet www; do
  curl -s -o /dev/null -w "$slug %{http_code}\n" https://$slug.mynetwk.biz/api/health
done

# 3. Atelier liste les 6 apps en running
curl -s https://app.mynetwk.biz/api/apps | jq '.[] | {slug,state,port}'

# 4. Files data — uploader un fichier de test, le retrouver dans /ssd_pool/homecloud/data/files/
curl -X POST https://files.mynetwk.biz/api/files/upload \
  -F "file=@/tmp/_test-$(date +%s).txt" -F 'path=/_test/'
ssh romain@10.0.0.254 "ls /ssd_pool/homecloud/data/files/_test/"

# 5. Logs propres
ssh romain@10.0.0.254 "journalctl -u atelier -u 'hr-app-*' --since '5 min ago' | grep -iE 'error|warn' | tail -30"

# 6. Postgres connexions locales uniquement
ssh romain@10.0.0.254 "ss -tnp '( dport = :5432 or sport = :5432 )' | head -20"

# 7. Atelier sur CloudMaster est down
systemctl is-active atelier.service  # → inactive
```

## Risques et garde-fous

- **R1 — Files data divergence** : risque qu'une donnée locale CloudMaster pas encore poussée sur le raid0 Medion soit perdue. Mitigation : Phase 4.0 inclut un diff explicite `/opt/homeroute/apps/files/src/data/` (CM) vs `/ssd_pool/homecloud/data/` (Medion) AVANT bascule, avec stop si divergence inattendue.
- **R2 — Postgres dataverse local-only** : Atelier passe de `10.0.0.254:5432` à `127.0.0.1:5432`. Vérifier que `pg_hba.conf` accepte les connexions locales pour `app_*` users (en principe oui).
- **R3 — Build matrix** : les binaires sont compilés sur CloudMaster (Linux x86_64), Medion est aussi Linux x86_64 → ABI compatible. Vérifier `uname -m` sur Medion.
- **R4 — hr-edge route swap atomicité** : si la mise à jour des routes échoue partiellement, certaines apps répondent 502. Mitigation : préparer les routes dans un JSON temporaire et les pousser en bloc.
- **R5 — Atelier sur CloudMaster relancé par accident** (par un timer oublié, par Atelier-restart hook) → conflit avec Atelier sur Medion. Mitigation Phase 6.1 : `disable` avant `stop`.
- **R6 — Le supervisor essaie de re-attacher** d'anciennes unit files `hr-app-*` sur Medion : aucune ne doit pré-exister, mais vérifier `systemctl list-units 'hr-app-*' --all` côté Medion avant Phase 4.3.

## Out of scope (reportés)

- **Path-routing `app.mynetwk.biz/apps/{slug}`** : changement majeur (hr-edge path matcher, NextJS basePath, auth rules) — plan séparé après stabilisation.
- **Migration des sources sur Medion** : non, sources restent sur CloudMaster (workflow code-server local). Si le besoin apparaît plus tard, c'est une décision séparée.
- **Migration des 4 apps NextJS stopped** : non. Si l'utilisateur veut les ressusciter, il faudra une session dédiée (sources à migrer, NextJS standalone build à reproduire).
- **Refacto extraction `homeroute-core`** : non concerné, les path-deps restent.
- **hr-flowd** : reste reporté (cf. `.claude/rules/next-plan.md`).
