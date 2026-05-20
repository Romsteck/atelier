# Homeroute :4000 — hors périmètre apps

> Depuis le rapatriement (2026-05-09), **toute la partie applicative est servie par Atelier sur port 4100** (apps lifecycle, dataverse, flows, docs, MCP `app.*` / `dv.*` / `docs.*` / `flow.*`). Homeroute (`hr-api`, port 4000) ne sert plus que la partie network (DNS, DHCP, ACME, dashboard, routes edge).

## Symptôme typique du mauvais port

```bash
$ curl -i -X POST http://10.0.0.254:4000/api/apps/www/ship
HTTP/1.1 405 Method Not Allowed
allow: GET,HEAD
```

Le `405` vient du SPA fallback `ServeFile` ([hr-api/src/lib.rs](/nvme/homeroute/crates/api/hr-api/src/lib.rs)) qui répond `allow: GET,HEAD` sur **tous** les paths inexistants — c'est attendu, pas un bug. La route `/api/apps/{slug}/ship` n'existe simplement pas côté homeroute.

## Le bon port

```bash
$ curl -i -X POST http://10.0.0.254:4100/api/apps/www/ship -d '{}'
HTTP/1.1 200 OK
```

Ou, depuis l'extérieur : `https://app.mynetwk.biz/api/...`.

## Règles

1. **Ne pas wirer côté homeroute** les routes `/api/apps/*`, `/api/dv/*`, `/api/flows/*`, `/api/docs/*`, `/api/git/*`. Elles vivent côté Atelier exclusivement.
2. **Si un skill / script appelle `:4000` pour une API apps**, c'est un bug à corriger côté skill (pointer vers `:4100`). Pas côté homeroute.
3. **Le MCP** côté agent : `mcp__atelier__*` (port 4100) pour tout ce qui touche aux apps / dataverse / flows / docs. Le legacy `mcp__homeroute__*` (port 4000) doit progressivement disparaître à mesure que les Studios se reconfigurent.

## Pourquoi cette règle existe

Un agent qui voit `405 Method Not Allowed allow: GET,HEAD` sur `:4000/api/apps/...` peut perdre du temps à investiguer un bug imaginaire côté homeroute (routes manquantes, méthodes mal câblées, etc.). Cette rule documente que **le 405 est la réponse correcte** et oriente vers le bon port (`:4100`).
