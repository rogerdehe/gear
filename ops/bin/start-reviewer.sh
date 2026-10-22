#!/usr/bin/env bash
# start-reviewer.sh <task-number> <branch-under-review> [model-label]
# Start a temp reviewer for T-<n> (17 §8, ops/CAPTAIN.md step 4): Herdr worktree
# on review/t<n> from dev, shared cargo cache, OpenCode with --auto, model
# checked on the status bar (switched via /model if needed), then the review
# prompt. Prints the pane and agent name. Manual stand-in for the engine.
set -eu
n=${1:?task number}; branch=${2:?branch under review}
model=${3:-DeepSeek V4.1 Flash 1017}     # must differ from the writer's model
main=$(git -C "$(dirname "$0")" worktree list --porcelain | sed -n '1s/^worktree //p')
agent="gear-r$n"

pane=$(herdr worktree create --cwd "$main" --branch "review/t$n" --base dev --label "$agent" --no-focus |
  sed -n 's/.*"root_pane":{[^}]*"pane_id":"\([^"]*\)".*/\1/p')
[ -n "$pane" ] || { echo "worktree create failed" >&2; exit 1; }

# The pane must be a plain shell before anything is typed into it (lesson L-1).
for _ in $(seq 1 20); do
  herdr pane read "$pane" --source recent --lines 3 | grep -q "review/t$n" && break; sleep 0.5
done
herdr agent get "$pane" >/dev/null 2>&1 && { echo "pane $pane already hosts an agent" >&2; exit 1; }
herdr pane run "$pane" "export CARGO_TARGET_DIR=$main/target/review-shared" >/dev/null
sleep 1

herdr agent start "$agent" --kind opencode --pane "$pane" --timeout 60000 -- --auto >/dev/null
bar() { herdr pane read "$pane" --source visible | grep -E 'Build auto' | tail -1; }
if ! bar | grep -qF "$model"; then
  herdr pane send-text "$pane" "/model" >/dev/null; herdr pane send-keys "$pane" enter >/dev/null; sleep 1
  herdr pane send-text "$pane" "$model" >/dev/null; sleep 1
  herdr pane send-keys "$pane" enter >/dev/null; sleep 1
fi
bar | grep -qF "$model" || { echo "model check failed: $(bar)" >&2; exit 1; }
bar | grep -q 'Build auto' || { echo "not unattended (--auto missing)" >&2; exit 1; }

herdr agent prompt "$agent" "You are a temp reviewer ($agent). Review T-$n on branch $branch per ops/REVIEW.md. You review exactly this one task, on branch review/t$n, then end your turn." >/dev/null
echo "pane=$pane agent=$agent model=$model"
