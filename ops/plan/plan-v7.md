<!-- gear-plan project=gear base=6 -->
# Decisions
- 2026-10-22 USER: never-stop means lanes always do VALUABLE work, not busywork. Every queued task must move the self-hosting loop or a user-facing need forward. Priority now: close develop -> review -> merge without the captain in the middle (T-5, T-12, T-13). T-11 cancelled (duplicates T-7). T-9 lowered.

# Queue
## T-5 [must] lane=runtime milestone=M1 required
deps: T-1

## NEW [must] lane=runtime milestone=M1 required
goal: The engine merges a passed task. New op kind `merge` executed by the engine: in the engine's own integration worktree (Herdr `worktree_open`/`worktree_create` branch `home/merge` based on the integration branch, created once), fetch, `git merge --no-ff origin/<task branch>` then `origin/review/t<n>`, refuse conflict markers / abort a conflicted merge, run the gate command from config (`[conventions].gate`, default `cargo build --workspace --all-targets && cargo test --workspace`; add the field), push the integration branch, delete the task and review branches on origin. Outcome recorded on the op (Confirmed / Failed with the git or gate output tail). A failed merge or gate leaves dev untouched and puts one item in the captain's inbox. Use ops/bin/merge-task.sh as the reference for the steps (it is the manual stand-in this replaces).
accept: cargo test --workspace green; tests over a scratch git repo (tempdir with a bare origin): clean merge -> pushed + branches gone; conflict -> dev untouched + Failed; gate fails -> dev untouched + Failed.
deps: -

## T-7 [must] lane=runtime milestone=M1 required
deps: -

## NEW [must] lane=data milestone=M1 required
goal: A review verdict settles the task without the captain (uses T-2's stored review). `gear_app::tasks::settle_review(tx, project, task, attempt, now)`: verdict pass -> attempt Accepted, task Done, and an intended `merge` op (kind added by the runtime lane in T-12; if it is not on dev yet, add the OpKind variant yourself — a one-line enum change both tasks may make); verdict fail -> attempt Rejected, task Ready with rework = the review findings, ranked first in its lane's queue (rework before new work). The captain gets an information line, not an action item. Called by the engine when a review arrives (one call in the engine tick; keep it small).
accept: cargo test --workspace green; tests: pass -> Done + one merge op; fail -> Ready at the head of the lane queue with the findings as rework; a second call is a no-op.
deps: -

## T-8 [should] lane=data milestone=M1
deps: -

## T-6 [must] lane=data milestone=M1 required
deps: -

## T-9 [could] lane=data milestone=M1
deps: -

## T-10 [should] lane=data milestone=M1
deps: -

## T-11 [should] lane=board milestone=M1
state: cancelled

## NEW [should] lane=board milestone=M1
goal: Task dependency view on the board (user asked for it): `gear board --deps` (and a key `d` in the live board toggling it) draws the open tasks as a dependency tree in text — each task with its state, children indented under the tasks they depend on, tasks with unmet deps marked `waits T-x`. Plain text, no graphics library.
accept: cargo test --workspace green; a contract test with a small DAG (diamond + one independent task) checks the rendered lines.
deps: -
