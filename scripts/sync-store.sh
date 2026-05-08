#!/usr/bin/env bash
# Sync app store from Medion (source of truth — homeroute écrit ici lors des publish).
# Atelier consomme /var/lib/atelier/store/ en lecture seule.
# Lancé périodiquement par atelier-sync-store.timer (toutes les 5 min).
set -euo pipefail

SRC="romain@10.0.0.254:/opt/homeroute/data/store/"
DEST="/var/lib/atelier/store/"

mkdir -p "$DEST"

rsync -aH --delete "$SRC" "$DEST"

date -Iseconds > /var/lib/atelier/store.last-sync
