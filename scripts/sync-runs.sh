#!/usr/bin/env bash
# Sync flow runs (JSON) from Medion to /var/lib/atelier/apps/{slug}/runs/.
# Layout mirrors /opt/homeroute/apps/{slug}/runs/ on Medion so flows.rs n'a
# qu'à pointer apps_runtime_root → /var/lib/atelier/apps.
set -euo pipefail

DEST="/var/lib/atelier/apps"
mkdir -p "$DEST"

rsync -aH --ignore-missing-args --delete \
  --include='*/' \
  --include='*/runs/' \
  --include='*/runs/*.json' \
  --exclude='*' \
  romain@10.0.0.254:/opt/homeroute/apps/ "$DEST/"

date -Iseconds > /var/lib/atelier/runs.last-sync
