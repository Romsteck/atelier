#!/usr/bin/env bash
# Sync docs from Medion (source of truth — hr-orchestrator écrit ici) vers CloudMaster.
# Atelier consomme /var/lib/atelier/docs/ en lecture seule, l'index FTS5 est local.
# Lancé périodiquement par atelier-sync-docs.timer (toutes les 5 min).
set -euo pipefail

SRC="romain@10.0.0.254:/opt/homeroute/data/docs/"
DEST="/var/lib/atelier/docs/"

mkdir -p "$DEST"

# --exclude '_index.sqlite*' : Atelier reconstruit sa propre FTS5 depuis les .md
# --delete : supprime les docs supprimées côté Medion
rsync -aH --delete --exclude '_index.sqlite*' "$SRC" "$DEST"

# Touch un marker pour debug
date -Iseconds > /var/lib/atelier/docs.last-sync
