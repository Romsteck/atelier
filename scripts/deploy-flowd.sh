#!/usr/bin/env bash
# Deploy atelier-flowd from CloudMaster to Medion.
#
# Usage: scripts/deploy-flowd.sh
#
# Pre-req: `cargo build --release -p hr-flow-daemon` already produced
#   target/release/atelier-flowd. The Makefile target `deploy-flowd` runs the
#   build before invoking this script.
#
# Layout:
#   CloudMaster build     : target/release/atelier-flowd
#   Medion binary         : /opt/atelier/bin/atelier-flowd
#   Medion service unit   : /etc/systemd/system/hr-flowd.service
#   Medion environment    : /etc/atelier-flowd/.env
set -euo pipefail

MEDION="${ATELIER_MEDION:-romain@10.0.0.254}"
# hr-flowd binds to 127.0.0.1:4002 on Medion (not exposed to the LAN), so the
# healthcheck must hit it from inside the host via SSH. Override `HR_FLOWD_HEALTH`
# to a fully-qualified URL only if you've made flowd listen on a routable iface.
HR_FLOWD_HEALTH="${HR_FLOWD_HEALTH:-http://127.0.0.1:4002/v1/health}"
BIN_LOCAL="target/release/atelier-flowd"
UNIT_LOCAL="systemd/hr-flowd.service"

if [[ ! -f "$BIN_LOCAL" ]]; then
  echo "error: $BIN_LOCAL not found — run 'cargo build --release -p hr-flow-daemon' first" >&2
  exit 1
fi
if [[ ! -f "$UNIT_LOCAL" ]]; then
  echo "error: $UNIT_LOCAL not found" >&2
  exit 1
fi

echo "→ rsync atelier-flowd binary to Medion"
rsync -a --rsync-path='sudo rsync' "$BIN_LOCAL" "$MEDION:/opt/atelier/bin/atelier-flowd.new"

echo "→ rsync hr-flowd.service unit"
rsync -a --rsync-path='sudo rsync' "$UNIT_LOCAL" "$MEDION:/etc/systemd/system/hr-flowd.service"

echo "→ atomic swap binary, ensure /etc/atelier-flowd exists, reload + restart"
ssh "$MEDION" '
  set -euo pipefail
  sudo install -o root -g root -m 0755 /opt/atelier/bin/atelier-flowd.new /opt/atelier/bin/atelier-flowd
  sudo rm /opt/atelier/bin/atelier-flowd.new
  sudo install -d -o root -g root -m 0750 /etc/atelier-flowd
  if [[ ! -f /etc/atelier-flowd/.env ]]; then
    echo "warning: /etc/atelier-flowd/.env missing — daemon will fail until ATELIER_FLOW_TOKEN is provisioned" >&2
  fi
  sudo systemctl daemon-reload
  sudo systemctl restart hr-flowd.service
'

echo "→ healthcheck $HR_FLOWD_HEALTH (via $MEDION)"
for i in 1 2 3 4 5; do
  if ssh "$MEDION" "curl -fsS '$HR_FLOWD_HEALTH'" | tee /dev/stderr | jq -e '.ok == true' >/dev/null 2>&1; then
    echo "→ hr-flowd is up"
    exit 0
  fi
  sleep 1
done

echo "error: hr-flowd healthcheck failed; recent logs:" >&2
ssh "$MEDION" 'sudo journalctl -u hr-flowd -n 40 --no-pager' >&2
exit 1
