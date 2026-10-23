<!-- gear-plan project=gear base=40 -->
# Decisions
- 2026-10-23 Data ran dry while all three remaining M1 tasks sat in runtime's queue (one after another). T-15 moves to data (it handled engine internals well in T-13); M1 now advances on two lanes. Data's M2 work after it: the two most valuable items of m2-backlog.

# Queue
## T-15 [must] lane=data milestone=M1 required
goal: Quota policy in the engine (PRD §9, kickoff) — rework brief, captain scope. Read first: `git show origin/review/t15:ops/reviews/T-15.md` (F3, F4) and the branch board/t15 (start from it, it has the rest). Do ONLY: (1) merge current origin/dev first (the release module landed at the same place in lib.rs; keep both); (2) after sending the exit keys, the switch completes only when the pane shows a shell (agent_get returns no agent) — an agent still there is the OLD one; then export the next model and start with resume args; (3) quota turns never flag the seat. Everything else in the reviews is out of scope.
accept: cargo test --workspace green merged with dev; engine tests: exit sent and the old agent still present on the next tick -> no start, no adoption; old agent gone -> export + start with resume args + one continue delivery; quota turns do not flag. One engine restart between exit and start (test it).
deps: -

## NEW [should] lane=data milestone=M2
goal: The hand-in refuses a stale base. `gear send --kind result` checks that the candidate contains current origin/<integration> (`git merge-base --is-ancestor origin/dev <candidate>` after a fetch, run in the lane's worktree); if not, it returns BAD_FORMAT "merge origin/dev into your branch, build, then hand in again" and stores nothing. Purpose: two reworks (T-13, T-15) came only from hand-ins that skipped merging dev; a mechanical check beats a written rule. Environment: the lane's cwd is its worktree; the fetch may fail (offline) -> refuse with that reason, never accept unchecked.
accept: cargo test --workspace green; tests over a scratch git repo with a bare origin: candidate containing dev -> accepted; candidate behind dev -> BAD_FORMAT and no message stored; fetch failure -> refused. Real proof: run it once in your own worktree against this repo and paste the output.
deps: -

## NEW [should] lane=data milestone=M2
goal: A lane's result does not ring the captain. Purpose: the engine now starts the reviewer, settles the verdict and (soon) merges; a hand-in needs nothing from the captain, yet it lands as an Open action item and rings the doorbell. Make the result an information line for the captain (visible on the home page, not actionable); the captain is rung only for what needs a decision (questions, blocked, a failed merge, a review the engine could not settle). Environment: the engine and `gear` commands run concurrently; existing Open result items stay as they are.
accept: cargo test --workspace green; tests: a result creates no actionable captain item and no doorbell; a question still does; the captain home shows the hand-in as information.
deps: -
