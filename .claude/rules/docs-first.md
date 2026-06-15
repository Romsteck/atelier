# Docs-first — Atelier

Pour toute tâche dans une app HomeRoute existante (sources sur Medion à `/var/lib/atelier/apps/{slug}/src/`, éditées via Studio), le système docs est la source de vérité de l'intention. Lire AVANT de coder.

## Workflow obligatoire

1. **`mcp__studio__docs_overview(app_id=<slug>)`** — TOUJOURS EN PREMIER. Renvoie l'overview prose + index compact. Cadre la tâche en peu de tokens.
2. **`mcp__studio__docs_search` ou `docs_list_entries`** — pour cibler l'entrée pertinente.
3. **`mcp__studio__docs_get`** — lecture détaillée.
4. **`mcp__studio__docs_diagram_get`** — si l'entrée a un diagramme mermaid.
5. **Exploration code** — UNIQUEMENT après les 4 étapes ci-dessus.
6. **Modification** — applique les changements.
7. **`mcp__studio__docs_update`** — mets à jour les entrées impactées.
8. **`mcp__studio__docs_diagram_set`** — régénère le mermaid si le flux a changé.
9. **`mcp__studio__docs_completeness`** — vérifie qu'il ne manque ni summary ni diagramme.

> Le MCP `studio` (servi par Atelier sur Medion à `http://127.0.0.1:4100/mcp`, scope par app via `?project={slug}`) est exposé dans chaque workspace code-server via `.mcp.json` ; les tools `mcp__studio__docs_*` sont auto-approuvés en lecture. (Le port 4001 est une référence legacy hr-orchestrator morte ; aucun serveur n'écoute dessus.)

## Pour les nouvelles features Atelier elles-mêmes

Atelier n'a pas (encore) de docs Atelier-internes, mais le code doit être commenté avec discipline :

- Pas de comments redondants ("explain WHAT" — le code le dit déjà)
- Comments uniquement pour le **WHY** non-évident : invariants, workarounds, contraintes externes
- Pas de docstrings multi-paragraphes
