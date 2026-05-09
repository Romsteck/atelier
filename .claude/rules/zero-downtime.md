# Zero downtime — règle obsolète

> **Cette règle ne s'applique plus depuis le rapatriement Atelier sur Medion (2026-05-09).** Conservée pour mémoire.

Auparavant (Phase 9 cutover Medion→CloudMaster), la règle protégeait contre le double pilotage des apps pendant la transition. Plus de phase parallèle aujourd'hui : Medion est l'unique hôte des apps + Atelier supervisor.

## Règles toujours valables (héritage)

- Toute modification d'un crate partagé encore en path-dep vers `/nvme/homeroute/crates/shared/` (`hr-common`, `hr-ipc`, `hr-docs`) doit garder homeroute compilable. Tester avec `cd /nvme/homeroute && cargo build --release` avant de pousser.
- Pendant un déploiement Atelier (`make deploy`), prévoir une fenêtre de ~5 secondes où l'API Atelier est down pendant le restart. Les apps elles-mêmes continuent à tourner (transient systemd units).
- Pendant un `make deploy-app SLUG=<x>`, l'app concernée est restart (1-3 sec d'indisponibilité du domaine). Pas d'impact sur les autres.

## Pour rollback éventuel

En cas de problème post-déploiement, deux possibilités :

```bash
# 1. Rollback du binaire Atelier sur Medion
ssh romain@10.0.0.254 "sudo tar xzf /var/backups/atelier-cloudmaster-2026-05-09.tar.gz -C /tmp && sudo cp /tmp/opt/atelier/bin/atelier /opt/atelier/bin/atelier && sudo systemctl restart atelier"

# 2. Rollback total vers CloudMaster (si Medion impossible à remonter)
# scripts/swap-edge-routes.sh cloudmaster --apply
# (puis restaurer /opt/atelier sur CloudMaster depuis l'archive et restart)
```
