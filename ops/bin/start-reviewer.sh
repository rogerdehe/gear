#!/usr/bin/env bash
# start-reviewer.sh <task-number> <branch-under-review> [--resume <session-id>] [--model <label>]
# Start a review job on a warm reviewer seat (17 §8a, ops/CAPTAIN.md step 4).
# A seat is a Herdr worktree `home/review-<k>` (label gear-review-<k>) whose
# shell and build cache stay warm. Each job is a NEW OpenCode session on branch
# review/t<n> (clean context); a re-review passes --resume <session-id>.
# Seats are created on demand, at most MAX_SEATS (default 3).
set -eu
n=${1:?task number}; branch=${2:?branch under review}; shift 2
resume=""; model="DeepSeek V4.1 Flash 1017"; filter=""   # must differ from the writer's model
alt_model=${REVIEW_ALT_MODEL:-"DeepSeek V4 Pro"}; alt_filter=${REVIEW_ALT_FILTER:-"DeepSeek V4 Pro"}
while [ $# -gt 0 ]; do case "$1" in --resume) resume=$2; shift 2 ;; --model) model=$2; shift 2 ;; *) echo "bad arg $1" >&2; exit 2 ;; esac; done
main=$(git -C "$(dirname "$0")" worktree list --porcelain | sed -n '1s/^worktree //p')
wtroot=$(dirname "$(git -C "$main" worktree list --porcelain | sed -n 's/^worktree \(.*\/home-data\)$/\1/p')")
max=${MAX_SEATS:-4}
# independence: if the writer lane currently runs the reviewer model, use the alternate (lanes may be switched on quota)
writer="gear-${branch%%/*}"
if herdr agent read "$writer" --source visible 2>/dev/null | grep -E 'Build auto' | grep -qF "$model"; then model=$alt_model; filter=$alt_filter; fi
[ -n "$filter" ] || filter=$model

# 0. a hand-in that does not merge into dev goes straight back to the writer (lesson L-27).
git -C "$main" fetch -q origin
git -C "$main" merge-tree --write-tree origin/dev "origin/$branch" >/dev/null ||
  { echo "origin/$branch conflicts with origin/dev: send it back as rework (merge dev in), no review" >&2; exit 3; }

# 1. a free seat = seat worktree whose pane hosts no agent; else create one.
# A resumed session is bound to the directory it was created in (OpenCode keeps
# working there), so a re-review must run on that same seat (lesson L-31).
seats=$(seq 1 "$max")
if [ -n "$resume" ]; then
  sdir=$(opencode session export "$resume" 2>/dev/null | grep -m1 -oE '"directory": *"[^"]*"' | sed 's/.*"\([^"]*\)"$/\1/')
  case "$sdir" in "$wtroot"/home-review-*) seats=${sdir##*-} ;; *) echo "session $resume has no seat directory ($sdir)" >&2; exit 1 ;; esac
  # its seat busy: do not wait — new-session recovery on any free seat (design/00 §1),
  # the new reviewer reads its predecessor's review file instead of remembering it
  p=$(herdr worktree list --cwd "$main" | tr '{' '\n' | grep -F "\"path\":\"$sdir\"" | sed -n 's/.*"open_workspace_id":"\([^"]*\)".*/\1/p')
  p=${p:+$(herdr pane list --workspace "$p" | sed -n 's/.*"pane_id":"\([^"]*\)".*/\1/p' | head -1)}
  if [ -n "$p" ] && herdr agent get "$p" >/dev/null 2>&1; then
    echo "seat $seats (session $resume) is busy: new-session recovery" >&2
    recovered=$resume; resume=""; seats=$(seq 1 "$max")
  fi
fi
seat=""; pane=""
for k in $seats; do
  dir="$wtroot/home-review-$k"
  if [ -d "$dir" ]; then
    ws=$(herdr worktree list --cwd "$main" | tr '{' '\n' | grep -F "\"path\":\"$dir\"" | sed -n 's/.*"open_workspace_id":"\([^"]*\)".*/\1/p')
    [ -n "$ws" ] || ws=$(herdr worktree open --cwd "$main" --path "$dir" --label "gear-review-$k" --no-focus | sed -n 's/.*"workspace_id":"\([^"]*\)".*/\1/p' | head -1)
    p=$(herdr pane list --workspace "$ws" | sed -n 's/.*"pane_id":"\([^"]*\)".*/\1/p' | head -1)
    if ! herdr agent get "$p" >/dev/null 2>&1; then seat=$k; pane=$p; break; fi
  else
    pane=$(herdr worktree create --cwd "$main" --branch "home/review-$k" --base dev --label "gear-review-$k" --no-focus |
      sed -n 's/.*"root_pane":{[^}]*"pane_id":"\([^"]*\)".*/\1/p')
    seat=$k; break
  fi
done
[ -n "$seat" ] && [ -n "$pane" ] || { echo "no free reviewer seat (${resume:+the seat of that session, $seats is busy; }max $max) — retry when one frees" >&2; exit 4; }
dir="$wtroot/home-review-$seat"

# 2. the seat's shell must be idle at a prompt before anything is typed (lesson L-1).
for _ in $(seq 1 30); do herdr pane read "$pane" --source recent --lines 2 | grep -q 'home-review' && break; sleep 0.5; done
herdr agent get "$pane" >/dev/null 2>&1 && { echo "seat $seat pane $pane hosts an agent" >&2; exit 1; }

# 3. put the seat on the job's branch (git only; no agent is running here).
git -C "$dir" fetch -q origin
if git -C "$dir" rev-parse -q --verify "refs/heads/review/t$n" >/dev/null; then git -C "$dir" switch -q "review/t$n"
elif git -C "$dir" rev-parse -q --verify "refs/remotes/origin/review/t$n" >/dev/null; then git -C "$dir" switch -q -c "review/t$n" "origin/review/t$n"
else git -C "$dir" switch -q -c "review/t$n" origin/dev; fi

# 4. a new session (or the job's own session on re-review), unattended, right model.
agent="gear-r$n"
if [ -n "$resume" ]; then herdr agent start "$agent" --kind opencode --pane "$pane" --timeout 60000 -- --auto -s "$resume" >/dev/null
else herdr agent start "$agent" --kind opencode --pane "$pane" --timeout 60000 -- --auto >/dev/null; fi
bar() { herdr pane read "$pane" --source visible | grep -E 'Build auto' | tail -1; }
for _ in $(seq 1 20); do bar | grep -q 'Build auto' && break; sleep 0.5; done   # UI drawn (cold seat)
if ! bar | grep -qF "$model"; then
  herdr pane send-text "$pane" "/model" >/dev/null; herdr pane send-keys "$pane" enter >/dev/null; sleep 1
  herdr pane send-text "$pane" "$filter" >/dev/null; sleep 1
  herdr pane send-keys "$pane" enter >/dev/null; sleep 1
  # models with variants (e.g. DeepSeek V4 Pro) open a "Select variant" dialog next (lesson L-30)
  if herdr pane read "$pane" --source visible | grep -q 'Select variant'; then
    herdr pane send-text "$pane" "${REVIEW_VARIANT:-high}" >/dev/null; sleep 0.5
    herdr pane send-keys "$pane" enter >/dev/null; sleep 1
  fi
fi
bar | grep -qF "$model" || { echo "model check failed: $(bar)" >&2; exit 1; }

if [ -n "${recovered:-}" ]; then
  herdr agent prompt "$agent" "You are a temp reviewer ($agent), new session recovering a re-review (the previous reviewer session $recovered is busy elsewhere). Re-review T-$n (branch $branch) per ops/REVIEW.md requirement 4: read the previous round in origin/review/t$n:ops/reviews/T-$n.md, check each of its findings is resolved, review the delta, report anything new; say 'new-session recovery' in the Reviewed line. Work on branch review/t$n, then end your turn." >/dev/null
elif [ -n "$resume" ]; then
  herdr agent prompt "$agent" "Re-review T-$n (branch $branch) per ops/REVIEW.md requirement 4: check each of your previous findings is resolved, review the delta, report anything new. Work on branch review/t$n, then end your turn." >/dev/null
else
  herdr agent prompt "$agent" "You are a temp reviewer ($agent). Review T-$n on branch $branch per ops/REVIEW.md. You review exactly this one task, on branch review/t$n, then end your turn." >/dev/null
fi
# delivered only when seen working (lesson L-30)
for _ in $(seq 1 20); do herdr agent get "$agent" | grep -q '"agent_status":"working"' && break; sleep 0.5; done
herdr agent get "$agent" | grep -q '"agent_status":"working"' || { echo "prompt not picked up: $(herdr pane read "$pane" --source visible | tail -5)" >&2; exit 1; }
echo "seat=$seat pane=$pane agent=$agent model=$model branch=review/t$n${resume:+ resume=$resume}"
