# Review of T-148

Status: done
Verdict: fail
Reviewed: review/t148 @ 9312a22e6c111617fc16429b92710bfa571c8e7a   Reviewer: Codex   Round: 1
Contract: v3

## Mechanical acceptance
- Build: `cargo build --workspace --all-targets` — passed, zero warnings (5.17 s with test artifacts built). The initial narrower build was interrupted to stop build-lock contention; the completed workspace build includes both changed crates.
- Tests: `scripts/test.sh` — exit 0; 1,142 passed, 0 failed, 0 skipped. Nextest execution 153.912 s; script suite elapsed 249 s (includes a build-lock wait), build 348 s. The suite remains below the 300 s red line. No changed test exceeded 10 s.
- Independent probe: compiled a standalone `/tmp` harness against the candidate's built libraries, reusing its public fake-reader API and in-memory fixture. Observed: left→spent after a periodic read made only one API call and incorrectly parked as provider error; an unreadable source made three calls in eight seconds without signals; seeding the committed park prefix without its uncommitted deadline left Parked at reset; seeding the committed Active wake prefix without its uncommitted Continue produced zero Intended ops. These verify F1–F3 without editing candidate code; the prefix fixtures simulate the exact transaction boundaries, rather than killing a live process.
- Allowed files: ok — quota.rs and config.rs are explicitly authorized; the config contract test and task report accompany them. No other implementation files changed.
- Merges into dev: clean — `git merge-tree --write-tree dev 9312a22e6c111617fc16429b92710bfa571c8e7a` exits 0. `git diff --check dev..9312a22` also passes.

## Spec compliance
Config parsing uses gear_app::config and the live API delegates to gear_usage::read_quota. Fake-reader tests cover spent, quota-left, unreadable and stale readings. The happy-path spent park opens/closes a quota pause, records paused seconds and queues the exact Continue message without switching sessions. Missing: a fresh read on a new signal, bounded periodic retries on an unreadable provider, restart-safe park/resume persistence, and a usable Codex source. These affect §9.1–9.2 and the STABLE bar. The captain owns the separate live MiniMax proof of done clause (1); unit tests do not establish it.

## Findings
F1 [P1, blocking] A new signal can make its decision from a reading taken before the stop — `crates/gear-engine/src/quota.rs:218` and `:1118`. SIGNAL_MIN_AGE_S permits a 60-second cache hit. Example: the periodic read reports quota left at t=0, the provider becomes exhausted at t=4, and the confirmed stop at t=8 still uses t=0. The lane takes the five-minute provider-error branch instead of parking to the actual reset with its round paused. This breaks done clause (1)'s provider-reset behavior and the STABLE bar in ordinary quota exhaustion. Smallest fix: in apply_quota_policy_with/read_source, force a provider read on the start of a new signal episode (sharing a read made after that signal across confirmation ticks/seats is fine); keep periodic caching separate. About 15–30 lines plus a fake-reader test which changes left to spent between the periodic read and signal and asserts the new reset and pause.
F2 [P1, blocking] An unreadable source is retried every engine tick, even with no quota signal or while all lanes are parked — `crates/gear-engine/src/quota.rs:1167`. Refresh scheduling uses only the last successful read_at; an error leaves the source continuously due. One ordinary API outage therefore causes synchronous curl calls (up to 15 seconds each) on every tick, delaying all subsequent engine work and repeatedly logging the same failure instead of reading every five minutes. This breaks the STABLE stability/usability bar and §9.1's polling cadence. Smallest fix: persist a per-source last-attempt timestamp on success and failure and schedule periodic refreshes from it, preserving the old successful reading/read_at for stale marking; a new signal may still force its own read. About 20–30 lines in refresh_readings/read_source plus a fake failing-reader call-count test at t=0,4,8,300.
F3 [P1, blocking] A single engine restart between committed writes can strand the lane or corrupt its paused time — `crates/gear-engine/src/quota.rs:1422` and `:1563`. park_seat commits Parked/open_pause before store_wait commits the deadline; a restart between them leaves a Parked lane with neither quota_wait nor the legacy resume deadline, so it never wakes. Conversely, wake_from_source separately commits close_pause, add_round_pause, Active, Continue and clearing the wait. A restart after Active but before Continue makes future ticks skip wake_from_source entirely; a restart after add_round_pause but before Active adds the same pause again on retry. This breaks done clause (1)'s automatic continuation without hand fixes and the STABLE restart bar. Smallest fix: make each park a single transaction containing seat state, pause and Wait metadata; make wake a single transaction containing pause close/add, state transition, Continue op and metadata/counter clearing. Propagate accounting errors instead of discarding them with `let _`. Reuse the existing Tx APIs and finish_switch pattern; about 60–100 lines rearranged, with restart-boundary/atomicity tests proving no parked state lacks its deadline and no Active wake lacks its Continue.
F4 [P1, blocking] The accepted `reader = "codex"` source can never produce a reading — `crates/gear-engine/src/quota.rs:1134`. read_source unconditionally opens an API-key file, although Codex needs no key, and always calls api.read with codex_session=None. gear_usage::read_quota(Codex, ...) explicitly rejects a missing reviewer session, even if a key file was supplied. Thus §9.1's supported Codex source stays unreadable permanently during normal reviewer operation. This breaks the STABLE normal-use bar and the task's configured-provider reading requirement. Smallest fix: for Codex, bypass key_of and pass the current Codex reviewer's binding/session reference from this project's observed seats/store; retain the key path for HTTP readers and the existing reviewer chain policy. About 25–45 lines in quota.rs threading the session into refresh_readings/read_source, plus a fake-reader assertion that a configured Codex source with no key file receives its session and persists a reading. No reviewer Parked-state change is needed.

## Set aside
- The board currently ignores the stored stale boolean. This task's acceptance asks for a stale reading to be marked, which the engine test demonstrates; rendering belongs to the board task and is outside the allowed implementation files.
- Reviewer parking/fallback behavior is unchanged. F4 concerns reading a configured source, not adding a Parked state for reviewers.
- Error-episode counters survive healthy ticks. “Second time in a row” does not specify what successful observation ends an episode sequence, so this review does not prescribe a new reset policy.
- Live provider credentials and the five-hour MiniMax run were not used in mechanical acceptance; the packet assigns that proof to the captain.

- Existing tests over 10 s are outside this diff and are set aside under ops/REVIEW.md §2; none is a failed mechanical fact for T-148. Measured names and times:
  - `gear-app::it base_gate::a_failing_fetch_is_refused_not_ignored` — 10.1 s.
  - `gear-app::it base_gate::a_candidate_that_contains_the_integration_branch_is_accepted` — 10.3 s.
  - `gear-app::it base_gate::a_deleted_remote_integration_branch_is_refused` — 10.6 s.
  - `gear-app::it base_gate::a_committed_hand_in_replays_its_receipt_after_a_fetch_failure` — 11.4 s.
  - `gear-app::it base_gate::a_candidate_behind_the_integration_branch_is_refused` — 11.9 s.
  - `gear-app::it base_gate::a_current_candidate_is_accepted` — 12.7 s.
  - `gear-app::it base_gate::a_branch_deleted_on_the_remote_is_refused` — 14.6 s.
  - `gear-app::it base_gate::a_replay_needs_the_live_seat` — 17.9 s.
  - `gear-app::it base_gate::a_stale_hand_in_stores_nothing` — 12.2 s.
  - `gear-app::it cancel_contract::a_done_task_whose_merge_was_given_up_is_cancelled_with_its_items` — 11.0 s.
  - `gear-app::it base_gate::a_restricted_fetch_refspec_cannot_hide_a_stale_candidate` — 17.6 s.
  - `gear-app::it base_gate::an_unpushed_candidate_is_refused` — 14.7 s.
  - `gear-app::it cancel_contract::a_done_task_whose_work_is_merged_cannot_be_cancelled` — 14.8 s.
  - `gear-engine merge::tests::a_candidate_that_is_not_fetchable_yet_is_retried` — 10.5 s.
  - `gear-engine merge::tests::a_candidate_on_no_pushed_ref_fails_with_a_clear_reason` — 11.4 s.
  - `gear-engine merge::tests::a_busy_worktree_waits_without_running_a_second_merge` — 14.1 s.
  - `gear-engine merge::tests::a_conflict_leaves_dev_untouched_and_fails` — 14.0 s.
  - `gear-engine merge::tests::a_candidate_on_no_pushed_ref_fails_the_op_and_rings_the_captain_once` — 15.0 s.
  - `gear-engine merge::tests::a_clean_merge_is_pushed_and_the_branches_are_gone` — 15.4 s.
  - `gear-engine merge::tests::a_branch_that_moved_past_the_candidate_merges_the_candidate_only` — 16.4 s.
  - `gear-engine merge::tests::a_candidate_on_another_branch_merges` — 18.8 s.
  - `gear-engine merge::tests::a_candidate_pushed_late_merges_after_the_retry` — 20.2 s.
  - `gear-cli::it fixtures_contract::the_fixtures_build_replace_and_pass_their_gates` — 26.0 s.
  - `gear-engine merge::tests::a_failing_gate_leaves_dev_untouched_and_fails` — 14.7 s.
  - `gear-engine merge::tests::a_gate_that_is_still_running_does_not_block_the_merge_step` — 14.5 s.
  - `gear-engine merge::tests::a_failed_merge_is_retried_from_the_plan_and_its_item_closes` — 19.0 s.
  - `gear-engine merge::tests::a_merge_that_carries_no_other_task_is_not_held` — 15.4 s.
  - `gear-engine merge::tests::a_gate_that_could_not_run_is_retried_and_the_merge_lands` — 17.8 s.
  - `gear-engine merge::tests::a_killed_worker_recovers_from_origin_without_a_half_merge` — 16.8 s.
  - `gear-engine merge::tests::a_remerge_is_refused_without_a_failed_merge` — 12.2 s.
  - `gear-engine merge::tests::a_merge_waits_for_an_earlier_attempt_of_the_task_it_carries` — 29.0 s.
  - `gear-engine merge::tests::a_merge_waits_for_the_task_its_commits_carry` — 27.7 s.
  - `gear-engine merge::tests::a_push_that_fails_once_is_retried_and_lands` — 17.1 s.
  - `gear-engine merge::tests::a_settled_review_op_without_a_review_key_still_merges` — 20.1 s.
  - `gear-engine merge::tests::a_retry_merges_once_and_pushes` — 21.2 s.
  - `gear-engine merge::tests::a_push_race_is_retried_and_the_merge_lands` — 23.5 s.
  - `gear-engine merge::tests::dev_moving_during_the_gate_is_a_race` — 22.6 s.
  - `gear-engine merge::tests::a_task_merges_in_the_repo_of_its_lane` — 23.3 s.
  - `gear-engine merge::tests::a_second_ready_merge_waits_for_the_first_one` — 24.9 s.
  - `gear-engine merge::tests::dispatch_and_doorbells_continue_while_the_gate_runs` — 14.7 s.
  - `gear-engine merge::tests::passed_work_still_merges_while_the_captain_is_on_quota` — 12.6 s.
  - `gear-engine merge::tests::the_engine_merges_a_candidate_on_another_branch_itself` — 15.2 s.
  - `gear-engine onboard::tests::an_old_gear_hooks_path_is_unset_on_onboarding` — 10.3 s.
  - `gear-engine review::tests::real_git_checkout_uses_the_submitted_commit` — 13.0 s.
  - `gear-engine review::tests::direct_checkout_stashes_dirty_worktrees_and_reports_them` — 19.6 s.
  - `gear-engine merge::tests::waiting_merges_take_turns_and_a_chain_lands_in_order` — 33.5 s.
  - `gear-engine::it parallel_contract::integration_changes_since_the_branch_base_count_too` — 14.0 s.
  - `gear-engine::it parallel_contract::git_read_failure_does_not_claim_that_existing_overlap_went_away` — 15.3 s.
  - `gear-engine::it parallel_contract::disjoint_and_same_lane_branches_raise_nothing` — 15.8 s.
  - `gear-engine::it parallel_contract::legacy_root_overlap_keeps_its_item_and_resolves_it` — 15.0 s.
  - `gear-engine::it parallel_contract::cleared_overlaps_resolve_and_can_report_a_later_recurrence` — 18.2 s.
  - `gear-engine::it parallel_contract::a_landed_task_is_not_an_open_change_and_open_tasks_still_pair` — 20.1 s.
  - `gear-engine::it multi_repo_contract::a_lane_in_a_child_repo_runs_from_onboard_to_merge_in_that_repo` — 25.5 s.
  - `gear-engine::it parallel_contract::overlapping_branches_name_both_tasks_and_the_shared_file_once` — 21.4 s.
  - `gear-engine::it parallel_contract::fresh_file_sets_get_fresh_ids_and_ring_through_the_engine` — 25.4 s.
  - `gear-engine::it parallel_contract::remote_tracking_task_refs_work_and_pair_state_survives_reopening` — 13.3 s.
  - `gear-engine::it parallel_contract::the_scan_of_a_child_repo_reads_its_own_integration_branch` — 12.4 s.
  - `gear-engine::it parallel_contract::tasks_of_different_repos_are_never_compared` — 14.1 s.
  - `gear-engine::it parallel_contract::periodic_scan_and_new_hand_in_trigger_at_their_required_cadence` — 27.2 s.

## Questions for the captain
- Confirm the separately observed live MiniMax park/reset/Continue proof for done clause (1) before marking that clause complete.

## Verdict
Fail: F1, F2, F3 and F4 block under the milestone's done clause (1) and/or STABLE bar. Each has a local fix within the task's allowed implementation files.
