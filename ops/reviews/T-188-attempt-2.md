# Review of T-188

Status: done
Verdict: pass
Reviewed: keel/t188 @ 44028791   Reviewer: Codex   Round: 2
Contract: v1; the attempt-2 packet limits rework to F2/F3 and assigns F1 to T-208.

## Mechanical acceptance
- build: `cargo build -p gear-engine --all-targets` → exit 0, zero warnings; 4m15s.
- tests: `scripts/test.sh` → exit 0; 1297 passed, 0 skipped, including all 16 overlap tests. Nextest execution 110.727 s; script-measured suite 114 s, below the 300 s hard limit. Test-binary build 420 s including the wait for the all-targets build. No compiler warnings. Slow-test warnings are recorded below.
- allowed files: ok; v1 has no explicit path allowlist. `git diff --name-only origin/dev...44028791` contains only `crates/gear-engine/src/parallel.rs`, `crates/gear-engine/tests/it/parallel_contract.rs`, and `ops/reports/T-188.md`. The requested local `dev..44028791` also shows voyage-plan drift; it is not a task-authored change (the task-only rework is commit 81f6c0b9 plus its report). Production files tested remain byte-identical to the candidate; review commits contain only review records.
- merges into dev: clean; independently ran `git merge-tree --write-tree dev 44028791` and `git merge-tree --write-tree origin/dev 44028791`, both exit 0, including a final repeat against fetched origin/dev.
- whitespace: `git diff --check 6aa75be6..44028791 -- crates/gear-engine/src/parallel.rs crates/gear-engine/tests/it/parallel_contract.rs` → clean.

## Spec compliance
- F2 resolved: `parallel::check` deduplicates on a persistent pair-notified marker, retained across an empty overlap. Previously stored item IDs also suppress duplicate notifications after an upgrade. File metadata is refreshed on each scan; an open/waiting item's text is updated without creating a new item. Tests independently passed for added shared files, overlap clearing and reappearing, another task pair, and a single captain prompt through engine ticks.
- F3 resolved: the two test-only scenarios are separate tests with their own no-item and no-scan-error assertions. The test-file case took 4.960 s; the contract-file case took 6.024 s, both below 10 s.
- F1 remains deliberately outside this rework: the captain assigned integration-overlap display to T-208 after T-163/T-197. The integration metadata transport remains intact, and the no-captain-item and information-clearing engine assertions pass.
- No new blocking product behavior found in the rework delta.

## Findings
F4 [P2, logged] Three regressions changed by this rework exceed the per-test 10-second warning threshold — `crates/gear-engine/tests/it/parallel_contract.rs:148`, `:286`, `:338`: `overlapping_branches_name_both_tasks_and_the_shared_file_once` took 19.062 s; `cleared_overlaps_resolve_and_the_pair_does_not_ring_again` took 10.776 s; `a_pair_rings_once_through_the_engine` took 17.033 s. Each executes repeated real Git scans, charging subsequent acceptance runs. Reduce repeated fixture/scan work while retaining the lifetime-notification assertions; the long first test's distinct-pair scenario could also be an independent test. Logged under ops/REVIEW.md's changed-test timing rule: all tests pass, the whole suite is 114 s, and this contention measurement establishes no new normal-use product failure or failed mechanical fact. It does not block under the milestone/STABLE verdict rule.

## Set aside
- Prior F1 is reassigned by explicit captain instruction, not silently treated as implemented. T-188's report still carries the original display claim, but its attempt-2 section records the reassignment.
- Resolved items retain their historical text; current files are stored in metadata, and only actionable items are refreshed. This preserves the captain's recorded answer without another notification.
- Existing cadence, branch selection, repo grouping, same-lane exclusion, and path filtering were reviewed for interaction with the new marker; no new delta issue found. Broader lane-rebinding behavior belongs to its own milestone work.
- Twenty-one other over-10-second warnings concern unchanged test bodies (including two existing parallel tests). Per ops/REVIEW.md, these are not failed mechanical facts of this rework. Timings from this run:

  - `gear-app::it base_gate::a_replay_needs_the_live_seat`: 11.3 s.
  - `gear-app::it base_gate::a_rework_that_keeps_the_earlier_candidate_is_accepted`: 10.5 s.
  - `gear-engine merge::tests::a_branch_that_moved_past_the_candidate_merges_the_candidate_only`: 11.3 s.
  - `gear-engine merge::tests::a_candidate_pushed_late_merges_after_the_retry`: 11.4 s.
  - `gear-engine merge::tests::a_candidate_on_another_branch_merges`: 12.2 s.
  - `gear-cli::it fixtures_contract::the_fixtures_build_replace_and_pass_their_gates`: 16.8 s.
  - `gear-engine merge::tests::a_failed_merge_is_retried_from_the_plan_and_its_item_closes`: 11.8 s.
  - `gear-engine merge::tests::a_killed_worker_recovers_from_origin_without_a_half_merge`: 10.5 s.
  - `gear-engine merge::tests::a_merge_waits_for_the_task_its_commits_carry`: 14.7 s.
  - `gear-engine merge::tests::a_merge_waits_for_an_earlier_attempt_of_the_task_it_carries`: 16.0 s.
  - `gear-engine merge::tests::a_push_race_is_retried_and_the_merge_lands`: 11.1 s.
  - `gear-engine merge::tests::a_pushed_head_is_reused_after_a_restart`: 13.3 s.
  - `gear-engine merge::tests::a_refused_land_is_retried_by_a_remerge`: 11.9 s.
  - `gear-engine merge::tests::dev_moving_during_the_gate_is_a_race`: 10.3 s.
  - `gear-engine merge::tests::a_task_merges_in_the_repo_of_its_lane`: 12.1 s.
  - `gear-engine merge::tests::a_second_ready_merge_waits_for_the_first_one`: 13.4 s.
  - `gear-engine merge::tests::the_engine_merges_a_candidate_on_another_branch_itself`: 11.4 s.
  - `gear-engine merge::tests::waiting_merges_take_turns_and_a_chain_lands_in_order`: 22.1 s.
  - `gear-engine::it parallel_contract::a_landed_task_is_not_an_open_change_and_open_tasks_still_pair`: 13.0 s.
  - `gear-engine::it multi_repo_contract::a_lane_in_a_child_repo_runs_from_onboard_to_merge_in_that_repo`: 17.1 s.
  - `gear-engine::it parallel_contract::periodic_scan_and_new_hand_in_trigger_at_their_required_cadence`: 15.4 s.

## Questions for the captain
- None.

## Verdict
- pass — F2 and F3 are resolved; F1 is explicitly reassigned; F4 is logged. Build, acceptance suite, allowed scope and merge checks pass.
