---
name: collect-issues
description: Agrège et trie les soucis PLATEFORME remontés par les apps HomeRoute (fichiers CLAUDE_ISSUES.json écrits par les chats Studio via /api/apps/{slug}/issues) pour les traiter en session dev Atelier. Utilise-moi quand l'utilisateur veut voir / trier / traiter les remontées des apps.
allowed-tools: Bash(bash .claude/skills/collect-issues/collect.sh*)
---

# Collecter les remontées plateforme des apps

Les chats Claude Code des apps remontent leurs frictions **plateforme** (tool MCP, doc, build/deploy, dataverse, agent) via la skill `0-report-issue`, qui appelle `POST /api/apps/{slug}/issues`. Atelier écrit chaque entrée dans `CLAUDE_ISSUES.json` à la racine du source de l'app (`/var/lib/atelier/apps/{slug}/src/CLAUDE_ISSUES.json`). Cette skill agrège tous ces fichiers pour que tu les traites ici, en développant Atelier.

## Collecter

```bash
bash .claude/skills/collect-issues/collect.sh        # tout
bash .claude/skills/collect-issues/collect.sh open   # uniquement status=open
```

Sortie : un tableau JSON fusionné, trié par sévérité (`high` → `medium` → `low`) puis par app. Chaque entrée : `app`, `severity`, `area`, `status`, `id`, `title`, `context`, `tried`.

## Traiter

1. Lis la synthèse, priorise par `severity`.
2. Pour chaque souci pertinent : reproduis / comprends la cause, corrige **à la racine** dans le code d'Atelier (`crates/…`, `web/…`, `runner/…`), `make deploy`, vérifie.
3. Marque la remontée comme traitée via l'endpoint Atelier (le fichier est écrit par Atelier, **pas à la main**) :

   ```bash
   # résolu
   curl -sS -X PATCH http://127.0.0.1:4100/api/apps/<slug>/issues/<id> \
     -H 'content-type: application/json' -d '{"status":"resolved","note":"<commit/explication>"}'
   # ou purge
   curl -sS -X DELETE http://127.0.0.1:4100/api/apps/<slug>/issues/<id>
   ```

   `dismissed` (faux positif / hors périmètre) : `{"status":"dismissed","note":"…"}`.

4. Récap à l'utilisateur : N résolus, M dismiss, K restants ouverts.

## Notes

- Le `slug` de chaque entrée est dans le champ `app`.
- Si l'API est down, la collecte (lecture fichier) marche quand même ; seul le `PATCH`/`DELETE` de clôture nécessite l'API.
