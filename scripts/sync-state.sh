#!/usr/bin/env bash
# Sync state JSON files (apps registry + port registry) from Medion.
# These small files reflect the live state du supervisor hr-orchestrator.
# Lancé périodiquement par atelier-sync-state.timer (toutes les 2 min).
set -euo pipefail

DEST="/var/lib/atelier/state"
mkdir -p "$DEST"

rsync -aH \
  romain@10.0.0.254:/opt/homeroute/data/apps.json \
  romain@10.0.0.254:/opt/homeroute/data/port-registry.json \
  "$DEST/"

# tasks.db: snapshot SQLite cohérent via .backup (WAL-safe pour lecteur read-only)
ssh romain@10.0.0.254 \
  'sudo sqlite3 /opt/homeroute/data/tasks.db ".backup /tmp/atelier-tasks.db" && sudo cat /tmp/atelier-tasks.db && sudo rm -f /tmp/atelier-tasks.db' \
  > "$DEST/tasks.db.tmp" 2>/dev/null
if [ -s "$DEST/tasks.db.tmp" ]; then
  mv "$DEST/tasks.db.tmp" "$DEST/tasks.db"
  rm -f "$DEST/tasks.db-wal" "$DEST/tasks.db-shm"
else
  rm -f "$DEST/tasks.db.tmp"
fi

# dataverse-secrets.json — sensible. ssh+sudo lecture, écriture privée 0600.
ssh romain@10.0.0.254 'sudo cat /opt/homeroute/data/dataverse-secrets.json' \
  > "$DEST/dataverse-secrets.json.tmp"
chmod 0600 "$DEST/dataverse-secrets.json.tmp"
mv "$DEST/dataverse-secrets.json.tmp" "$DEST/dataverse-secrets.json"

date -Iseconds > /var/lib/atelier/state.last-sync
