#!/usr/bin/env bash
# Swap hr-edge routes between target backends. Used during the
# CloudMaster→Medion cutover (and reversible for rollback).
#
# Usage: scripts/swap-edge-routes.sh medion|cloudmaster [--apply]
#
# - medion       : route apps to Medion loopback (127.0.0.1:port)
# - cloudmaster  : route apps to CloudMaster (10.0.0.10:port) [rollback]
# - --apply      : actually POST. Without it, dry-run.
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 medion|cloudmaster [--apply]" >&2
  exit 1
fi

DEST="$1"
MODE="${2:-dryrun}"

API="${HR_EDGE_API:-http://10.0.0.254:4000/api/edge/routes}"

case "$DEST" in
  medion)
    TARGET_IP="127.0.0.1"
    HOST_ID="local"
    ;;
  cloudmaster)
    TARGET_IP="10.0.0.10"
    HOST_ID="cloudmaster"
    ;;
  *)
    echo "error: dest must be 'medion' or 'cloudmaster'" >&2
    exit 1
    ;;
esac

# domain  app_id  port
ROUTES=(
  "www.mynetwk.biz     www      3005"
  "files.mynetwk.biz   files    3006"
  "home.mynetwk.biz    home     3007"
  "trader.mynetwk.biz  trader   3008"
  "wallet.mynetwk.biz  wallet   3009"
  "myfrigo.mynetwk.biz myfrigo  3010"
  "app.mynetwk.biz     atelier  4100"
)

for line in "${ROUTES[@]}"; do
  read -r domain app_id port <<<"$line"
  body=$(jq -n \
    --arg d "$domain" \
    --arg a "$app_id" \
    --arg h "$HOST_ID" \
    --arg t "$TARGET_IP:$port" '{
      domain: $d,
      app_id: $a,
      host_id: $h,
      target: $t,
      auth_required: false,
      allowed_groups: [],
      local_only: false
    }')
  if [[ "$MODE" == "--apply" ]]; then
    printf "POST %-25s -> %s:%s ... " "$domain" "$TARGET_IP" "$port"
    curl -sf -X POST "$API" -H 'content-type: application/json' -d "$body" -o /dev/null -w "HTTP %{http_code}\n"
  else
    echo "DRYRUN: $domain -> $TARGET_IP:$port (run with --apply to actually POST)"
  fi
done
