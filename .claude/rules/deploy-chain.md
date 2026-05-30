# Deploy chain — Atelier (post-rapatriement Studio, 2026-05-27)

Atelier (le binaire + frontend) tourne sur **Medion**. Son code source vit encore sur **CloudMaster** (`/nvme/atelier/`), où le `make deploy` build localement puis push vers Medion.

Les **sources des 6 apps HomeRoute** vivent désormais sur **Medion** (`/var/lib/atelier/apps/<slug>/src/`) — éditées via le Studio (`atelier-studio.service` sur 127.0.0.1:8443, exposé via `codeserver.mynetwk.biz`). Le `make deploy-app` build directement sur Medion (local si lancé sur Medion, sinon via SSH).

## Modification du code Atelier (Rust + frontend)

```bash
make deploy        # build local CM + rsync binaire+web/dist Medion + restart atelier.service + healthcheck
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

## Modification d'une app HomeRoute (sources sur Medion)

```bash
make deploy-app SLUG=files   # build sur Medion (local ou SSH) + restart via API + healthcheck
```

Sous le capot ([scripts/deploy-app.sh](../../scripts/deploy-app.sh)) :
1. Lit `build_command` + `stack` + `port` + `health_path` depuis l'API Atelier
2. Si `hostname == medion` : exécute `bash -lc 'cd /var/lib/atelier/apps/<slug>/src && BUILD_CMD'` en local
3. Sinon : `ssh romain@10.0.0.254 'bash -lc "cd ... && BUILD_CMD"'`
4. POST `/api/apps/<slug>/control` action=restart
5. Healthcheck `https://<slug>.mynetwk.biz<health_path>` — exerce la chaîne complète (hr-edge route + auth + app TCP listener). Un `3xx` est accepté car les apps `auth_required: true` redirigent les anonymes vers `/login`.

## Règles absolues

- **JAMAIS** `cargo run` en local — toujours `make deploy` (sinon le binaire ne va pas en prod sur Medion).
- **JAMAIS** modifier les sources des apps depuis CloudMaster (`/opt/homeroute/apps/<slug>/src/`) — ce sont des vestiges figés (snapshot tarball pour rollback). Les sources canoniques sont sur Medion (`/var/lib/atelier/apps/<slug>/src/`), éditées via Studio.
- **TOUJOURS** vérifier le healthcheck dans la sortie du `make deploy*` avant de considérer un déploiement réussi.
- Atelier est autonome depuis le 2026-05-30 : `atelier-common`/`atelier-ipc`/`atelier-docs` sont internalisées (`crates/`), plus aucun path-dep vers `/nvme/homeroute/`. Plus besoin de garder homeroute compilable avant de pousser.

## Fenêtres d'indisponibilité

- `make deploy` : ~5 sec où l'API Atelier est down pendant le restart. Les apps continuent à tourner.
- `make deploy-app SLUG=<x>` : 1-3 sec d'indispo de l'app concernée. Pas d'impact sur les autres.

## Rollback

### Rollback du Studio sur CloudMaster (snapshot 2026-05-27)

```bash
# Re-host Studio sur CM, reroute hr-edge
ssh romain@10.0.0.254 "sudo systemctl stop atelier-studio.service"
sudo systemctl start hr-studio.service                              # sur CM
curl -X POST http://10.0.0.254:4000/api/edge/routes \
  -H 'content-type: application/json' \
  -d '{"domain":"codeserver.mynetwk.biz","target":"10.0.0.10:8443","auth_required":true,"allowed_groups":[],"local_only":false,"app_id":"studio","host_id":"cloudmaster"}'
# Restore sources sur CM (si besoin)
sudo tar xzf /var/backups/cm-studio-snapshot-2026-05-27.tar.gz -C /
```

### Rollback du binaire Atelier (snapshot 2026-05-09)

```bash
ssh romain@10.0.0.254 "sudo tar xzf /var/backups/atelier-cloudmaster-2026-05-09.tar.gz -C /tmp \
  && sudo cp /tmp/opt/atelier/bin/atelier /opt/atelier/bin/atelier \
  && sudo systemctl restart atelier"
```

## Variables Makefile

```bash
MEDION       (default: romain@10.0.0.254)
ATELIER_API  (default: http://10.0.0.254:4100)
```

Override en ligne de commande pour un autre setup :

```bash
make deploy MEDION=romain@10.0.0.20 ATELIER_API=http://10.0.0.20:4100
```
