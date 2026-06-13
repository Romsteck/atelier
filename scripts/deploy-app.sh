#!/usr/bin/env bash
# Deploy a single app: trigger build on Medion (local or via SSH) + restart via Atelier API.
#
# Usage: scripts/deploy-app.sh <slug> [--build|--no-build]
#
# Layout post-rapatriement (2026-05-27):
#   Sources canoniques + runtime : /var/lib/atelier/apps/<slug>/  (sur Medion)
#   Atelier API                  : http://10.0.0.254:4100
#
# Behaviour :
#   - Quand le script tourne SUR Medion : build in-place (cd src/ && build_cmd).
#   - Ailleurs (CM, dev workstation)    : build via SSH vers Medion.
#   - Plus de rsync transversal — source = runtime depuis le cutover.
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <slug> [--build|--no-build]" >&2
  exit 1
fi

SLUG="$1"
BUILD="${2:---build}"

MEDION="${ATELIER_MEDION:-romain@10.0.0.254}"
ATELIER_API="${ATELIER_API:-http://10.0.0.254:4100}"
APP_SRC="/var/lib/atelier/apps/$SLUG/src"

is_local_medion() { [[ "$(uname -n)" == "medion" ]]; }

# bash -lc charges the login profile so cargo / corepack / pnpm are on PATH.
run_on_medion() {
  local cmd="$1"
  if is_local_medion; then
    bash -lc "$cmd"
  else
    ssh "$MEDION" "bash -lc $(printf '%q' "$cmd")"
  fi
}

# Serialise concurrent deploys of the same slug.
LOCKFILE="/tmp/atelier-deploy-${SLUG}.lock"
exec 9>"$LOCKFILE"
if ! flock -n 9; then
  echo "error: another deploy of '$SLUG' is already in progress (lock: $LOCKFILE)" >&2
  exit 1
fi

# Fetch the registry for build_command + health_path + stack. A failure aborts —
# a silent fallback would mis-detect the stack or skip the build.
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

if is_local_medion; then
  echo "→ deploy-app slug=$SLUG stack=$STACK build=$BUILD (local on Medion)"
else
  echo "→ deploy-app slug=$SLUG stack=$STACK build=$BUILD (via SSH to $MEDION)"
fi

# Sanity check: source tree exists on Medion.
if ! run_on_medion "[[ -d '$APP_SRC' ]]"; then
  echo "error: $APP_SRC missing on Medion — aborting" >&2
  exit 1
fi

# Build.
if [[ "$BUILD" == "--build" ]]; then
  if [[ -n "$BUILD_CMD" ]]; then
    echo "→ build: cd $APP_SRC && $BUILD_CMD"
    if ! run_on_medion "cd '$APP_SRC' && $BUILD_CMD"; then
      echo "error: build_command failed for $SLUG — aborting" >&2
      exit 1
    fi
  else
    echo "  (no build_command for $SLUG, skipping build)"
  fi
fi

# Restart via Atelier API.
echo "→ POST $ATELIER_API/api/apps/$SLUG/control restart"
curl -fsS -X POST "$ATELIER_API/api/apps/$SLUG/control" \
  -H 'content-type: application/json' \
  -d '{"action":"restart"}' \
  --max-time 30 \
  -w "\n  HTTP %{http_code}\n" || {
  echo "error: restart request failed for $SLUG" >&2
  exit 1
}

# Healthcheck: poll the app through the Atelier path-proxy (/apps/{slug}) —
# exercises the same proxy chain users traverse and reaches the app's TCP
# listener. The {slug}.mynetwk.biz hostnames are dead (path-routing era), and
# the edge would 302 anonymous calls before reaching the app anyway.
HEALTH_URL="${ATELIER_API}/apps/${SLUG}${HEALTH_PATH}"
echo "→ healthcheck $HEALTH_URL"
HEALTH_CODE="000"
for i in $(seq 1 15); do
  sleep 2
  HEALTH_CODE="$(curl -s -o /dev/null -w '%{http_code}' --max-time 8 "$HEALTH_URL" || echo 000)"
  if [[ "$HEALTH_CODE" =~ ^(2|3)[0-9][0-9]$ ]]; then
    echo "  healthcheck OK (HTTP $HEALTH_CODE after $((i * 2))s)"
    exit 0
  fi
done
echo "error: healthcheck failed for $SLUG (last HTTP $HEALTH_CODE)" >&2
exit 1
