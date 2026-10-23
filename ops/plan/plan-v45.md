<!-- gear-plan project=gear base=44 -->
# Milestone M2: Keep the captain on course
Done when ANY captain, in any project, is held to the goal by GEAR itself, not by its own discipline: every milestone carries a checkable done-sentence and quality bar; reviewers judge against it; findings outside it are logged, not reworked; rework rounds are capped; drift raises a direction review the captain must answer. Then: quota handling (T-15, captain quota), findings text in the store (T-21), the rest of m2-backlog.

# Decisions
- 2026-10-23 USER: holding the direction cannot depend on the captain's discipline or on this conversation — other captains in other projects must be held too. M2 starts with four mechanisms (T-22…T-25), ahead of T-15/T-21. `ops/REVIEW.md` is only this repo's file; the bar must travel in GEAR's own review packet.

# Queue
## NEW [must] lane=data milestone=M2 required
goal: A milestone carries its bar. The plan's `# Milestone <id>: <title>` block requires two fields: `done:` (one checkable sentence) and `bar:` (the quality line for this phase, e.g. "skeleton: block only loop breaks and data loss; log the rest"). `gear plan apply` rejects a milestone without them (BAD_FORMAT naming the milestone); they are stored with the milestone and shown on the captain home page and `gear --show T-n` (the task's milestone). Purpose: every task and finding is judged against one sentence the captain agreed with the user, in every project. Environment: existing plans without the fields keep applying only if they do not touch the milestone block (old milestones are grandfathered until edited).
accept: cargo test --workspace green; tests: a milestone without done/bar is rejected; with them it is stored and shown in the packet; an old milestone untouched by the draft still applies. Real proof: apply a scratch plan on a scratch store with the built binary.
deps: -

## NEW [must] lane=data milestone=M2 required
goal: The captain's gate in the automatic flow. (1) The engine-built review packet includes the task's milestone `done:` and `bar:` and the rule "fail only for a failed mechanical fact, a P0, or a finding that breaks the done-sentence; list everything else with its severity". (2) `gear send --kind review` takes an optional `--blocking <ids>` naming which findings block; settle_review sends back only the blocking ones in the rework note, and puts the rest as one information item for the captain ("logged findings for T-n"). (3) Rework rounds are capped by `[conventions].review_max_rounds` (default 3): when a task would be sent back past the cap, the engine does not rework it — it opens one captain action item "T-n reached N review rounds: pass with logged findings, or re-scope" and leaves the task Verifying. Purpose: the reviewer is a screw; the captain keeps the ship. Environment: the engine settles concurrently with `gear` commands; a restart between storing the review and settling must not double-settle.
accept: cargo test --workspace green; tests: packet contains done/bar and the rule; a fail with non-blocking findings only -> rework note lists only blocking ones, the rest go to one captain info item; round N+1 past the cap -> captain item, no rework, task stays Verifying; restart between review and settlement -> one settlement.
deps: -

## NEW [must] lane=runtime milestone=M2 required
goal: Drift raises a direction review. The engine keeps per milestone: age since its first task, open tasks on it, review rounds per task, lines added per hand-in (from the candidate's diff stat against the integration branch). When any crosses a threshold (`[timing]`: milestone_age_h default 8, rounds default = review_max_rounds - 1, a hand-in adding more than lines_per_task default 600) it opens ONE captain review item per milestone (design/11 review items: cause `drift`, detail lists what crossed) with the fixed question "Does the plan still serve the milestone's done-sentence? What is the critical path? Keep (with reason) or change (plan apply)". The item closes only through `gear plan apply` with a `# Review` conclusion (design/11). Environment: concurrent ticks, restarts; one item per milestone, updated not duplicated.
accept: cargo test --workspace green; engine tests: a task at round 2 of 3 -> one drift item; a second tick -> same item updated; a plan apply with a keep conclusion closes it; restart keeps one item.
deps: -

## NEW [should] lane=board milestone=M2
goal: The drift gauges are visible. The board and the captain snapshot show per milestone: its done-sentence, age, open tasks, the tasks with the most review rounds, lines added this milestone; a gauge past its threshold is marked. Read-only from the store.
accept: cargo test --workspace green; board contract test with one milestone over the age threshold and one task at round 3; PTY proof of the frame (rows at column 0).
deps: -

## T-15 [must] lane=data milestone=M2
deps: -

## T-21 [must] lane=board milestone=M2
deps: -
