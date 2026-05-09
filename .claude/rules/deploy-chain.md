# Deploy chain — Atelier (post-rapatriement, 2026-05-09)

Atelier tourne sur **Medion** (`romain@10.0.0.254`), code-server pour le dev reste sur **CloudMaster**. Le build se fait sur CloudMaster, les artefacts sont push vers Medion.

## Modification du code Atelier (Rust + frontend)

```bash
make deploy        # build local + rsync binaire+web/dist Medion + restart atelier.service + healthcheck
```

Sous le capot :
1. `cargo build --release -p atelier` (CloudMaster)
2. `npm run build` dans `web/` (CloudMaster)
3. `rsync` du binaire vers `/opt/atelier/bin/atelier.new` sur Medion + atomic rename
4. `rsync` de `web/dist/` vers `/opt/atelier/web/dist/` sur Medion
5. `ssh romain@10.0.0.254 sudo systemctl restart atelier.service`
6. `curl -fsS http://10.0.0.254:4100/api/health`

Vérification supplémentaire :

```bash
make logs          # tail journalctl -u atelier sur Medion via SSH
```

## Modification d'une app HomeRoute (sources sur CloudMaster)

```bash
make deploy-app SLUG=files   # build + rsync app + restart via Atelier API + healthcheck
```

Sous le capot ([scripts/deploy-app.sh](../../scripts/deploy-app.sh)) :
1. Lit `build_command` + `stack` depuis l'API Atelier
2. Exécute le `build_command` dans `/opt/homeroute/apps/<slug>/src/` (CloudMaster)
3. Rsync stack-aware vers `/var/lib/atelier/apps/<slug>/` (Medion)
   - Les apps **NextJS** : on inclut `/src/node_modules/` (le `.next/standalone/node_modules/` est incomplet)
   - Les apps **Rust** : on exclut `/src/{target,node_modules}/` (binaire compilé suffit)
4. POST `/api/apps/<slug>/control` action=restart
5. `curl -s -o /dev/null -w "%{http_code}" https://<slug>.mynetwk.biz<health_path>`

## Règles absolues

- **JAMAIS** `cargo run` en local — toujours `make deploy` (sinon le binaire ne va pas en prod sur Medion).
- **JAMAIS** modifier les sources des apps sur Medion (`/var/lib/atelier/apps/`) — c'est de l'artefact, pas une source. Les sources canoniques sont sur CloudMaster (`/opt/homeroute/apps/<slug>/src/`).
- **TOUJOURS** vérifier le healthcheck dans la sortie du `make deploy*` avant de considérer un déploiement réussi.

## Variables Makefile

```bash
MEDION       (default: romain@10.0.0.254)
ATELIER_API  (default: http://10.0.0.254:4100)
```

Override en ligne de commande pour un autre setup :

```bash
make deploy MEDION=romain@10.0.0.20 ATELIER_API=http://10.0.0.20:4100
```
