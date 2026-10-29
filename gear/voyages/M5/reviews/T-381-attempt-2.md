# Review of T-381

Status: done
Verdict: pass
Reviewed: review/t381 @ b1fdb95b   Reviewer: Codex (gear:review-9)   Round: 2
Contract: v1

## Mechanical acceptance

- Build: `make check CRATE=gear-engine` exited 0 with zero compiler warnings; compilation took 3m 12s. No other build or test command was run.
- Tests: 707 passed, 0 failed, 0 skipped across two binaries in 305.557 seconds. All ten overrun tests passed, including both settlement regressions and the live-budget regression.
- Allowed files: `git diff dev...b1fdb95b` contains only overrun.rs and the required task report. Production edits stay within process apart from its required TaskState import; tests stay in the existing module.
- Merge: `git merge-tree --write-tree dev b1fdb95b` and the fetched `origin/dev` equivalent both exited 0 without conflicts. Both integration refs were 5a0b974e; both three-dot diffs passed `git diff --check`.
- Candidate code remains untouched. Review commits add only this review and preserve the prior round's review artifact by merging origin/review/t381 without a force push.

## Spec compliance

F1 from attempt 1 is resolved at `crates/gear-engine/src/overrun.rs:98`: filtering asked_at by !settled skips the answer-processing block after settlement and retains the non-live cleanup. The regression independently passed the exact ordering: budget question at T+1800, queued reviewer progress at T+1801, captain fail at T+1802, then ticks at 1803, 1804, 6000 and 9000 with an empty overrun item thread.

The full contract remains satisfied: review liveness requires Submitted/Verifying as well as no reviewer verdict and no end_at; settlement supplies the stop timestamp, falling back to now; open or waiting items for the same attempt on its review overrun thread are resolved with the specified text. The existing-item regression verifies the active clock stops at settlement and the resolution timestamp stays unchanged on later ticks. The Submitted live-budget and preceding-progress-before-reviewer-verdict tests both pass.

## Findings

None; no blocking or logged findings remain.

## Set aside

- Reviewer-verdict completion still processes preceding progress while the attempt remains Submitted/Verifying, as the existing regression requires. The new guard applies specifically to the settlement-state condition; development rounds are unaffected.
- Missing settlement timestamps fall back to now exactly as the contract requires. Cancellation clock redesign is outside this task. An old attempt remains non-Submitted when a later attempt reaches Verifying, and item cleanup filters by attempt number.
- Untouched tests over ten seconds included `merge::tests::waiting_merges_take_turns_and_a_chain_lands_in_order` (13.649 s), `multi_repo_contract::a_lane_in_a_child_repo_runs_from_onboard_to_merge_in_that_repo` (10.486 s), and `parallel_contract::overlapping_branches_name_both_tasks_and_the_shared_file_once` (10.118 s). Slow untouched tests do not block under the reviewer rules. The required check's compilation and suite duration consumed most of the review budget; reported as hand-in friction.
- Integration work inherited since attempt 1 is not part of this rework. The requested dev three-dot diff isolates this task's two paths; the task's rework production delta is the single answer guard. The merge gate runs make gate.

## Questions for the captain

None.

## Verdict

Pass. Mechanical acceptance passes, F1 is resolved, and no finding breaks the M5 done sentence or DELIVERABLE bar.
