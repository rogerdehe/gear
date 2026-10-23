<!-- gear-plan project=gear base=29 -->
# Milestone M2: After the core loop
Non-essential or for-some-users features, picked up after M1 runs and is pleasant to use: T-7, T-8, T-9, T-10, T-16 (T-9/T-10 merge if their current re-review passes; otherwise they wait here).

# Decisions
- 2026-10-23 USER: scope freeze. M1 = the core loop only (develop -> review -> merge without the captain in the middle): T-12, T-13, T-15, T-17, T-18. Everything else to M2 as `wont` (not dispatched in this version) so never-stop does not pull it in. Rework fixes only what breaks the loop; other findings are logged. After M1 runs: a deletion pass (old manual scripts/ledger, Python-era design docs, duplicate notes, dead code) — check each item is not in use first.
- 2026-10-23 T-15 rework moves from runtime (T-17, T-12, T-18 queued) to board (its tasks went to M2). T-15 lives in its own module (quota.rs).

# Queue
## T-13 [must] lane=data milestone=M1 required
deps: -

## T-17 [must] lane=runtime milestone=M1 required
deps: -

## T-12 [must] lane=runtime milestone=M1 required
deps: -

## T-18 [must] lane=runtime milestone=M1 required
deps: T-17

## T-15 [must] lane=board milestone=M1 required
deps: -

## T-7 [wont] lane=board milestone=M2
deps: -

## T-8 [wont] lane=data milestone=M2
deps: -

## T-16 [wont] lane=board milestone=M2
deps: -
