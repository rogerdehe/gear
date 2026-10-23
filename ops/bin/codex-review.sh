#!/usr/bin/env bash
# codex-review.sh <n> <lane-branch> "<where the contract is>" ["<extra focus>"]
# STOPGAP (ops/STOPGAPS.md): start a Codex temp reviewer for T-<n> until the
# engine does it (T-5). Herdr worktree review/t<n> from origin/<lane-branch>,
# Codex gpt-6.1-sol high (agent:codex SOP), review prompt per ops/REVIEW.md.
set -euo pipefail
n=${1:?task number}; branch=${2:?lane branch}; contract=${3:?where the contract is}; focus=${4:-}
repo=/Volumes/Data/myai/gear
git -C "$repo" fetch -q origin
pane=$(herdr worktree create --cwd "$repo" --branch "review/t$n" --base "origin/$branch" \
  --label "gear-review-t$n" --no-focus | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["root_pane"]["pane_id"])')
for _ in $(seq 30); do herdr pane read "$pane" --source visible | grep -q "review-t$n" && break; sleep 2; done
herdr agent start "gear-review-t$n" --kind codex --pane "$pane" --timeout 60000 -- \
  -m gpt-6.1-sol -c model_reasoning_effort=high --dangerously-bypass-approvals-and-sandbox >/dev/null
herdr agent prompt "gear-review-t$n" "You are a temp reviewer for the GEAR project (repo in this worktree, branch review/t$n). Review task T-$n (branch origin/$branch, report ops/reports/T-$n.md on that branch). Task contract: $contract. Follow ops/REVIEW.md exactly (requirements, delivering, template): write ops/reviews/T-$n.md on branch review/t$n, commit and push it. Review the branch as merged with current origin/dev; a finding already fixed on dev is not a finding. Skeleton mode: a fail needs a failed mechanical fact or a P0-P2 that would break the feature or the loop; list P3s without blocking. Prove behaviour in its real environment where you can (built binary, GEAR_DB on a scratch temp store — never ~/.local/state/gear; a PTY for terminal UI; FakeHerdr-driven ticks for engine logic: one engine restart at a step, one captain action such as a cancel; stacked coincidences are P3 with their likelihood, never blocking). $focus Do not run herdr commands, do not change origin/$branch. End your turn with one line: Verdict: pass|fail." --wait --timeout 10000 >/dev/null 2>&1 || true
echo "gear-review-t$n on $pane: $(herdr agent get "gear-review-t$n" | grep -o '"agent_status":"[a-z]*"')"
