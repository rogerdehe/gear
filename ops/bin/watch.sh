#!/usr/bin/env bash
# watch.sh — the captain's doorbell (manual stand-in for the engine).
# Blocks until something needs the captain, prints it, exits. Run it in the
# background; when it exits the captain is woken, handles it, and runs it again.
# Events:
#   packet  a pushed task branch's packet Status changed to review/question/blocked
#   stopped a lane agent is idle/done/blocked (it ended its turn) — re-rings
#           every REPEAT seconds (default 600) for as long as it stays stopped
#   lowready a lane has fewer than 2 ready tasks it has not started — re-rings
#           every REPEAT seconds until the captain restocks its queue
# Packet events ring once per status change; remembered in .git/gear-watch.
set -u
main=$(git -C "$(dirname "$0")" worktree list --porcelain | sed -n '1s/^worktree //p')
board="$main/ops/PLAN.md"
seen="$main/.git/gear-watch"
interval=${WATCH_INTERVAL:-20}
repeat=${REPEAT:-600}
touch "$seen"

# Ring for <key> unless it rang less than $repeat seconds ago.
nag() { # key line
  local now last
  now=$(date +%s)
  last=$(sed -n "s/^$1 \([0-9]*\)\$/\1/p" "$seen" | tail -1)
  [ -n "$last" ] && [ $((now - last)) -lt "$repeat" ] && return 1
  sed -i "" "/^$1 [0-9]*\$/d" "$seen"
  echo "$1 $now" >>"$seen"
  echo "$2"
}

report() { # key line
  grep -qxF "$1" "$seen" && return 1
  echo "$1" >>"$seen"
  echo "$2"
  return 0
}

while :; do
  git -C "$main" fetch -q --prune origin 2>/dev/null
  # Board rows: | T-n | lane | `branch` | content | state | depends |
  awk -F'|' 'NF>=7 && $2 ~ /[TR]-[0-9]/ { for (i=2;i<=6;i++) gsub(/^ +| +$|`/, "", $i); print $2, $3, $4, $6 }' "$board" |
  while read -r task lane branch state; do
    case "$state" in ready|running|rework|waiting|verifying) ;; *) continue ;; esac
    case "$task" in
      T-*) file="ops/tasks/T-${task#T-}.md" ;;
      R-*) file="ops/reviews/T-$(sed -n 's#^review/t\([0-9]*\)$#\1#p' <<<"$branch").md" ;;
    esac
    st=$(git -C "$main" show "origin/$branch:$file" 2>/dev/null | sed -n 's/^Status: *\([a-z]*\).*/\1/p' | head -1)
    sha=$(git -C "$main" rev-parse --short "origin/$branch" 2>/dev/null)
    case "$st" in
      review|question|blocked|done)
        report "packet $task $st" "packet $task ($lane) Status: $st @ $sha" && touch "$main/.git/gear-watch.hit" ;;
      running) sed -i "" "/^packet $task /d" "$seen" ;;   # re-arms after rework / answer
    esac
  done
  for agent in gear-data gear-runtime gear-review; do
    s=$(herdr agent get "$agent" 2>/dev/null | sed -n 's/.*"agent_status":"\([a-z]*\)".*/\1/p')
    case "$s" in
      idle|done|blocked) nag "stopped $agent" "stopped $agent ($s)" && touch "$main/.git/gear-watch.hit" ;;
      working) sed -i "" "/^stopped $agent [0-9]*$/d" "$seen" ;;
    esac
  done
  for lane in data runtime review; do
    ready=0
    while IFS='|' read -r _ task l branch _ state _; do
      [ "$(echo $l)" = "$lane" ] && [ "$(echo $state)" = ready ] || continue
      git -C "$main" rev-parse -q --verify "origin/$(echo $branch | tr -d '\`')" >/dev/null || ready=$((ready+1))
    done < <(grep -E '^\| *[TR]-[0-9]' "$board")
    if [ "$ready" -lt 2 ]; then
      nag "lowready $lane" "lowready $lane ($ready ready task(s) not started)" && touch "$main/.git/gear-watch.hit"
    else
      sed -i "" "/^lowready $lane [0-9]*$/d" "$seen"
    fi
  done
  [ -e "$main/.git/gear-watch.hit" ] && { rm -f "$main/.git/gear-watch.hit"; exit 0; }
  sleep "$interval"
done
