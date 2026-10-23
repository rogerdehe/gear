<!-- gear-plan project=gear base=20 -->
# Verdicts
- T-13 attempt=1 contract=1 candidate=fa640ee fail rework="Review: git show origin/review/t13:ops/reviews/T-13.md — three P1s, each with a reproduction. (1) A review for a task that was cancelled (or is otherwise terminal) makes every tick fail with BAD_TRANSITION before dispatch — the whole engine stops. Skip/consume obsolete reviews, and never let one settlement error abort the rest of the tick (log it, flag that task, go on). (2) settle_review accepts an attempt whose adopted contract is older than the task's current one; keep the version rule of tasks::settle (a stale pass becomes rework on the current contract, or waits for the captain). (3) Duplicate review submissions leave an Open captain item after settlement; resolve every matching review item, and a duplicate after settlement creates none. Add the three regressions with an engine restart and unrelated ready work, as in the review."

# Queue
## T-13 [must] lane=data milestone=M1 required
deps: -
