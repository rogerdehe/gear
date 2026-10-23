<!-- gear-plan project=gear base=31 -->
# Decisions
- 2026-10-23 T-17 review: rework scope = R1 (Queued/Starting reviewers get released — reviews never start) and R4 (workspace_not_found not treated as removed — slot held forever). R2 (slot freed one tick later) and R3 (exit keys may repeat after a restart) logged, not in this rework. The captain destroyed T-10's engine reviewer by force-removing an old manual worktree the engine had reused (L-48); T-10 gets a new reviewer.

# Verdicts
- T-17 attempt=1 contract=1 candidate=4043ddf fail rework="Review: git show origin/dev:ops/reviews/T-17.md. Fix ONLY: R1 — never release a temp that is Queued or Starting, or whose task is still under review with its current attempt (only release when the task is cancelled/settled or a newer attempt replaced the reviewed one); R4 — treat `workspace_not_found` as already removed. R2 and R3 are logged for later; do not do them now."
