<!-- gear-plan project=gear base=21 -->
# Decisions
- 2026-10-22 Rework so far came from scenarios the contracts did not name (empty data, captain changes state mid-flow, engine restart between steps, one item's error). Not-yet-started engine tasks get them in their acceptance now. Rule for all engine code: one item's error never stops the tick.

# Queue
## T-12 [must] lane=runtime milestone=M1 required
accept: cargo test --workspace green; tests over a scratch git repo (tempdir with a bare origin): clean merge -> pushed + branches gone; conflict -> dev untouched + Failed; gate fails -> dev untouched + Failed. Adversarial (one test each): the engine restarts between merge and push (no double merge, no lost push); the task is cancelled after the merge op was queued (op voided, nothing merged); a merge failure of one task does not stop the tick for other seats/projects.
deps: -

## T-7 [must] lane=runtime milestone=M1 required
accept: cargo test --workspace green; an engine test: lane with 1 ready task -> one captain item; second tick no duplicate; queue refilled -> item closed. Adversarial: a lane with no tasks at all; a lane that is parked/held (no warning); engine restart while an item is open (no duplicate).
deps: -

## T-8 [should] lane=data milestone=M1
accept: cargo test --workspace green; engine test: contract bump during an attempt -> one delivery; after the lane's view the attempt's adopted version is current. Adversarial: two bumps before the lane looks (one delivery, adopts the latest); bump while the attempt is already handed in (no delivery; the review/verdict sees the version rule); engine restart between bump and delivery.
deps: -
