#!/usr/bin/env bash
# Agrège les remontées CLAUDE_ISSUES.json de toutes les apps en un seul tableau
# JSON trié par sévérité (high → low) puis par app. Lecture fichier directe
# (robuste : marche même si l'API Atelier est down).
#
# Usage: collect.sh [open]     # 'open' = ne garder que les soucis status=open
set -euo pipefail
APPS_ROOT="${ATELIER_APPS_RUNTIME_ROOT:-/var/lib/atelier/apps}"
FILTER="${1:-}"

shopt -s nullglob
files=("$APPS_ROOT"/*/src/CLAUDE_ISSUES.json)
if [ "${#files[@]}" -eq 0 ]; then
  echo "[]"
  echo "Aucun CLAUDE_ISSUES.json trouvé sous $APPS_ROOT/*/src/." >&2
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "jq introuvable — requis pour fusionner les fichiers JSON." >&2
  exit 3
fi

jq -s --arg filter "$FILTER" '
  (add // [])
  | (if $filter == "open" then map(select(.status == "open")) else . end)
  | map(. + {sev_rank: ({high:0, medium:1, low:2}[.severity] // 3)})
  | sort_by(.sev_rank, .app, .ts)
  | map({app, severity, area, status, id, title, context, tried})
' "${files[@]}"
