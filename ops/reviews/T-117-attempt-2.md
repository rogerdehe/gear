# Review of T-117

Status: done
Verdict: fail
Reviewed: hull/t117 @ 792c34b3b6467dffc309c53f9053754d7a18a774   Reviewer: Codex (GPT-6)   Round: 2

## Mechanical acceptance
- build: `env -u GEAR_DB cargo build --workspace --all-targets` → exit 0, zero warnings.
- tests: `env -u GEAR_DB cargo test --workspace` → exit 0; 877 passed, 0 failed, 0 ignored, zero warnings. Includes five tailed unit tests and four all-tail engine integration tests.
- focused acceptance: `env -u GEAR_DB cargo test -p gear-engine --test it all_tail` → exit 0; 4 passed, 0 failed, zero warnings.
- allowed files: ok. Contract v1 has no path allowlist. Against the merge base with fetched origin/dev, the task changes only `crates/gear-engine/src/{lib,tailed}.rs`, `crates/gear-engine/tests/it/{main,all_tail_contract}.rs`, and `ops/reports/T-117.md`. Attempt 2's own commit changes only tailed.rs, its integration test, and the report.
- merges into dev: local `dev` (6332347) is clean (`git merge-tree --write-tree dev 792c34b` → exit 0). Fetched `origin/dev` (00294d4, with T-116 integrated) conflicts (`git merge-tree --write-tree origin/dev 792c34b` → exit 1) in `crates/gear-engine/src/lib.rs` and `crates/gear-engine/tests/it/main.rs`. F2 blocks this failed mechanical fact.
- candidate isolation: this review branch adds only review artifacts. The prior attempt's review history was merged into the review branch to preserve the remote branch; candidate production files are untouched.

## Spec compliance
- Attempt-1 F1 is resolved: a project-scoped marker survives captain resolution, suppresses further items throughout the active state, and clears when the condition ends even with no open item. Marker/message updates share the store transaction. Open/Waiting items still close on tail clearance or required work ending.
- The delta adds unit and engine regressions covering open → captain resolve → unchanged tick creates no item → clear tail → re-tail creates exactly one new item. Existing coverage checks the named lanes/tasks, one item over two ticks, task-ending closure, and the waiting-task case.
- No new behavioral defect found in the attempt-2 delta. The remaining blocker is integration with the now-landed T-116 work.

## Findings
F2 [P1, blocking] Candidate no longer merges cleanly into current integration — `crates/gear-engine/src/lib.rs:282` and `crates/gear-engine/tests/it/main.rs:43` — independently fetched origin/dev at 00294d4 and ran `git merge-tree --write-tree origin/dev 792c34b3b6467dffc309c53f9053754d7a18a774`; exit 1 reports both paths conflicted. T-116 adds dead-temp release at the same tick location and its integration module at the same module-list location as T-117. This fails the required clean-merge mechanical check and prevents done clause (3)'s batch-2 fixes from landing together. Smallest fix: merge current origin/dev into the writer branch; in lib.rs retain BOTH the `report.dead_temps.extend(release::release_dead_temps(...))` step and the `report.tail.extend(tailed::watch(...))` step before `start_reviews`; in tests/it/main.rs retain BOTH `mod dead_temp_slot_contract;` and `mod all_tail_contract;` (two conflict hunks, about 15 retained lines). Retain all other incoming T-116 changes. Run `cargo test --workspace` and the clean-merge check, then hand in the merge candidate. No redesign needed.

## Set aside
- Prior F1's required regression uses the same message-state update as captain Resolve; changing its state independently of the marker is exactly the behavior being checked.
- Nonempty marker semantics match the writer's two values ("1" and empty). Store transaction rollback prevents a failed insert from leaving a suppression marker behind.
- Running/Verifying tasks are already held by a lane, so Ready/Waiting filtering remains within the contract. Initial body snapshots, deferred items, retired seats, and projects without lanes remain as assessed in round 1; this delta does not change them.
- The requested local dev comparison contains inherited T-118/T-119 and planning history; these are not additional task edits. Differences showing missing newer T-116 work in the candidate are branch divergence, not intentional deletions; F2 explicitly requires preserving that incoming work.
- The test's joined function/body line and the comment's mistaken T-116 review reference are cosmetic and do not warrant rework under the DELIVERABLE bar.
- The milestone's live pi-lane proof, quota fallback, and release remain outside this task.

## Questions for the captain
None.

## Verdict
fail — F2 fails the clean-merge mechanical requirement; prior F1 is resolved.
