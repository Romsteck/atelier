# Deploy chain — Atelier

À chaque modification du code Rust ou du frontend :

1. Build : `make atelier` (et `make web` si frontend touché)
2. Restart : `make deploy` (qui restart le systemd local `atelier.service`)
3. Vérification : `curl https://app.mynetwk.biz/api/health` doit répondre 200
4. Logs : `journalctl -u atelier --since '1 min ago' | tail -50`

Atelier tourne sur **CloudMaster** (la machine de dev). Pas de rsync cross-host. Le binaire compile en local, redémarre en local.

**Règle absolue** : pas de `cargo run` direct. Toujours via systemd (analogue à `homeroute.service`) pour ne pas dévier de la prod.

## Commandes

```bash
make atelier     # cargo build --release -p atelier
make web         # npm run build dans web/ (Phase 2+)
make deploy      # build all + systemctl restart + healthcheck
make logs        # journalctl -u atelier -f
```
