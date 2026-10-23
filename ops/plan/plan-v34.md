<!-- gear-plan project=gear base=33 -->
# Decisions
- 2026-10-23 T-13 round 2: rework = merge current dev (conflicts with T-5 in engine lib.rs; keep both). R2-1 (a deferred review item returns after settlement) logged in m2-backlog. T-7 (M2): rework R4 (a delivered task counted as ready — the warning misses) and R3 (parked/held lane keeps its warning); R1, R2 logged.

# Verdicts
- T-13 attempt=2 contract=1 candidate=c864f58 fail rework="Review: git show origin/dev:ops/reviews/T-13.md. Fix ONLY R2-2: merge current origin/dev into data/t13 and resolve the conflict in crates/gear-engine/src/lib.rs (T-5's review step and your settlement step both stay); rerun the workspace tests. R2-1 and the P3 are logged for later — do not do them now."
- T-7 attempt=1 contract=2 candidate=d4d4d1b fail rework="Review: git show origin/dev:ops/reviews/T-7.md. Fix ONLY: R4 — a task with a Delivered/started attempt is not ready work (do not count it); R3 — when a lane is parked or held, close its open warning. R1 and R2 are logged for later — do not do them now."
