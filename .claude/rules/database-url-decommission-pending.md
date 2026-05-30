# 🚧 Décommission de l'accès Postgres direct (`DATABASE_URL`) — phase ultérieure

> La migration vers **postgres-dataverse** est finalisée côté Atelier (moteur SQLite supprimé le 2026-05-30, cf. CLAUDE.md « Migration postgres-dataverse — finalisée »). Le **plan long terme** est de bannir l'accès Postgres direct des apps : ne garder que la passerelle (`/api/dv/{slug}` + tools `dv_*`). Cette coupure est **explicitement reportée**.

## Pourquoi reporté ?

Chaque app reçoit aujourd'hui, en plus des credentials passerelle (`HR_DV_BASE_URL` / `HR_DV_TOKEN` / `HR_APP_UUID`), une `DATABASE_URL` (DSN Postgres direct) injectée à chaque boot par `sync_dv_env` (`crates/atelier-api/src/mcp/apps_ops.rs`). Les apps **lisent encore** ce DSN via `sqlx` :

| App | usages `sqlx` directs (≈) |
|-----|---------------------------|
| trader | 283 |
| myfrigo | 27 |
| files | 3 |
| home | 3 |
| www | 1 (surtout passerelle, 139 usages `HR_DV_*`) |
| wallet | 1 |

Couper `DATABASE_URL` (ou révoquer `LOGIN` sur le rôle PG) **maintenant** ferait planter trader/myfrigo et probablement toutes les apps à la première requête DB → régression utilisateur directe.

## Prérequis (par app)

1. Réécrire l'app pour passer de `sqlx` direct (`DATABASE_URL`) à la passerelle (`HR_DV_*` + client typé `dv-{slug}`).
2. Déployer la version « passerelle uniquement » et **vérifier e2e** tous les endpoints DB de l'app.
3. Confirmer qu'il ne reste **aucune** lecture de `DATABASE_URL` dans le code de l'app.

## Trigger de reprise

Quand les 6 apps n'ont plus aucune référence `sqlx`/`DATABASE_URL` directe (refacto passerelle-only terminé et vérifié app par app), annoncer :

> « Les 6 apps sont en gateway-only. On peut maintenant bannir l'accès PG direct : retirer l'injection `DATABASE_URL` dans `sync_dv_env` + révoquer `LOGIN` sur les rôles PG `app_{slug}`. »

## Plan d'attaque (esquisse)

1. **Une app à la fois** : migrer en passerelle-only, déployer, vérifier e2e, garder `DATABASE_URL` en parallèle 24 h, puis confirmer.
2. Une fois **toutes** les apps migrées :
   - Retirer la ligne `upsert_env_var(&env_path, "DATABASE_URL", &secret.dsn)` de `sync_dv_env` et purger `DATABASE_URL` des `.env`.
   - `REVOKE LOGIN` (ou `ALTER ROLE … NOLOGIN`) sur chaque rôle `app_{slug}` côté Postgres-dataverse.

## Garde-fou

- **NE JAMAIS** révoquer `LOGIN` ni retirer `DATABASE_URL` tant qu'une seule app lit encore le DSN direct. Vérifier avec un `grep -r 'DATABASE_URL\|sqlx' /var/lib/atelier/apps/<slug>/src` avant toute coupure.
- Migrer **une app à la fois**, jamais en masse.
