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

# dataverse-secrets.json — sensible. ssh+sudo lecture, écriture privée 0600.
ssh romain@10.0.0.254 'sudo cat /opt/homeroute/data/dataverse-secrets.json' \
  > "$DEST/dataverse-secrets.json.tmp"
chmod 0600 "$DEST/dataverse-secrets.json.tmp"
mv "$DEST/dataverse-secrets.json.tmp" "$DEST/dataverse-secrets.json"

date -Iseconds > /var/lib/atelier/state.last-sync
