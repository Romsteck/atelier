# Refonte www

## État

- Statut : DONE (deployé + smoke tests passent)
- Démarré : 2026-05-26
- Terminé : 2026-05-26
- Stack : Next.js (TypeScript)
- Branche atelier : eradication-flows
- Branche app : pas créée (modifs directes sur `/opt/homeroute/apps/www/src/`)

## Inventaire

### Flows TOML existants (3)

| TOML | Appelé par | Décision |
|---|---|---|
| `handle_contact_request.toml` | `app/api/contact/route.ts:45` | Refonte en service TS |
| `open_contact_request.toml` | `app/api/admin/contact-requests/[id]/route.ts:19` | Refonte en service TS |
| `get_or_create_legal_page.toml` | `app/api/legal/[page]/route.ts:50` | Refonte en service TS |

### Mapping flow → service natif

| Flow | Service TS cible | Fichier |
|---|---|---|
| handle_contact_request | `handleContactRequest(input)` | `lib/services/contact.ts` |
| open_contact_request | `openContactRequest(id)` | `lib/services/contact.ts` |
| get_or_create_legal_page | `getOrCreateLegalPage(page, defaultSections)` | `lib/services/legal.ts` |

## Suppression — DONE

- [x] `lib/flow/` supprimé (dv-connector.ts, handler.ts, invoke.ts)
- [x] `app/%5Fflow/` (callback routes) supprimé
- [x] `flows/` (3 TOML) supprimé
- [x] `package.json` : `@homeroute/flow-action` retiré
- [x] `node_modules/@homeroute/flow-action` nettoyé localement
- [x] `npx tsc --noEmit` vert
- [x] `npm run build` vert
- [x] `grep -rn "runFlow\|@/lib/flow\|@homeroute/flow-action\|FlowError"` vide

## Intégration logging — DIFFÉRÉE

Même décision que files : on diffère à une sub-phase post-Phase 3. www continue de loguer stdout → journalctl pour l'instant.

## Vérification (deploy) — DONE

- [x] Autorisation deploy reçue
- [x] `CI=true make deploy-app SLUG=www` succès build+rsync+restart
- [x] App active sur Medion (PID 655159, 96 MB)
- [x] Loopback `/apps/www/api/contact-types` → 200 (preuve que Next.js standalone tourne)
- [x] Loopback `/apps/www/api/legal/mentions-legales` → 200 + payload structuré (preuve `getOrCreateLegalPage` fonctionne)
- [x] Loopback POST `/apps/www/api/contact` avec `object="meeting"` → 201 (preuve `handleContactRequest` fonctionne)
- [x] Entrée DV `contact_requests` id=17 créée puis nettoyée
- [ ] Healthcheck `make deploy-app` a échoué (HTTP 000) car `https://www.mynetwk.biz/api/health` est cassé par le quirk pré-existant **path-routing-pending** (cf. `.claude/rules/path-routing-pending.md`) : `basePath: "/apps/www"` est configuré dans `next.config.ts` (commit 8807f70 du 2026-05-09) mais hr-edge n'est pas encore path-aware → toutes les routes publiques retournent 404. Indépendant de la refonte.

## Reverify J+1

- [x] Pas d'erreur 24h (J+1 2026-05-27)
- [x] Métriques DB cohérentes (contact_requests, legal_contents)
- [x] Pas de régression remontée

## Notes

- Le webhook (POST Azure) n'a pas été testé en smoke (env `AZURE_WEBHOOK_URL` non set, ce qui est le comportement par défaut en local et prod actuelle). La branche est triviale et préservée : si webhook fail (transport ou non-2xx), on update `email_error`; si 2xx, on set `email_sent=true`. Comportement équivalent au TOML.
- `openContactRequest` non testé via l'admin endpoint (besoin d'un cookie admin). Logique triviale (get + update si status="new"), types alignés sur le shape du flow original.
- Path-routing quirk : reprendre dans `path-routing-pending.md` plan.
