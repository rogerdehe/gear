#!/usr/bin/env bash
# start-reviewer.sh <task-number> <branch-under-review> [--resume <session-id>] [--model <label>]
# Start a review job on a warm reviewer seat (17 §8a, ops/CAPTAIN.md step 4).
# A seat is a Herdr worktree `home/review-<k>` (label gear-review-<k>) whose
# shell and build cache stay warm. Each job is a NEW OpenCode session on branch
# review/t<n> (clean context); a re-review passes --resume <session-id>.
# Seats are created on demand, at most MAX_SEATS (default 3).
set -eu
n=${1:?task number}; branch=${2:?branch under review}; shift 2
resume=""; model="DeepSeek V4.1 Flash 1017"     # must differ from the writer's model
while [ $# -gt 0 ]; do case "$1" in --resume) resume=$2; shift 2 ;; --model) model=$2; shift 2 ;; *) echo "bad arg $1" >&2; exit 2 ;; esac; done
main=$(git -C "$(dirname "$0")" worktree list --porcelain | sed -n '1s/^worktree //p')
wtroot=$(dirname "$(git -C "$main" worktree list --porcelain | sed -n 's/^worktree \(.*\/home-data\)$/\1/p')")
max=${MAX_SEATS:-3}

# 0. a hand-in that does not merge into dev goes straight back to the writer (lesson L-27).
git -C "$main" fetch -q origin
git -C "$main" merge-tree --write-tree origin/dev "origin/$branch" >/dev/null ||
  { echo "origin/$branch conflicts with origin/dev: send it back as rework (merge dev in), no review" >&2; exit 3; }

# 1. a free seat = seat worktree whose pane hosts no agent; else create one.
seat=""; pane=""
for k in $(seq 1 "$max"); do
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
[ -n "$seat" ] && [ -n "$pane" ] || { echo "no free reviewer seat (max $max)" >&2; exit 1; }
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
if ! bar | grep -qF "$model"; then
  herdr pane send-text "$pane" "/model" >/dev/null; herdr pane send-keys "$pane" enter >/dev/null; sleep 1
  herdr pane send-text "$pane" "$model" >/dev/null; sleep 1
  herdr pane send-keys "$pane" enter >/dev/null; sleep 1
fi
bar | grep -qF "$model" || { echo "model check failed: $(bar)" >&2; exit 1; }

if [ -n "$resume" ]; then
  herdr agent prompt "$agent" "Re-review T-$n (branch $branch) per ops/REVIEW.md requirement 4: check each of your previous findings is resolved, review the delta, report anything new. Work on branch review/t$n, then end your turn." >/dev/null
else
  herdr agent prompt "$agent" "You are a temp reviewer ($agent). Review T-$n on branch $branch per ops/REVIEW.md. You review exactly this one task, on branch review/t$n, then end your turn." >/dev/null
fi
echo "seat=$seat pane=$pane agent=$agent model=$model branch=review/t$n${resume:+ resume=$resume}"
