# 🚧 Path-routing — couche interne LIVE, intégration hr-edge pendante

> **MAJ 2026-06-15** : le path-routing **interne à Atelier est LIVE**. Les apps sont servies en même-origine sous `http://127.0.0.1:4100/apps/{slug}/` (`crates/atelier-api/src/routes/apps_proxy.rs`), `www` en mode no-strip avec `basePath:/apps/www` (autres slugs no-strip via `ATELIER_PRESERVE_PREFIX_SLUGS`). Les sous-domaines `{slug}.mynetwk.biz` sont **morts** (404). Ce qui reste pendant : l'intégration **côté hr-edge** (hostname public + path) et l'**auth path-aware**.

## État par couche

1. **hr-edge / hr-proxy** — ⏳ PENDANT : matcher hostname-only aujourd'hui (`find_route(host)` dans `crates/edge/hr-proxy/src/handler.rs`). Pas encore de path-matching `/apps/{slug}` côté edge ; l'accès externe passe par `atelier.mynetwk.biz` (le frontend Atelier proxifie en interne).
2. **NextJS apps** — ✅ FAIT pour `www` (`basePath:/apps/www` dans `next.config.ts`, servi no-strip par le path-proxy). À refaire par app NextJS future.
3. **Apps Rust/Axum** — ✅ servies en mode strip par le path-proxy (préfixe `/apps/{slug}` retiré avant proxy) ; pas de `Router::nest` requis côté app.
4. **Auth** — ⏳ PENDANT : forward-auth hr-edge matche par `domain_only` ; à étendre pour matcher path + hostname quand on exposera `/apps/{slug}` publiquement.

## Trigger de reprise

La couche interne est faite. Le trigger restant concerne l'**exposition publique** par path. Quand on veut router `atelier.mynetwk.biz/apps/{slug}` (ou un autre hostname) avec auth path-aware au lieu de tout passer par le frontend Atelier, attaquer les Phases A (hr-edge path-matching) + C (auth path-aware) ci-dessous.

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
