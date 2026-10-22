#!/usr/bin/env bash
# inbox.sh <lane> — manual stand-in for the `gear` home view (lane side).
# Run after every finished checklist item. Reads the captain's board and mail
# from the main checkout (lanes never edit them). A task whose branch is already
# pushed is shown with the Status from its pushed packet (the lane's own claim),
# so the lane never re-takes work it started. Priority order:
#   1. rework   — board says rework (do first, conventions §5)
#   2. messages — open captain messages for you
#   3. next     — first ready task not yet started, if below the in-flight cap
set -u
lane=${1:?usage: inbox.sh <lane>   (data | runtime | review)}

main=$(git worktree list --porcelain | sed -n '1s/^worktree //p')
board="$main/ops/PLAN.md"
mail="$main/ops/mail/$lane.md"
git fetch -q origin 2>/dev/null

dev_n=0; rev_n=0; next=""; rework=""; mine=""
while IFS='|' read -r _ task l branch content state _; do
  task=$(echo $task); l=$(echo $l); state=$(echo $state)
  branch=$(echo $branch | tr -d '`'); content=$(echo $content)
  [ "$l" = "$lane" ] || continue
  case "$task" in
    T-[0-9]*) n=${task#T-}; file="ops/tasks/T-$n.md" ;;
    R-[0-9]*) n=$(sed -n 's#^review/t\([0-9]*\)$#\1#p' <<<"$branch"); file="ops/reviews/T-$n.md" ;;
    *) continue ;;
  esac
  pstat=$(git show "origin/$branch:$file" 2>/dev/null | sed -n 's/^Status: *\([a-z]*\).*/\1/p' | head -1)
  [ "$pstat" = done ] && continue
  case "$state" in
    done|cancelled|draft) continue ;;
    rework) rework+="  $task  $branch  $content"$'\n'; dev_n=$((dev_n+1)); continue ;;
  esac
  if [ -n "$pstat" ] && [ "$pstat" != ready ]; then
    mine+="  $task  $branch  $pstat"$'\n'
    case "$pstat" in review) rev_n=$((rev_n+1)) ;; question|blocked) ;; *) dev_n=$((dev_n+1)) ;; esac
  elif [ "$state" = ready ] && [ -z "$next" ]; then
    next="  $task  $branch  $content"
  fi
done < <(grep -E '^\| *[TR]-[0-9]' "$board")

echo "== rework (do first)"; printf '%s' "$rework"
echo "== messages"
[ -f "$mail" ] && awk '/^### /{open = ($0 ~ /\[open\]/)} open' "$mail"
echo "== your tasks in flight (status from your pushed packets)"; printf '%s' "$mine"
echo "   developing: $dev_n   in review: $rev_n   (cap: 1 developing + 2 in review)"
echo "== next"
if [ "$rev_n" -ge 2 ] || [ "$dev_n" -ge 1 ]; then
  echo "  none for now: finish rework / your task in development first; at the review cap end your turn"
elif [ -n "$next" ]; then
  echo "$next"
else
  echo "  no ready task: end your turn"
fi
