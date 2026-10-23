<!-- gear-plan project=gear base=24 -->
# Decisions
- 2026-10-23 T-5 passed by captain decision after Codex round 2 (ops/reviews/T-5.md on review/t5): R2-1 (accepted start repeated after a crash + agent exit while down) and R2-3 (quota lowered mid-tick) are stacked coincidences with bounded effect — logged, not tasks. R2-2 (an obsolete reviewer keeps its slot forever) blocks the loop -> T-17. Review severity calibrated in ops/REVIEW.md (P1 = one fault at a time).

# Verdicts
- T-5 attempt=2 contract=2 candidate=7de2574 pass evidence=ops/reviews/T-5.md

# Queue
## NEW [must] lane=runtime milestone=M1 required
goal: A review temp whose task is no longer under review (cancelled, settled, or a newer attempt handed in) is released on the next tick: exit the agent (profile exit keys), remove its review worktree via Herdr, free the slot. Purpose: a slot held forever stops all later reviews (T-5 review R2-2). Environment: the engine ticks concurrently with `gear` commands and may restart between steps; one release failing must not stop the tick.
accept: cargo test --workspace green; engine tests with FakeHerdr: task cancelled while its reviewer runs -> next tick exit + worktree_remove + slot free; an engine restart between exit and remove -> the next tick finishes the removal without a second exit; a failing remove for one temp does not stop the tick.
deps: -
