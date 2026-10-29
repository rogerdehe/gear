# Review of T-351

Status: done
Verdict: pass
Reviewed: review/t351 @ 02404ce4   Reviewer: Codex (gear:review-11)   Round: 2
Contract: v1

## Mechanical acceptance

- Build/tests: independently ran `make check CRATE=gear-app`: exit 0, 562 passed, 0 skipped, 44.403s test time; compilation 1m 00s, zero compiler warnings.
- Build/tests: independently ran `make check CRATE=gear-board`: first run 190 passed and one unrelated PTY frame timeout; reran the same command, exit 0, all 191 passed, 0 skipped, 5.909s test time, zero compiler warnings. Initial compilation took 1m 25s including the shared build-directory wait; rerun compilation took 0.80s.
- Both task regressions pass: `decision_contract::a_moved_task_is_recorded_in_the_voyage_it_left` and `voyage_select_contract::a_task_that_left_the_voyage_is_named_but_not_counted`. The T-335 goal-change regression and T-362 landed-voyage tests also pass.
- Allowed files: fetched `origin/dev...02404ce4` isolates crates/gear-app/src/plan.rs, crates/gear-board/src/lib.rs, tests in those crates, and the mandated task report. The requested local `dev...02404ce4` includes already-integrated ancestor work because local dev is stale.
- `git merge-tree --write-tree dev 02404ce4` and the same against fetched origin/dev (d0019592) both exit 0 without conflicts. Diff whitespace checks are clean.
- Preserved prior review history by merging origin/review/t351. Relative to the candidate, this branch adds only review artifacts; candidate code remains untouched.

## Spec compliance

Contract v1 is satisfied. apply_queue records the exact prescribed move event before assigning the new milestone, only for distinct Some(old) and Some(new). The app regression checks the old voyage, exact text, new membership and absence of duplication on another apply.

ProjectBoard exposes moved_out and derives it from the shown voyage's move decisions. Store decisions are ordered by seq; filtering and collection preserve their order. Existing milestone scoping excludes the moved task from the old voyage's tasks and totals, as the board regression verifies.

Attempt 1 had no findings. The merge rework is resolved: both T-335's goal_changes and T-351's moved_out remain declared, computed and populated. Both appended sets of voyage-selection tests survive the T-362 merge. No new defect was found.

## Findings

None. No blocking or logged candidate findings.

## Set aside

- The first board run's `board_contract::a_long_graph_scrolls_and_a_tall_panel_keeps_its_borders` failed at board_contract.rs:1040 waiting five seconds for a frame. The candidate does not change that test, and the complete required command passed on rerun, including that test (3.248s). This transient test timeout does not establish a candidate defect.
- Moves involving None intentionally produce no event. Repeated departures and returns retain history under the explicitly prescribed stable event identity; no additional membership filter or event design is required.
- Malformed move rows require data outside the new production writer's exact format; no normal-use defect established.
- Drawing the moved-out line and exporting it through JSON are outside the explicit model-field contract.
- T-353's downstream branch and changes already integrated into origin/dev are outside this candidate review.
- No workspace gate was run; the merge gate owns `make gate`.

## Questions for the captain

None.

## Verdict

Pass. Required checks pass, merge rework preserves both sides, and no finding breaks the M5 done sentence or DELIVERABLE bar.
