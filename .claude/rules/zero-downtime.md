# Zero downtime — règle critique pendant la migration

Pendant TOUTES les phases de la migration depuis homeroute (cf. plan en cours), homeroute continue à servir les apps existantes. **Zéro downtime** sur les domaines `{slug}.mynetwk.biz` et `proxy.mynetwk.biz`.

## Ce que ça implique

- Atelier sur CloudMaster ne touche PAS aux processus apps gérés par hr-orchestrator (Medion). Au minimum jusqu'au cutover (Phase 9).
- Atelier peut lire les sources des apps dans `/opt/homeroute/apps/` (CloudMaster, sources canoniques) en read-only.
- Atelier peut éditer le contenu des apps via le Studio code-server (déjà le cas), mais ne pas restart les apps.
- Toute modification d'un crate partagé (`hr-common`, `hr-ipc`, `hr-docs` via path-dep) doit garder homeroute compilable. Tester avec `cd /nvme/homeroute && cargo build --release` avant de pousser.

## Comparaison parité (per feature)

À chaque feature migrée vers Atelier, vérifier sur 24-48h que les deux endpoints retournent la même donnée :

```bash
diff <(curl -s https://proxy.mynetwk.biz/api/<route> | jq -S .) \
     <(curl -s https://app.mynetwk.biz/api/<route> | jq -S .)
```

Si divergence : investiguer, ne pas avancer sur la feature suivante.
