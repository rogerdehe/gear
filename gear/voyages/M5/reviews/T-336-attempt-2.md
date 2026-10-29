# Review of T-336

Status: done
Verdict: pass
Reviewed: review/t336 @ 8820df4d   Reviewer: Codex (gear:review-11)   Round: 2
Contract: v2

## Mechanical acceptance

- Build: independently ran `make check CRATE=gear-app` at the candidate; compilation completed in 53.62 seconds with zero warnings.
- Tests: the same command exited 0; 551 passed, 0 skipped in 447.006 seconds. The new captain-home acceptance test and all station-contract tests passed.
- Allowed files: source changes confined to home.rs and station.rs; integration tests and task report accompany them.
- Merge: `git merge-tree --write-tree dev 8820df4d` and the equivalent against freshly fetched `origin/dev` both succeeded without conflicts.
- Whitespace: `git diff --check dev...8820df4d` and the equivalent against `origin/dev` passed. Both comparisons show the same five changed paths.
- Candidate code untouched; review commits contain only this review artifact.

## Spec compliance

- Contract v2 is satisfied. The captain-only branch in `home::build_with_agents` selects `plan::current_voyage`, omits an empty milestone, and adds the shared `goal_text` immediately after the voyage report lines and before their trailing blank line. Lane and temp pages never enter this branch.
- Both captain messages in `station::checkpoints` pass `report.milestone` to the shared helper. It prepends the goal and one newline when present, and returns the existing body unchanged when absent. The landing instructions and station report content are preserved.
- Integration tests verify the exact home-page goal and station progress immediately after the report, the no-stations ending, absence on a lane page, and the goal-first station and landing bodies. Existing line-count and exact-body expectations were updated as required.

## Findings

None. No blocking or logged findings.

## Set aside

- Missing voyages and absent/blank done text: the existing shared `goal_text` returns None; the new callers add no goal line or extra newline. The empty current-voyage guard also preserves a captain page before departure. These branches were inspected in code; no defect found.
- Multiple voyages: home deliberately uses the latest departed voyage through the exact selector required by v2; each station item uses its own report's milestone, independent of that selector.
- Snapshot/replay behavior: existing station markers and immutable stored messages are unchanged; the quarter-snapshot and coalescing tests pass. Existing home read-only and view-recording tests also pass.
- Task-state completion and merge-time progress can differ: the shared goal text intentionally measures merged work. Changing that existing model or the landing trigger is outside this task's explicit wiring contract.
- The merge gate owns `make gate`; this review ran only the requested crate check. Milestone-wide board approval and release obligations remain with their own tasks.

## Questions for the captain

None.

## Verdict

Pass. Mechanical acceptance and Contract v2 are satisfied; no finding breaks the M5 done sentence or DELIVERABLE bar.

Review friction: the required crate check spent 447.006 seconds executing tests, including existing Git fixtures, after a 53.62-second compilation on the shared machine.
