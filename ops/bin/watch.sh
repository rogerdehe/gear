#!/usr/bin/env bash
# watch.sh — the captain's doorbell (manual stand-in for the engine).
# Blocks until something needs the captain, prints it, exits. Run it in the
# background; when it exits the captain is woken, handles it, and runs it again.
# Events:
#   packet  a pushed task branch's packet Status changed to review/question/blocked
#   stopped a lane agent is idle/done/blocked (it ended its turn)
# Idempotent: remembers what it already reported in .git/gear-watch.
set -u
main=$(git -C "$(dirname "$0")" worktree list --porcelain | sed -n '1s/^worktree //p')
board="$main/ops/PLAN.md"
seen="$main/.git/gear-watch"
interval=${WATCH_INTERVAL:-20}
touch "$seen"

report() { # key line
  grep -qxF "$1" "$seen" && return 1
  echo "$1" >>"$seen"
  echo "$2"
  return 0
}

while :; do
  git -C "$main" fetch -q --prune origin 2>/dev/null
  # Board rows: | T-n | lane | `branch` | content | state | depends |
  awk -F'|' 'NF>=7 && $2 ~ /T-[0-9]/ { for (i=2;i<=6;i++) gsub(/^ +| +$|`/, "", $i); print $2, $3, $4, $6 }' "$board" |
  while read -r task lane branch state; do
    case "$state" in ready|running|rework|waiting|verifying) ;; *) continue ;; esac
    n=${task#T-}
    st=$(git -C "$main" show "origin/$branch:ops/tasks/T-$n.md" 2>/dev/null | sed -n 's/^Status: *\([a-z]*\).*/\1/p' | head -1)
    sha=$(git -C "$main" rev-parse --short "origin/$branch" 2>/dev/null)
    case "$st" in review|question|blocked)
      report "packet $task $st $sha" "packet $task ($lane) Status: $st @ $sha" && touch "$main/.git/gear-watch.hit" ;;
    esac
  done
  for agent in gear-data gear-runtime; do
    s=$(herdr agent get "$agent" 2>/dev/null | sed -n 's/.*"agent_status":"\([a-z]*\)".*/\1/p')
    case "$s" in
      idle|done|blocked) report "stopped $agent" "stopped $agent ($s)" && touch "$main/.git/gear-watch.hit" ;;
      working) sed -i "" "/^stopped $agent$/d" "$seen" ;;
    esac
  done
  [ -e "$main/.git/gear-watch.hit" ] && { rm -f "$main/.git/gear-watch.hit"; exit 0; }
  sleep "$interval"
done
