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
#   CloudMaster  : sources at /opt/homeroute/apps/<slug>/src/  (+ .env, db.sqlite)
#   Medion       : runtime at /var/lib/atelier/apps/<slug>/
#   Atelier API  : http://10.0.0.254:4100 (or via app.mynetwk.biz)
#
# Safety invariants:
#   - Only /src/ is mirrored with --delete ; Medion-only runtime state
#     (db.sqlite, runs/, bin/) is NEVER touched by this script.
#   - The CloudMaster db.sqlite is a stale snapshot — it is explicitly
#     excluded so a deploy can never clobber the live production DB.
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
MEDION_APP="/var/lib/atelier/apps/$SLUG"

if [[ ! -d "$APP_DIR/src" ]]; then
  echo "error: $APP_DIR/src not found on CloudMaster" >&2
  exit 1
fi
# Guard against an empty source tree producing a destructive --delete mirror.
if [[ -z "$(find "$APP_DIR/src" -mindepth 1 -print -quit 2>/dev/null)" ]]; then
  echo "error: $APP_DIR/src is empty — refusing to deploy (would wipe Medion src/)" >&2
  exit 1
fi

# Serialise concurrent deploys of the same slug — two parallel rsync into the
# same Medion directory would interleave artefacts.
LOCKFILE="/tmp/atelier-deploy-${SLUG}.lock"
exec 9>"$LOCKFILE"
if ! flock -n 9; then
  echo "error: another deploy of '$SLUG' is already in progress (lock: $LOCKFILE)" >&2
  exit 1
fi

# Fetch the registry once. A failure here must abort — a silent fallback
# would mis-detect the stack and exclude node_modules from a NextJS deploy.
APPS_JSON="$(curl -fsS --max-time 10 "$ATELIER_API/api/apps")" || {
  echo "error: cannot reach Atelier API at $ATELIER_API/api/apps — aborting" >&2
  exit 1
}
STACK="$(jq -r --arg s "$SLUG" '.data.apps[] | select(.slug==$s) | .stack' <<<"$APPS_JSON")"
if [[ -z "$STACK" || "$STACK" == "null" ]]; then
  echo "error: app '$SLUG' not found in the Atelier registry — aborting" >&2
  exit 1
fi
BUILD_CMD="$(jq -r --arg s "$SLUG" '.data.apps[] | select(.slug==$s) | .build_command // empty' <<<"$APPS_JSON")"
HEALTH_PATH="$(jq -r --arg s "$SLUG" '.data.apps[] | select(.slug==$s) | .health_path // "/"' <<<"$APPS_JSON")"
echo "→ deploy-app slug=$SLUG stack=$STACK build=$BUILD"

# Stack-aware rsync excludes, anchored at the src/ root. NextJS apps need the
# source node_modules (the .next/standalone bundle is incomplete and falls
# back to the parent tree).
EXCLUDES=(
  --exclude='/target/'
  --exclude='/.git/'
  --exclude='/.pnpm-store/'
  --exclude='/.cache/'
  --exclude='/.next/cache/'
  --exclude='/.vite/'
  --exclude='/.claude/'
  --exclude='/mobile/'
  --exclude='/desktop/'
  --exclude='/devices-code/'
  --exclude='/server/target/'
  --exclude='/api/target/'
  --exclude='*.log'
)
if [[ "$STACK" != "next-js" ]]; then
  EXCLUDES+=(
    --exclude='/node_modules/'
    --exclude='/web/node_modules/'
    --exclude='/client/node_modules/'
    --exclude='/api/node_modules/'
  )
fi

# Build (optional)
if [[ "$BUILD" == "--build" ]]; then
  if [[ -n "$BUILD_CMD" ]]; then
    echo "→ build: cd $APP_DIR/src && $BUILD_CMD"
    if ! (cd "$APP_DIR/src" && bash -c "$BUILD_CMD"); then
      echo "error: build_command failed for $SLUG — aborting (nothing rsynced)" >&2
      exit 1
    fi
  else
    echo "  (no build_command for $SLUG, skipping build)"
  fi
fi

# Rsync src/ with --delete so removed files do not linger on Medion. The
# destination is scoped to <app>/src/ — --delete can therefore never reach
# the Medion-only runtime state (db.sqlite, runs/, bin/).
echo "→ rsync $APP_DIR/src/ → $MEDION:$MEDION_APP/src/"
rsync -a --rsync-path='sudo rsync' --delete --info=stats1 \
  "${EXCLUDES[@]}" \
  "$APP_DIR/src/" "$MEDION:$MEDION_APP/src/"

# Sync the app .env (no --delete, never the stale db.sqlite).
if [[ -f "$APP_DIR/.env" ]]; then
  echo "→ rsync .env"
  rsync -a --rsync-path='sudo rsync' "$APP_DIR/.env" "$MEDION:$MEDION_APP/.env"
fi

# Restart via Atelier API. curl -f + set -e abort the script on a non-2xx.
echo "→ POST $ATELIER_API/api/apps/$SLUG/control restart"
curl -fsS -X POST "$ATELIER_API/api/apps/$SLUG/control" \
  -H 'content-type: application/json' \
  -d '{"action":"restart"}' \
  --max-time 30 \
  -w "\n  HTTP %{http_code}\n" || {
  echo "error: restart request failed for $SLUG" >&2
  exit 1
}

# Healthcheck — poll readiness instead of a blind sleep, and FAIL the deploy
# if the app never answers 2xx/3xx.
DOMAIN="${SLUG}.mynetwk.biz"
echo "→ healthcheck https://$DOMAIN$HEALTH_PATH"
HEALTH_CODE="000"
for i in $(seq 1 15); do
  sleep 2
  HEALTH_CODE="$(curl -s -o /dev/null -w '%{http_code}' --max-time 8 "https://$DOMAIN$HEALTH_PATH" || echo 000)"
  if [[ "$HEALTH_CODE" =~ ^(2|3)[0-9][0-9]$ ]]; then
    echo "  healthcheck OK (HTTP $HEALTH_CODE after $((i * 2))s)"
    exit 0
  fi
done
echo "error: healthcheck failed for $SLUG (last HTTP $HEALTH_CODE)" >&2
exit 1
