# Testing — Atelier

Après chaque déploiement, tester de bout en bout les endpoints touchés.

## Après chaque deploy

```bash
# 1. Service répond
curl -s https://app.mynetwk.biz/api/health | jq

# 2. Endpoints touchés répondent (200/201/400/404 attendus)
curl -s https://app.mynetwk.biz/api/<route> | jq
curl -s -o /dev/null -w "%{http_code}\n" https://app.mynetwk.biz/api/<route>

# 3. Logs propres
ssh romain@10.0.0.254 "sudo journalctl -u atelier --since '1 min ago' | grep -iE 'error|warn' | tail -20"
```

## Données de test

- Préfixer les ressources créées par `_test-` ou `_tmp-`
- Toujours nettoyer après vérification
- JAMAIS modifier les apps existantes en prod pour tester
- JAMAIS supprimer/altérer une donnée appartenant à une app utilisateur
