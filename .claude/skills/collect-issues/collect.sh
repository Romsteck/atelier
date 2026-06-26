#!/usr/bin/env bash
# Récupère les remontées plateforme (CLAUDE_ISSUES) depuis le store CENTRAL
# d'Atelier (control-plane Postgres `atelier_meta.platform_issues`) via l'API.
# Le tri (sévérité high→low, puis app) et le filtre status sont déjà faits côté
# serveur ; jq ne sert qu'à projeter les champs pour l'affichage.
#
# Usage: collect.sh [open]     # 'open' = ne garder que les soucis status=open
set -euo pipefail
API_BASE="${ATELIER_API:-http://127.0.0.1:4100}"
FILTER="${1:-}"

if ! command -v jq >/dev/null 2>&1; then
  echo "jq introuvable — requis pour formater la sortie JSON." >&2
  exit 3
fi

url="$API_BASE/api/issues"
[ "$FILTER" = "open" ] && url="$url?status=open"

resp="$(curl -sS --max-time 10 "$url")" || {
  echo "Échec de l'appel à $url — l'API Atelier (:4100) est-elle up ?" >&2
  exit 4
}

echo "$resp" | jq '
  (.data // [])
  | map({app, severity, area, status, id, title, context, tried})
'
