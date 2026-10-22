#!/usr/bin/env bash
# board.sh — manual stand-in for the `gear` home view (captain side).
# For each lane: Herdr agent status, checked-out task branch, HEAD, and the
# lane's own progress as recorded in its packet (Status line, ticked boxes).
set -u

WT_ROOT=/Volumes/Data/.herdr-worktrees/gear
LANES="data:gear-data:home-data runtime:gear-runtime:home-runtime"

printf '%-13s %-8s %-20s %-8s %-9s %s\n' LANE AGENT BRANCH HEAD STATUS TASKS
for entry in $LANES; do
  IFS=: read -r lane agent dir <<<"$entry"
  wt="$WT_ROOT/$dir"
  status=$(herdr agent get "$agent" 2>/dev/null | sed -n 's/.*"agent_status":"\([a-z]*\)".*/\1/p')
  branch=$(git -C "$wt" branch --show-current 2>/dev/null)
  head=$(git -C "$wt" rev-parse --short HEAD 2>/dev/null)
  # task branch <lane>/t<n>-<slug> → packet ops/tasks/T-<n>.md (read from the lane's tree)
  n=$(sed -n 's#^[a-z-]*/t\([0-9]*\)-.*#\1#p' <<<"$branch")
  pstatus=- ; ticks=-
  if [ -n "$n" ] && [ -f "$wt/ops/tasks/T-$n.md" ]; then
    packet="$wt/ops/tasks/T-$n.md"
    pstatus=$(sed -n 's/^Status: *\([a-z]*\).*/\1/p' "$packet" | head -1)
    done_n=$(grep -c '^- \[x\]' "$packet")
    all_n=$(grep -cE '^- \[( |x)\]' "$packet")
    ticks="$done_n/$all_n"
  fi
  printf '%-13s %-8s %-20s %-8s %-9s %s\n' "$agent" "${status:-none}" "${branch:--}" "${head:--}" "$pstatus" "$ticks"
done
