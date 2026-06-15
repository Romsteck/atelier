# Testing — Atelier

Après chaque déploiement, tester de bout en bout les endpoints touchés.

## Après chaque deploy

```bash
# Hostname externe = atelier.mynetwk.biz (PLUS app.mynetwk.biz : route edge morte).
# En local sur Medion : http://127.0.0.1:4100. Une app se teste via le path-proxy
# (/apps/<slug>/...), pas via <slug>.mynetwk.biz (sous-domaines morts).

# 1. Service répond
curl -s http://127.0.0.1:4100/api/health | jq            # ou https://atelier.mynetwk.biz/api/health

# 2. Endpoints touchés répondent (200/201/400/404 attendus)
curl -s http://127.0.0.1:4100/api/<route> | jq
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:4100/api/<route>
# App via path-proxy (sans auth en local) :
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:4100/apps/<slug>/<health_path>

# 3. Logs propres (en local sur Medion ; sinon ssh romain@10.0.0.254 "...")
sudo journalctl -u atelier --since '1 min ago' | grep -iE 'error|warn' | tail -20
```

## Données de test

- Préfixer les ressources créées par `_test-` ou `_tmp-`
- Toujours nettoyer après vérification
- JAMAIS modifier les apps existantes en prod pour tester
- JAMAIS supprimer/altérer une donnée appartenant à une app utilisateur
