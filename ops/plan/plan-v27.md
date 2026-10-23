<!-- gear-plan project=gear base=26 -->
# Decisions
- 2026-10-23 T-12 review (engine-started Codex reviewer, ops/reviews/T-12.md on review/t12) found 2 P0 + 6 P1 + 1 P2. Rework scope set by the captain (skeleton): the two P0s, P1 "old pass authorises a newer attempt", P1 "does not merge into current dev", P1 "gate stdout lost". Logged for later, not in this rework: failed-merge retry/duplicate items, remote cleanup errors, Unknown ops of cancelled tasks, the Executing-before-gate P2. T-13 rework first in data.

# Verdicts
- T-12 attempt=1 contract=2 candidate=aa7d458 fail rework="Review: git show origin/review/t12:ops/reviews/T-12.md. Fix ONLY these (captain scope): [P0] merge exactly the approved candidate SHA, never the branch tip (the op carries the candidate; a branch that moved past it is refused and goes to the captain); [P0] a missing task branch is not proof of a merge — confirm only when origin/<integration> contains the candidate; [P1] require the pass verdict of the CURRENT attempt and candidate, never an older attempt's; [P1] merge current origin/dev into your branch first (it conflicts with the engine's review step in lib.rs; keep both); [P1] keep a bounded tail of stdout AND stderr of a failed gate. Everything else in the review is logged for later — do not do it now."
- T-16 attempt=1 contract=1 candidate=bc5e38eb76f5ef9dd1fd975a798062fc554fdce2 fail rework="Review: git show origin/review/t16:ops/reviews/T-16.md. P1 F1: count hand-ins and rejections by the seat that made the attempt (attempt.seat), not by the lane the task belongs to now — tasks do move between lanes (T-7, T-9, T-10 moved today). Add a test where a task moves lanes after a rejection."

# Queue
## T-13 [must] lane=data milestone=M1 required
deps: -

## T-12 [must] lane=runtime milestone=M1 required
deps: -

## T-16 [should] lane=board milestone=M1
deps: -
