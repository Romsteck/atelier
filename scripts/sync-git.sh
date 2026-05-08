#!/usr/bin/env bash
# Sync git bare repos from Medion (homeroute écrit ici via push HTTP/SSH).
# Atelier consomme /var/lib/atelier/git/repos/ en lecture seule (clone/fetch + browse).
# Lancé périodiquement par atelier-sync-git.timer.
set -euo pipefail

SRC="romain@10.0.0.254:/opt/homeroute/data/git/repos/"
DEST="/var/lib/atelier/git/repos/"

mkdir -p "$DEST"

rsync -aH --delete "$SRC" "$DEST"

date -Iseconds > /var/lib/atelier/git.last-sync
