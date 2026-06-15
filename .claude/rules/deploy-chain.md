# Deploy chain — Atelier (source rapatriée sur Medion, 2026-05-31)

Atelier (binaire + frontend) **et son code source** vivent sur **Medion**. Le source est à `/home/romain/atelier`, édité via `code-server@romain.service` (127.0.0.1:8081). `make deploy` build **en place** et installe localement dans `/opt/atelier` — plus aucun cross-build/rsync vers un hôte distant. CloudMaster est décommissionné.

Les **sources des 5 apps HomeRoute** (www, home, trader, wallet, myfrigo — `files` décommissionnée le 2026-05-31) vivent sur **Medion** (`/var/lib/atelier/apps/<slug>/src/`) — éditées via le Studio (`atelier-studio.service` sur 127.0.0.1:8443, exposé via `codeserver.mynetwk.biz`). Le `make deploy-app` build directement sur Medion.

## Modification du code Atelier (Rust + frontend)

```bash
make deploy        # build + install /opt/atelier + restart atelier.service + healthcheck (en place)
```

Sous le capot (sur Medion) :
1. `cargo build --release -p atelier`
2. `npm ci` (si besoin) + `npm run build` dans `web/` (web/.npmrc porte `legacy-peer-deps=true`)
3. `make runner` : `npm ci --omit=dev` du runner Node (Agent SDK). ⚠️ **JAMAIS `--omit=optional`** — le binaire natif `@anthropic-ai/claude-agent-sdk-linux-x64` est une optional-dep, sans lui le runner échoue au runtime (garde-fou Makefile).
4. `sudo install` du binaire → `/opt/atelier/bin/atelier.new` + `mv -f` atomique
5. `sudo rsync --delete web/dist/` → `/opt/atelier/web/dist/`
6. `sudo rsync` du crate `atelier-logging-shipper` → `/opt/atelier/crates/atelier-logging-shipper/` (path-dep absolu de plusieurs apps)
7. `sudo rsync` du runner → `/opt/atelier/runner/{src,node_modules,package*.json,.npmrc}` (exécuté en `hr-studio`)
8. `sudo systemctl restart atelier.service`
9. `curl -fsS http://127.0.0.1:4100/api/health`

> Lancé hors Medion, `make deploy` bascule sur le fallback `deploy-remote` (build local + rsync/SSH vers `$MEDION`). Nécessite `sudo` sans mot de passe sur Medion pour `install`/`rsync`/`mv`/`systemctl restart atelier.service`.

Vérification supplémentaire :

```bash
make logs          # tail journalctl -u atelier (local sur Medion)
```

## Modification d'une app HomeRoute (sources sur Medion)

```bash
make deploy-app SLUG=home   # build sur Medion + restart via API + healthcheck
```

Sous le capot ([scripts/deploy-app.sh](../../scripts/deploy-app.sh)) :
1. Lit `build_command` + `stack` + `port` + `health_path` depuis l'API Atelier
2. `hostname == medion` → `bash -lc 'cd /var/lib/atelier/apps/<slug>/src && BUILD_CMD'` en local
3. POST `/api/apps/<slug>/control` action=restart
4. Healthcheck via le **path-proxy local** `http://127.0.0.1:4100/apps/<slug><health_path>` (commit `bf1e3a8`, 2026-06-13 — exerce le proxy Atelier + le listener TCP de l'app ; les hostnames `<slug>.mynetwk.biz` sont morts). Un `3xx` est accepté car les apps `auth_required: true` redirigent les anonymes vers `/login`.

## Règles absolues

- **JAMAIS** `cargo run` en local — toujours `make deploy` (install dans `/opt/atelier`).
- **TOUJOURS** vérifier le healthcheck dans la sortie du `make deploy*` avant de considérer un déploiement réussi.
- Atelier est autonome depuis le 2026-05-30 : `atelier-common`/`atelier-ipc`/`atelier-docs` sont internalisées (`crates/`), plus aucun path-dep vers `/nvme/homeroute/`.

## Fenêtres d'indisponibilité

- `make deploy` : ~5 sec où l'API Atelier est down pendant le restart. Les apps continuent à tourner.
- `make deploy-app SLUG=<x>` : 1-3 sec d'indispo de l'app concernée. Pas d'impact sur les autres.

## Rollback

Le binaire et le frontend précédents restent installés dans `/opt/atelier` jusqu'au prochain `make deploy`. Pour revenir en arrière : `git checkout <commit>` dans `/home/romain/atelier` puis `make deploy`. L'historique git complet est sur Medion + poussé sur `origin` (github.com/Romsteck/atelier).

> CloudMaster est décommissionné : les anciens rollbacks vers CM (snapshots `cm-studio-snapshot-*`, `atelier-cloudmaster-*`) ne sont plus applicables.

## Variables Makefile

```bash
MEDION       (default: romain@10.0.0.254)   — cible SSH du fallback deploy-remote uniquement
ATELIER_API  (default: http://127.0.0.1:4100)
PREFIX       (default: /opt/atelier)
```
