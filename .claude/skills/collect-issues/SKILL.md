---
name: collect-issues
description: Agrège et trie les soucis PLATEFORME remontés par les apps HomeRoute (store central Atelier `atelier_meta.platform_issues`, alimenté par les chats Studio via /api/apps/{slug}/issues) pour les traiter en session dev Atelier. Utilise-moi quand l'utilisateur veut voir / trier / traiter les remontées des apps.
allowed-tools: Bash(bash .claude/skills/collect-issues/collect.sh*)
---

# Collecter les remontées plateforme des apps

Les chats Claude Code des apps remontent leurs frictions **plateforme** (tool MCP, doc, build/deploy, dataverse, agent) via la skill `0-report-issue`, qui appelle `POST /api/apps/{slug}/issues`. Atelier les **centralise dans son control-plane** (Postgres `atelier_meta.platform_issues`) — il n'y a **plus de fichier `CLAUDE_ISSUES.json` au niveau des projets**. Cette skill interroge le store central pour que tu les traites ici, en développant Atelier.

## Collecter

```bash
bash .claude/skills/collect-issues/collect.sh        # tout
bash .claude/skills/collect-issues/collect.sh open   # uniquement status=open
```

Sous le capot : `GET /api/issues` (tri serveur par sévérité `high` → `medium` → `low` puis par app). Sortie : un tableau JSON, chaque entrée projetée sur `app`, `severity`, `area`, `status`, `id`, `title`, `context`, `tried`.

## Traiter

1. Lis la synthèse, priorise par `severity`.
2. Pour chaque souci pertinent : reproduis / comprends la cause, corrige **à la racine** dans le code d'Atelier (`crates/…`, `web/…`, `runner/…`), `make deploy`, vérifie.
3. Marque la remontée comme traitée via l'API Atelier. Les endpoints de triage sont **platform-level** (l'`id` est globalement unique, **plus besoin du slug**) :

   ```bash
   # résolu
   curl -sS -X PATCH http://127.0.0.1:4100/api/issues/<id> \
     -H 'content-type: application/json' -d '{"status":"resolved","note":"<commit/explication>"}'
   # ou purge
   curl -sS -X DELETE http://127.0.0.1:4100/api/issues/<id>
   ```

   `dismissed` (faux positif / hors périmètre) : `{"status":"dismissed","note":"…"}`.

4. Récap à l'utilisateur : N résolus, M dismiss, K restants ouverts.

## Notes

- Le `slug` de chaque entrée est dans le champ `app` (info, plus requis pour le triage).
- Tout passe par l'API Atelier (:4100) : collecte (`GET /api/issues`) **et** clôture (`PATCH`/`DELETE /api/issues/{id}`). Si Postgres est down, l'API renvoie une erreur.
