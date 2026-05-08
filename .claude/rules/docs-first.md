# Docs-first — Atelier

Pour toute tâche dans une app HomeRoute existante (sous `/opt/homeroute/apps/{slug}/`), le système docs est la source de vérité de l'intention. Lire AVANT de coder.

## Workflow obligatoire

1. **`mcp__homeroute__docs_overview(app_id=<slug>)`** — TOUJOURS EN PREMIER. Renvoie l'overview prose + index compact. Cadre la tâche en peu de tokens.
2. **`mcp__homeroute__docs_search` ou `docs_list_entries`** — pour cibler l'entrée pertinente.
3. **`mcp__homeroute__docs_get`** — lecture détaillée.
4. **`mcp__homeroute__docs_diagram_get`** — si l'entrée a un diagramme mermaid.
5. **Exploration code** — UNIQUEMENT après les 4 étapes ci-dessus.
6. **Modification** — applique les changements.
7. **`mcp__homeroute__docs_update`** — mets à jour les entrées impactées.
8. **`mcp__homeroute__docs_diagram_set`** — régénère le mermaid si le flux a changé.
9. **`mcp__homeroute__docs_completeness`** — vérifie qu'il ne manque ni summary ni diagramme.

> ⚠️ Pendant la phase de migration, le MCP `mcp__homeroute__docs_*` est encore servi par hr-orchestrator sur Medion. Une fois la Phase 2 terminée, Atelier expose `mcp__atelier__docs_*` avec le même contrat. Préférer le nouveau si disponible.

## Pour les nouvelles features Atelier elles-mêmes

Atelier n'a pas (encore) de docs Atelier-internes, mais le code doit être commenté avec discipline :

- Pas de comments redondants ("explain WHAT" — le code le dit déjà)
- Comments uniquement pour le **WHY** non-évident : invariants, workarounds, contraintes externes
- Pas de docstrings multi-paragraphes
