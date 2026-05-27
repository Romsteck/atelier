# Phase 0 — Préconditions

## État

- Statut : DONE
- Démarré : 2026-05-26
- Terminé : 2026-05-26

## Tâches

### Git (DONE)

- [x] Stash des modifs uncommit pré-éradication (`pre-eradication-stash-2026-05-26`)
- [x] Tag `pre-eradication-2026-05-26` sur `main` de `/nvme/atelier`
- [x] Branche `eradication-flows` créée sur `/nvme/atelier`
- [ ] Tag + branche sur chaque source app `/opt/homeroute/apps/{slug}/src/` (6 apps) — à faire au moment de la Phase 2.x correspondante (chaque app indépendamment)

### Build de référence

- [x] Atelier (`-p atelier`) : `cargo build --release -p atelier` vert (57s, binaire produit `target/release/atelier`)
- [x] Web : `npm run build` vert (8s, `web/dist` produit)
- [ ] Apps Rust (sources `/opt/homeroute/apps/{slug}/src/`) — vérifié au moment de chaque Phase 2.x
- [ ] App www (NextJS) — vérifié en Phase 2.2

> ⚠ Bug pré-existant signalé : `cargo build --workspace --release` casse sur `hr-dataverse-migrate` (variant `FieldType::Money` non couvert dans un match à la ligne 568 de `src/lib.rs`). Pas dans notre périmètre, le binaire `atelier` n'en dépend pas (crate CLI séparée pour les migrations). À corriger plus tard.
>
> Note env : `CARGO_HOME=/opt/rust/cargo` (defaut système) n'est pas writable par l'user `romain`. Builds locaux doivent utiliser `CARGO_HOME=/home/romain/.cargo`.

### État runtime Medion (snapshot avant)

État au 2026-05-26 : tous les services sont `active` :
- `atelier.service` : active
- `hr-flowd.service` : active
- `atelier-app-{files,home,myfrigo,trader,wallet,www}.service` : tous active

### Snapshots / Backups

- [ ] `pg_dump` instance Postgres dataverse (apps DBs + `_dv_audit`) — à valider avec l'utilisateur sur le où/quand
- [ ] `tar` des `/var/lib/atelier/apps/*/runs/` (archive de réf) — à faire avant Phase 3 (teardown)

### Préparation env (Phase 1)

- [ ] Générer `ATELIER_LOGS_TOKEN` (32 bytes b64) — à faire en début Phase 1
- [ ] Pré-injecter dans `/opt/atelier/.env` sur Medion — Phase 1

## Notes

Décision : tags + branches per-app reportés au début de chaque refonte d'app (Phase 2.x). Évite le travail de masse sur 6 repos avant que la méthode soit validée sur le pilote (files).

Décision : pg_dump différé — il sera plus utile juste avant Phase 3 (teardown) que maintenant. Pour la refonte des apps, l'audit `_dv_audit` continue de tourner et fournit la traçabilité des mutations.

## Critère DONE Phase 0

- Git baseline en place sur `/nvme/atelier` ✓
- Build de réf Atelier vert ☐
- État runtime Medion connu ☐
- Plan de la suite confirmé avec l'utilisateur ☐
