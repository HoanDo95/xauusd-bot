#!/usr/bin/env bash
# Backup the persistent bot state (config + signal history + quota/dedup).
# Usage (on server):
#   bash scripts/backup-state.sh
# Optional env:
#   XAUUSD_ROOT=/home/jason/xauusd-bot
#   KEEP=14
set -euo pipefail
umask 077

ROOT="${XAUUSD_ROOT:-/home/jason/xauusd-bot}"
STATE="$ROOT/data/bot_state.json"
BACKUP_DIR="$ROOT/backups"
KEEP="${KEEP:-14}"

if [ ! -f "$STATE" ]; then
  echo "state file not found: $STATE" >&2
  exit 1
fi

mkdir -p "$BACKUP_DIR"
chmod 700 "$BACKUP_DIR"
ts="$(date -u +%Y%m%dT%H%M%SZ)"
dest="$BACKUP_DIR/bot_state-$ts.json"
cp -p "$STATE" "$dest"
chmod 600 "$dest"

# Keep only the newest $KEEP backups.
ls -1t "$BACKUP_DIR"/bot_state-*.json 2>/dev/null \
  | tail -n +"$((KEEP + 1))" \
  | xargs -r rm -f

echo "backup ok: $dest"
