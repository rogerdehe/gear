# Review of T-150

Status: done
Verdict: fail
Reviewed: review/t150 @ 5c38e1e   Reviewer: Codex   Round: 2

## Mechanical acceptance
- build: `cargo build --workspace --all-targets` exited 0, but generated two unused-variable warnings in `contract_update.rs:122` and `:168`; the mandatory zero-warning check fails (F3).
- tests: `scripts/test.sh` exited 0: 1044 passed, 0 failed, 0 skipped; build 115 s, suite 93 s (nextest 87.775 s), below the 300 s red line. All six delivery_grace_contract cases and all four contract_update cases passed. No changed test exceeded 10 s.
- allowed files: ok. Contract v1 has no explicit path whitelist; the 15 paths in `dev..5c38e1e` implement delivery/doorbell behavior, update affected assertions, or document the report. The rework delta is confined to dispatch, confirmation, stall cleanup, their tests and the report.
- merges into dev: clean. After fetching, `git merge-tree --write-tree origin/dev 5c38e1e` exited 0 against origin/dev `eb9f3e7`.
- Review commits change only review records; candidate code remains unchanged. The prior review is preserved in `ops/reviews/T-150-attempt-1.md`.

## Spec compliance
- The four functional acceptance cases pass: a view 100 s after delivery confirms it, a late view confirms a failed delivery and closes its stall, Started attempts receive no second Deliver, and the taskless doorbell matches §10.4.
- Previous F1 is resolved: Adopt for a Started attempt inserts an urgent instruction message rather than a Deliver; the full tick test proves its doorbell reaches the lane and the next view adopts the version.
- Previous F2 is resolved: normal retry confirmation closes the stall on Delivered → Started, and the failed-op late-view path also handles an already Started attempt. The retry-then-view regression passes in the real confirmation order.
- Functional scope complies with contract v1. Mechanical build cleanliness does not.

## Findings
F3 [P3, blocking — failed mechanical fact] The rework leaves two unused `report` bindings and therefore fails the required zero-warning build — `crates/gear-engine/tests/it/contract_update.rs:122` and `:168`. Both `scripts/test.sh` and `cargo build --workspace --all-targets` report these warnings. This blocks under `gear --skill review` §1.2's explicit mechanical requirement, independently of the milestone behavior; no runtime defect is claimed. Smallest fix: in `a_bump_is_delivered_once_and_the_view_adopts_it` and `two_bumps_before_the_lane_looks_give_one_delivery_with_the_latest`, remove `let report = ` from the first tick call (two lines), retaining the call and `.unwrap()`. Re-run the all-target build with zero warnings and the required suite.

## Set aside
- The project setting's parser/wiring remains the T-147 integration responsibility identified by the first review. This re-review judges the engine knob/default and the rework delta; it does not expand that previously accepted task split.
- Delivered-result acceptance, session policy and overrun questions belong to other tasks.
- Historical failed deliveries remain scanned for late evidence; the new timestamp bound prevents a historical view from closing a stall created after that view. No additional blocking case established.
- Untouched slow tests are not a failed mechanical fact of T-150 under ops/REVIEW.md. Observed warnings (all outside the changed tests):
  - 13.5 s  gear-app::it base_gate::a_replay_needs_the_live_seat
  - 11.3 s  gear-app::it base_gate::a_restricted_fetch_refspec_cannot_hide_a_stale_candidate
  - 10.3 s  gear-app::it cancel_contract::a_done_task_whose_work_is_merged_cannot_be_cancelled
  - 10.9 s  gear-app::it base_gate::an_unpushed_candidate_is_refused
  - 11.0 s  gear-engine merge::tests::a_busy_worktree_waits_without_running_a_second_merge
  - 11.1 s  gear-engine merge::tests::a_conflict_leaves_dev_untouched_and_fails
  - 11.8 s  gear-engine merge::tests::a_candidate_on_no_pushed_ref_fails_the_op_and_rings_the_captain_once
  - 13.1 s  gear-engine merge::tests::a_branch_that_moved_past_the_candidate_merges_the_candidate_only
  - 13.1 s  gear-engine merge::tests::a_candidate_on_another_branch_merges
  - 13.3 s  gear-engine merge::tests::a_candidate_pushed_late_merges_after_the_retry
  - 18.4 s  gear-cli::it fixtures_contract::the_fixtures_build_replace_and_pass_their_gates
  - 14.5 s  gear-engine merge::tests::a_failed_merge_is_retried_from_the_plan_and_its_item_closes
  - 10.1 s  gear-engine merge::tests::a_gate_that_is_still_running_does_not_block_the_merge_step
  - 10.9 s  gear-engine merge::tests::a_merge_that_carries_no_other_task_is_not_held
  - 13.4 s  gear-engine merge::tests::a_gate_that_could_not_run_is_retried_and_the_merge_lands
  - 13.6 s  gear-engine merge::tests::a_killed_worker_recovers_from_origin_without_a_half_merge
  - 11.1 s  gear-engine merge::tests::a_merged_attempt_gets_its_merge_time
  - 12.0 s  gear-engine merge::tests::a_push_that_fails_once_is_retried_and_lands
  - 20.7 s  gear-engine merge::tests::a_merge_waits_for_the_task_its_commits_carry
  - 10.2 s  gear-engine merge::tests::a_retry_merges_once_and_pushes
  - 21.5 s  gear-engine merge::tests::a_merge_waits_for_an_earlier_attempt_of_the_task_it_carries
  - 14.8 s  gear-engine merge::tests::a_push_race_is_retried_and_the_merge_lands
  - 12.5 s  gear-engine merge::tests::a_second_ready_merge_waits_for_the_first_one
  - 11.2 s  gear-engine merge::tests::a_task_merges_in_the_repo_of_its_lane
  - 16.7 s  gear-engine merge::tests::waiting_merges_take_turns_and_a_chain_lands_in_order
  - 11.0 s  gear-engine::it parallel_contract::fresh_file_sets_get_fresh_ids_and_ring_through_the_engine
  - 11.9 s  gear-engine::it multi_repo_contract::a_lane_in_a_child_repo_runs_from_onboard_to_merge_in_that_repo
  - 10.2 s  gear-engine::it parallel_contract::overlapping_branches_name_both_tasks_and_the_shared_file_once
  - 12.7 s  gear-engine::it parallel_contract::periodic_scan_and_new_hand_in_trigger_at_their_required_cadence

## Questions for the captain
- The STABLE bar allows one rework round, which this attempt has consumed. F3 is a two-line mechanical cleanup; the captain must decide its disposition under that round limit. This review does not waive the mandatory zero-warning gate.

## Verdict
fail — F3 is the sole blocker, a failed mechanical fact. Both previous functional blockers are resolved.
