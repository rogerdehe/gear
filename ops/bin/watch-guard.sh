#!/usr/bin/env bash
# watch-guard.sh — Claude Code Stop hook for the captain (lesson L-22/L-39).
# Refuses to let the captain end a turn while the doorbell watcher is not
# running (heartbeat older than 300 s; one watcher loop over the board can take a minute or more). `touch .git/gear-watch.pause` turns the
# guard off (e.g. when the user stops the run).
main=$(git -C "$(dirname "$0")" worktree list --porcelain | sed -n '1s/^worktree //p')
[ -e "$main/.git/gear-watch.pause" ] && exit 0
hb="$main/.git/gear-watch.alive"
if [ ! -e "$hb" ] || [ $(( $(date +%s) - $(stat -f %m "$hb") )) -gt 300 ]; then
  echo "GEAR watcher is not running: re-arm it before ending the turn — run ops/bin/watch.sh with run_in_background (ops/CAPTAIN.md step 3)." >&2
  exit 2
fi
exit 0
