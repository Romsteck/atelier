#!/usr/bin/env bash
# Phase 9.3 — Migrate one app from Medion to CloudMaster.
# Usage: ./migrate-app.sh <slug>
set -euo pipefail
export CI=1

slug="$1"
ATELIER="http://127.0.0.1:4100"
HOMEROUTE="http://10.0.0.254:4000"
APP_DIR="/opt/homeroute/apps/$slug"
SRC_DIR="$APP_DIR/src"

# Read app config from Atelier
config=$(curl -sS "$ATELIER/api/apps/$slug")
stack=$(jq -r '.data.stack' <<<"$config")
port=$(jq -r '.data.port' <<<"$config")
build_cmd=$(jq -r '.data.build_command // empty' <<<"$config")
state_med=$(curl -sS "$HOMEROUTE/api/apps/$slug/status" | jq -r '.data.state // "unknown"')

echo "=== $slug ($stack, port $port, Medion=$state_med) ==="

# 1. Build on CloudMaster
echo "[1/6] Build..."
if [ -n "$build_cmd" ]; then
  (cd "$SRC_DIR" && bash -c "$build_cmd") || { echo "Build FAILED"; exit 1; }
fi

# 2. Place binary (per-app convention: ./bin/{slug} runner)
echo "[2/6] Place binary..."
case "$slug" in
  wallet)   bin_src="server/target/release/wallet-server" ;;
  trader)   bin_src="server/target/release/trader-server" ;;
  files)    bin_src="server/target/release/home-cloud" ;;
  home)     bin_src="" ;;  # runs ./server/target/release/smart-home directly
  www)      bin_src="" ;;  # node .next/standalone/server.js
  myfrigo)  bin_src="target/release/my-frigo-api" ;;
  *)        bin_src="" ;;
esac
if [ -n "$bin_src" ] && [ -f "$SRC_DIR/$bin_src" ]; then
  sudo mkdir -p "$SRC_DIR/bin"
  sudo cp "$SRC_DIR/$bin_src" "$SRC_DIR/bin/$slug"
  sudo chmod +x "$SRC_DIR/bin/$slug"
  echo "    placed $SRC_DIR/bin/$slug ($(stat -c%s "$SRC_DIR/bin/$slug") bytes)"
fi

# 3. Sync .env from Medion + swap localhost → LAN
echo "[3/6] Sync .env..."
sudo rsync -aH "romain@10.0.0.254:$APP_DIR/.env" "$APP_DIR/.env" 2>/dev/null || true
if [ -f "$APP_DIR/.env" ]; then
  sudo sed -i \
    -e 's|http://127.0.0.1:4000/api/dv|http://127.0.0.1:4100/api/dv|g' \
    -e 's|@127.0.0.1:5432|@10.0.0.254:5432|g' \
    -e 's|@localhost:5432|@10.0.0.254:5432|g' \
    "$APP_DIR/.env"
fi

# 4. Stop on Medion (graceful via homeroute API)
echo "[4/6] Stop on Medion..."
ssh romain@10.0.0.254 "curl -sS -X POST -H 'Content-Type: application/json' -d '{\"action\":\"stop\"}' http://127.0.0.1:4000/api/apps/$slug/control" | jq -c
sleep 2

# 5. Start via Atelier (CloudMaster)
echo "[5/6] Start via Atelier..."
curl -sS -X POST -H 'Content-Type: application/json' -d '{"action":"start"}' "$ATELIER/api/apps/$slug/control" | jq -c
sleep 5

# 6. Update hr-edge route
echo "[6/6] Update hr-edge route..."
curl -sS -X POST -H 'Content-Type: application/json' \
  -d "{\"domain\":\"$slug.mynetwk.biz\",\"target\":\"10.0.0.10:$port\",\"auth_required\":false,\"local_only\":false,\"app_id\":\"$slug\",\"host_id\":\"cloudmaster\"}" \
  "$HOMEROUTE/api/edge/routes" | jq -c

# Verify
echo "=== Verify $slug ==="
sleep 2
status=$(curl -sS "$ATELIER/api/apps/$slug/status" | jq -c '.data')
echo "Atelier status: $status"
http=$(curl -s -o /dev/null -w '%{http_code}' "https://$slug.mynetwk.biz/api/health" || echo "??")
echo "https://$slug.mynetwk.biz/api/health: $http"
echo
