#!/usr/bin/env bash
# Deploy a single app from CloudMaster (sources + build) to Medion (runtime).
#
# Usage: scripts/deploy-app.sh <slug> [--build|--no-build]
#
# - <slug>    : files | home | myfrigo | trader | wallet | www | ...
# - --build   : run the app's build_command before rsync (default)
# - --no-build: skip build, only rsync existing artefacts
#
# Layout assumptions:
#   CloudMaster  : sources at /opt/homeroute/apps/<slug>/src/
#   Medion       : runtime at /var/lib/atelier/apps/<slug>/
#   Atelier API  : http://10.0.0.254:4100 (or via app.mynetwk.biz)
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <slug> [--build|--no-build]" >&2
  exit 1
fi

SLUG="$1"
BUILD="${2:---build}"

MEDION="${ATELIER_MEDION:-romain@10.0.0.254}"
ATELIER_API="${ATELIER_API:-http://10.0.0.254:4100}"
APP_DIR="/opt/homeroute/apps/$SLUG"
DST="$MEDION:/var/lib/atelier/apps/$SLUG/"

if [[ ! -d "$APP_DIR/src" ]]; then
  echo "error: $APP_DIR/src not found on CloudMaster" >&2
  exit 1
fi

# Stack-aware rsync excludes. NextJS apps need the source node_modules
# (the .next/standalone bundle is incomplete and falls back to the parent tree).
STACK="$(curl -sf "$ATELIER_API/api/apps" | jq -r --arg s "$SLUG" '.data.apps[] | select(.slug==$s) | .stack' 2>/dev/null || echo unknown)"
echo "→ deploy-app slug=$SLUG stack=$STACK build=$BUILD"

EXCLUDES=(
  --exclude='/src/target/'
  --exclude='/src/.git/'
  --exclude='/src/.pnpm-store/'
  --exclude='/src/.cache/'
  --exclude='/src/.next/cache/'
  --exclude='/src/.vite/'
  --exclude='/src/.claude/'
  --exclude='/src/mobile/'
  --exclude='/src/desktop/'
  --exclude='/src/devices-code/'
  --exclude='/src/server/target/'
  --exclude='/src/api/target/'
  --exclude='*.log'
)
# Only NextJS apps need /src/node_modules (their .next/standalone bundle is incomplete).
if [[ "$STACK" != "next-js" ]]; then
  EXCLUDES+=(
    --exclude='/src/node_modules/'
    --exclude='/src/web/node_modules/'
    --exclude='/src/client/node_modules/'
    --exclude='/src/api/node_modules/'
  )
fi

# Build (optional)
if [[ "$BUILD" == "--build" ]]; then
  BUILD_CMD="$(curl -sf "$ATELIER_API/api/apps" | jq -r --arg s "$SLUG" '.data.apps[] | select(.slug==$s) | .build_command // empty')"
  if [[ -n "$BUILD_CMD" ]]; then
    echo "→ build: cd $APP_DIR/src && $BUILD_CMD"
    (cd "$APP_DIR/src" && bash -c "$BUILD_CMD")
  else
    echo "  (no build_command for $SLUG, skipping build)"
  fi
fi

# Rsync
echo "→ rsync $APP_DIR/ → $DST"
rsync -a --rsync-path='sudo rsync' --info=stats1 "${EXCLUDES[@]}" "$APP_DIR/" "$DST"

# Restart via Atelier API
echo "→ POST $ATELIER_API/api/apps/$SLUG/control restart"
curl -sf -X POST "$ATELIER_API/api/apps/$SLUG/control" \
  -H 'content-type: application/json' \
  -d '{"action":"restart"}' \
  -w "\n  HTTP %{http_code}\n"

# Healthcheck
sleep 5
HEALTH_PATH="$(curl -sf "$ATELIER_API/api/apps" | jq -r --arg s "$SLUG" '.data.apps[] | select(.slug==$s) | .health_path // "/"')"
DOMAIN="${SLUG}.mynetwk.biz"
echo -n "→ healthcheck https://$DOMAIN$HEALTH_PATH : "
curl -s -o /dev/null -w "%{http_code}\n" --max-time 8 "https://$DOMAIN$HEALTH_PATH"
