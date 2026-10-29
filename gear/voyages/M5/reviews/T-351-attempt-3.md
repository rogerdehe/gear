# Review of T-351

Status: done
Verdict: pass
Reviewed: review/t351 @ 43266e0e   Reviewer: Codex (gear:review-11)   Round: 3
Contract: v1

## Mechanical acceptance

- Independently ran only `make check CRATE=gear-app`: exit 0, 562 passed (4 slow), 0 skipped, 92.410s test time; compilation 40.23s, zero compiler warnings.
- Independently ran `make check CRATE=gear-board`: exit 0, 193 passed, 0 skipped, 51.389s test time; compilation 1m 05s including the shared build-directory lock wait, zero compiler warnings.
- Both required regressions pass: `decision_contract::a_moved_task_is_recorded_in_the_voyage_it_left` and `voyage_select_contract::a_task_that_left_the_voyage_is_named_but_not_counted`. All board golden-frame tests pass.
- Allowed files: fetched origin/dev (e04aa0a7) isolates crates/gear-app/src/plan.rs, crates/gear-board/src/lib.rs, tests in their crates, and the mandated task report.
- `git merge-tree --write-tree dev 43266e0e` and the same against origin/dev exit 0 without conflicts. `git diff --check origin/dev...43266e0e` is clean.
- Preserved prior review artifacts by merging origin/review/t351. Relative to the candidate, this branch adds only review artifacts; candidate code remains untouched.

## Spec compliance

Contract v1 is satisfied. apply_queue records the exact prescribed move event before assigning the new milestone, only for distinct Some(old) and Some(new). The app regression checks the old voyage, exact text, new membership and absence of duplication on another apply.

ProjectBoard exposes moved_out and derives it from the shown voyage's move decisions. Store decisions are ordered by seq; filtering and collection preserve that order. Existing milestone scoping excludes the moved task from the old voyage's tasks and totals, as the board regression verifies.

Attempts 1 and 2 had no candidate findings. Attempt 3 resolves the subsequent integration compile failure: its board regression reads all shown lanes' tasks instead of calling T-363's removed token_tasks helper. The moved-out tuple, graph exclusion, lane exclusion and zero-task total assertions remain. The merged production code preserves both moved_out and goal_changes.

## Findings

None. No blocking or logged candidate findings.

## Set aside

- Local dev is stale; its broader diff includes already integrated tasks.
- The requested local-dev whitespace check flags inherited golden-frame trailing spaces; the isolated T-351 diff against fetched origin/dev is clean. These frame changes are not candidate findings.
- Moves involving None intentionally produce no event. Repeated departures and returns retain history under the explicitly prescribed stable event identity; no additional membership filter or event design is required.
- Malformed move rows require data outside the new production writer's exact format; no normal-use defect established.
- Drawing the moved-out line and exporting it through JSON are outside the explicit model-field contract.
- T-353's downstream branch is outside this candidate review. No workspace gate was run; the merge gate owns `make gate`.
- Four existing app base_gate tests were marked slow but passed; no candidate defect established.

## Questions for the captain

None.

## Verdict

Pass. Required checks pass, attempt 3 resolves the removed-helper compile failure, and no finding breaks contract v1 or the M5 DELIVERABLE bar.
