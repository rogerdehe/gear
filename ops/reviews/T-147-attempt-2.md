# Review of T-147

Status: done
Verdict: pass
Reviewed: review/t147 @ a17abb65740e785d15881ed4982c29d61f773e92   Reviewer: gear:review-10 (Codex)   Round: 2

## Mechanical acceptance
- Build: `cargo build -p gear-app -p gear-engine --all-targets` exited 0, zero compiler warnings/errors, 4m 22s including its initial build-lock wait. The full-suite test build also had zero compiler warnings/errors. Logs: `/tmp/t147-review-build.log` and `/tmp/t147-review-tests.log`.
- Tests: `scripts/test.sh` exited 0: 1,215 passed, 0 failed, 0 skipped; nextest 134.484 s. The script reported build 135 s and suite 293 s (including a build-lock wait while the separately requested all-targets build ran between its two cargo invocations), below the mandatory 300 s limit. Every added/changed test passed in under 10 s, including all ownership, rollback, session, parser and delivery-evidence regressions.
- Independent diagnostic probe: `/tmp/t147-review-probe.rs`, compiled against the candidate libraries, passed. All four price fields reject negative, NaN and positive/negative infinity with their full field names; negative, string and overflowing grace values report `delivery_grace_s`; zero produces a validation issue; omitted grace defaults to 180.
- Allowed files: ok. Contract v6 explicitly allows tasks.rs and config.rs; it does not supply an exclusive two-file allowlist. The task-specific changes additionally implement caller ownership in mail.rs, nullable session refresh in gear-store, project timing and result evidence in the engine, their acceptance tests, and the report. These support the stated goal. The supplied `dev..candidate` comparison also includes the already merged T-195 profile implementation/review and voyage snapshot. Against fetched `origin/dev` c9b6624679b6275694c65e18b55fd76628dfe074, the candidate has only those ten T-147 paths. No unrelated task-specific edit found. `git diff --check dev..a17abb65` passed.
- Merges into dev: clean. `git merge-tree --write-tree dev a17abb65` exited 0, tree `4d4318515df5668d486d8db78c934e97e62efce2`. Against fetched origin/dev it also exited 0, tree `77599a3783697cc0e2112769f238b0a5393fad0b`.
- Candidate source is unchanged; review commits contain only this artifact.

## Spec compliance
- Contract v6 is satisfied in the reviewed code: an authenticated owning seat with the Delivered attempt's generation can submit, with Delivered → Started → Submitted and task Ready → Running → Verifying in the send transaction. Another seat or mismatched Delivered generation is refused. Contract errors roll back the implicit start and timestamps.
- Hand-in refreshes both session_ref and session_cli from the current binding, including clearing a stale session when the current binding has no known session. A Started attempt can retain ownership across a seat restart, while require_seat rejects a stale caller generation.
- Provider/model price tables load all four USD-per-million rates, allow integer numeric rates, reject invalid model keys and non-finite/negative rates, and name affected fields. timing.delivery_grace_s defaults to 180 and the engine reads each project's latest config within its project loop; the process-wide value is used only when the stored config cannot be parsed/read.
- Accepted hand-ins settle pending delivery evidence and pre-existing stall items without requiring a check-in view. Late hand-ins settle failed-delivery stall evidence once; later stall items are preserved.
- Attempt 1 was abandoned with no code/report, as recorded in the voyage and packet; no prior T-147 review findings exist to recheck. This is an independent review of attempt 2's requested diff.

## Findings
- None.

## Set aside
- Models' context_window_k, quota source parsing, session-selection/resume policy and price-based usage calculation belong to their separate tasks; v6 explicitly excludes context_window_k here. This review certifies the parser/session hand-in bookkeeping, not the entire voyage's live milestone demonstration.
- Existing TOML parse/type failures remain BadFormat diagnostics with field names; semantic invalid rates and zero grace are ConfigIssue values. This follows the established parse/validate split.
- The pending/late delivery changes are necessary supporting behavior: accepting a result while leaving an unconfirmed delivery to time out would produce a false stall after successful submission.

- 38 unchanged tests exceeded 10 s. Under ops/REVIEW.md these are recorded rather than blocking this task; no added/changed test exceeded 10 s. Build contention inflated script wall time, so both its wall time and actual nextest runtime are reported above.
  - `gear-app::it base_gate::a_replay_needs_the_live_seat` (11.1 s).
  - `gear-app::it base_gate::a_restricted_fetch_refspec_cannot_hide_a_stale_candidate` (10.2 s).
  - `gear-engine merge::tests::a_clean_merge_is_pushed_and_the_branches_are_gone` (10.1 s).
  - `gear-engine merge::tests::a_branch_that_moved_past_the_candidate_merges_the_candidate_only` (10.9 s).
  - `gear-engine merge::tests::a_conflict_leaves_dev_untouched_and_fails` (10.7 s).
  - `gear-engine merge::tests::a_candidate_pushed_late_merges_after_the_retry` (11.7 s).
  - `gear-engine merge::tests::a_candidate_on_another_branch_merges` (12.1 s).
  - `gear-cli::it fixtures_contract::the_fixtures_build_replace_and_pass_their_gates` (17.8 s).
  - `gear-engine merge::tests::a_failed_merge_is_retried_from_the_plan_and_its_item_closes` (13.5 s).
  - `gear-engine merge::tests::a_merge_that_carries_no_other_task_is_not_held` (10.7 s).
  - `gear-engine merge::tests::a_gate_that_could_not_run_is_retried_and_the_merge_lands` (14.3 s).
  - `gear-engine merge::tests::a_killed_worker_recovers_from_origin_without_a_half_merge` (15.2 s).
  - `gear-engine merge::tests::a_push_that_fails_once_is_retried_and_lands` (13.2 s).
  - `gear-engine merge::tests::a_merge_waits_for_an_earlier_attempt_of_the_task_it_carries` (22.3 s).
  - `gear-engine merge::tests::a_merge_waits_for_the_task_its_commits_carry` (21.8 s).
  - `gear-engine merge::tests::a_settled_review_op_without_a_review_key_still_merges` (10.2 s).
  - `gear-engine merge::tests::a_push_race_is_retried_and_the_merge_lands` (16.0 s).
  - `gear-engine merge::tests::a_retry_merges_once_and_pushes` (13.1 s).
  - `gear-engine merge::tests::dev_moving_during_the_gate_is_a_race` (12.0 s).
  - `gear-engine merge::tests::a_task_merges_in_the_repo_of_its_lane` (13.5 s).
  - `gear-engine merge::tests::a_second_ready_merge_waits_for_the_first_one` (14.9 s).
  - `gear-engine merge::tests::passed_work_still_merges_while_the_captain_is_on_quota` (12.8 s).
  - `gear-engine merge::tests::the_engine_merges_a_candidate_on_another_branch_itself` (15.2 s).
  - `gear-engine review::tests::direct_checkout_stashes_dirty_worktrees_and_reports_them` (12.2 s).
  - `gear-engine merge::tests::waiting_merges_take_turns_and_a_chain_lands_in_order` (28.6 s).
  - `gear-engine::it parallel_contract::integration_changes_since_the_branch_base_count_too` (16.4 s).
  - `gear-engine::it parallel_contract::git_read_failure_does_not_claim_that_existing_overlap_went_away` (18.1 s).
  - `gear-engine::it parallel_contract::disjoint_and_same_lane_branches_raise_nothing` (19.5 s).
  - `gear-engine::it parallel_contract::legacy_root_overlap_keeps_its_item_and_resolves_it` (18.5 s).
  - `gear-engine::it parallel_contract::cleared_overlaps_resolve_and_can_report_a_later_recurrence` (24.6 s).
  - `gear-engine::it parallel_contract::a_landed_task_is_not_an_open_change_and_open_tasks_still_pair` (25.2 s).
  - `gear-engine::it multi_repo_contract::a_lane_in_a_child_repo_runs_from_onboard_to_merge_in_that_repo` (32.9 s).
  - `gear-engine::it parallel_contract::overlapping_branches_name_both_tasks_and_the_shared_file_once` (30.8 s).
  - `gear-engine::it parallel_contract::fresh_file_sets_get_fresh_ids_and_ring_through_the_engine` (33.1 s).
  - `gear-engine::it parallel_contract::remote_tracking_task_refs_work_and_pair_state_survives_reopening` (19.7 s).
  - `gear-engine::it parallel_contract::the_scan_of_a_child_repo_reads_its_own_integration_branch` (18.7 s).
  - `gear-engine::it parallel_contract::tasks_of_different_repos_are_never_compared` (21.8 s).
  - `gear-engine::it parallel_contract::periodic_scan_and_new_hand_in_trigger_at_their_required_cadence` (40.7 s).

## Questions for the captain
- None.

## Verdict
Pass. Mechanical acceptance passes; no P0 or blocking STABLE-bar finding was found.
