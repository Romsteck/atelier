# 🚧 Path-routing — phase ultérieure

> **Le but initial de la séparation Studio→Atelier était de servir les apps via path-based routing** : `https://app.mynetwk.biz/apps/{slug}` au lieu de `https://{slug}.mynetwk.biz`. Cette transformation est **explicitement reportée** ; à reprendre une fois le rapatriement Medion stabilisé.

## Pourquoi reporté ?

Le path-routing demande un changement coordonné sur 4 couches :
1. **hr-edge / hr-proxy** : matcher hostname-only aujourd'hui (`find_route(host)` dans `crates/edge/hr-proxy/src/handler.rs`). Faut ajouter une couche de path-matching pour `/apps/{slug}`.
2. **NextJS apps** (`www`, plus tard aptymus/calendar/forge/padel) : ajouter `basePath: "/apps/{slug}"` dans `next.config.{ts,js}` au build. Sinon les chemins relatifs CSS/JS (`/_next/static/...`) cassent.
3. **Apps Rust/Axum** : utiliser `Router::nest("/apps/{slug}", ...)` (pris en charge nativement par axum). Vérifier les redirections internes (`/login` → `/apps/<slug>/login`).
4. **Auth** : forward-auth actuel matche par `domain_only`. Faut étendre pour matcher path + hostname.

Le rapatriement Medion (2026-05-09) ne touche pas à ces couches : routes restent `{slug}.mynetwk.biz → 127.0.0.1:port`. Reprendre path-routing ensuite.

## Trigger de reprise

Quand l'écosystème est stable (≥1 semaine post-rapatriement, pas de régression majeure), annoncer :

> "Le rapatriement Medion est stable depuis N jours. On peut maintenant attaquer le path-routing `app.mynetwk.biz/apps/{slug}` (cf. .claude/rules/path-routing-pending.md)."

## Plan d'attaque (esquisse, à raffiner)

### Phase A — hr-edge supporte le path-routing

Modifier `crates/edge/hr-proxy/src/handler.rs` :
- Si `host == "app.mynetwk.biz"` ET `path` matche `/apps/{slug}/...` → router vers `127.0.0.1:port-de-{slug}` après strip prefix `/apps/{slug}`.
- Sinon comportement actuel.

Ajouter une struct `PathRoute { prefix, target_ip, target_port, strip_prefix: bool }` à côté de `AppRoute`.

### Phase B — basePath par app

Pour chaque app NextJS (à commencer par `www`) :
1. Ajouter `basePath: "/apps/<slug>"` dans `next.config.ts`.
2. Vérifier visuellement dans le navigateur que toutes les pages se chargent à `https://app.mynetwk.biz/apps/<slug>/`.
3. Revoir tout `Link href="/..."` codé en dur : remplacer par `<Link href="/...">` (NextJS appliquera le basePath automatiquement) ou par usage des helpers.

Pour les apps Axum :
1. Vérifier que `Router::new().nest("/apps/<slug>", ...)` fonctionne.
2. Si l'app sert des assets statiques en chemin absolu (`/static/...`), les passer en relatif ou utiliser un router enfant.

### Phase C — Auth path-aware

Étendre `check_forward_auth` (cf. `crates/edge/hr-proxy/src/handler.rs:380`) pour matcher `(domain, path_prefix)` au lieu de juste `domain`.

### Phase D — Migration progressive

Une app à la fois :
1. Activer le path-route (ajout dans hr-edge + apps).
2. Garder le hostname-route en parallèle pendant 24h.
3. Couper le hostname-route, ne laisser que le path.

## Garde-fous

- Ne pas créer de route `app.mynetwk.biz/apps/{slug}` tant que l'app n'a pas été rebuildée avec `basePath`. Sinon : page blanche / 404 sur tous les assets.
- Garder le hostname-route en backup pendant le deploy, à supprimer après confirmation.
- L'auth doit fonctionner dès le début sur les paths (pas de phase où le path est non-protégé).
