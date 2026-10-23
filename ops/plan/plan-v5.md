<!-- gear-plan project=gear base=4 -->
# Decisions
- 2026-10-22 USER — THE SOUL OF GEAR: lanes never stop. Parallel development is the default, not an option. A lane hands in a result and immediately takes its next task; it never waits for a review or a merge. Queues are kept 2–3 ready tasks deep per lane; a dependency is written only on code a task cannot be built without, never "wait until the other task is reviewed/merged" (lanes merge dev before handing in). Plan v1–v4 broke this: one task per lane and deps on tasks under review left runtime and data idle.

# Queue
## T-5 [must] lane=runtime milestone=M1 required
deps: T-1

## NEW [must] lane=runtime milestone=M1 required
goal: The engine keeps lanes from running dry (the soul of GEAR: lanes never stop). Each tick, for every active lane count its ready tasks (dependencies met, not in progress). When a lane has fewer than 2, put one action item in the captain's inbox "lane <x> has <n> ready task(s) left" (one open item per lane, updated while the condition lasts, closed when it recovers) so the captain's doorbell rings before the lane stops. Also when a lane is idle with no ready task: the item says "lane <x> is IDLE". Use the existing review-item / message mechanism, the smallest change.
accept: cargo test --workspace green; an engine test: lane with 1 ready task -> one captain item; second tick no duplicate; queue refilled -> item closed.
deps: -

## NEW [should] lane=runtime milestone=M1
goal: A contract update reaches a lane mid-attempt (lesson: T-1 worked to v1 after v2 was applied). When `plan apply` bumps the contract of a task with an attempt in progress, the engine delivers one line to that lane at its next idle point: "T-<n> contract changed to v<k>: run `gear` and `gear --show T-<n>`", and the lane's next `gear` view adopts it (the attempt's adopted version moves to the current one).
accept: cargo test --workspace green; engine test: contract bump during an attempt -> one delivery; after the lane's view the attempt's adopted version is current.
deps: -

## T-6 [must] lane=data milestone=M1 required
deps: -

## NEW [must] lane=data milestone=M1 required
goal: The captain's home page (`gear` as captain) starts with a one-screen snapshot before the inbox: per lane — current task and state, number of ready tasks queued; deliveries not confirmed; temps in use / quota. Read-only, from the store. Keep it under 15 lines.
accept: cargo test --workspace green; a test renders the captain home with 2 lanes and checks the snapshot lines come before INBOX.
deps: -

## NEW [should] lane=data milestone=M1
goal: Kickoff support (PRD §8a): `gear admin kickoff` prints, for the captain to turn into a proposal: the agent kinds Herdr supports that are installed on PATH (opencode, codex, claude…), for each the models it can use (opencode: `opencode models`; codex: slugs in ~/.codex/models_cache.json), and a gear.toml skeleton with [quota], [[lanes]], [temps], [models] (lanes = fast model, review = a different stronger one, on_quota = fallback then wait, quota_reset_h = 5). Read-only, no Herdr writes.
accept: cargo test --workspace green; unit tests for the parsers on captured outputs; the command runs on this machine (paste its output in the report).
deps: -

## NEW [should] lane=board milestone=M1
goal: The board makes a lane running dry visible: a lane with no ready task is marked `DRY` and one with 1 ready task `LOW` next to its name; the totals line shows how many lanes are working now.
accept: cargo test --workspace green; board contract test with one dry lane and one low lane.
deps: -

# Verdicts
- T-2 attempt=1 contract=1 candidate=2d8fa06 pass evidence=ops/reviews/T-2.md
