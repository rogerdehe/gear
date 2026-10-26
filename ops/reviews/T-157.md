# Review of T-157

Status: done
Verdict: fail
Reviewed: deck/t157 @ 19f4c3d   Reviewer: Codex (GPT-6)   Round: 1
Contract: v1

## Mechanical acceptance

- Build: `cargo build --workspace --all-targets` exits 0, zero compiler warnings.
- Tests: `scripts/test.sh` exits 0: 1017 passed, 0 skipped; nextest 64.241 s, script wall time 66 s, build 112 s. All six new graph tests pass (0.052–0.250 s).
- Allowed files: no explicit path whitelist in contract v1. Candidate changes are graph.rs, board lib.rs, graph_contract.rs, test registration, and ops/reports/T-157.md, all in scope.
- Merge: `git merge-tree --write-tree dev 19f4c3d` exits 0 without conflicts, including after dev advanced to 6b47b33. Its merge base is 58891c2. The later two-dot diff also shows gear/voyages/M4.md because dev advanced; the candidate did not edit that plan relative to its merge base.
- Independent fixture probe: compiled a temporary Rust executable outside the repository against this build, reusing the candidate's world() fixture. Set T-141's acceptance verdict to pass, inserted a review round and fail review verdict for T-142, then called tasks(), panel(), and run_with() with Graph and injected j/Enter/q. Output: `REVIEW ORDER: ["T-142", "T-141"]`; panel shows `review attempt 1 codex 0m 0k tokens` without its fail verdict; interactive output has neither ROUNDS nor selection marks. No candidate source was edited.

## Spec compliance

The renderer implements the symbols, merge lines and crossings, dependency labels and dependency marks when selected is passed explicitly. The standalone panel reads rounds before falling back to attempts and lists both dependency directions. Missing: reachable live selection/Enter details, the prototype's combined review/merging row order, and review outcomes in panels backed by rounds. These break done-sentence (2), matching the prototype's views and keys on live data, and the STABLE usability bar.

## Findings

F1 [P1, blocking] Graph selection and Enter details are unreachable in the board — `crates/gear-board/src/lib.rs:640`. The only live graph call passes selected=None; run_with at line 849 handles only quit and d, and no production caller invokes graph::panel. Starting in Graph and injecting j/Enter/q never produces selection marks or ROUNDS. Contract v1 explicitly requires the selected row's marks and the task panel on Enter; deferring the entire connection to T-156 leaves those behaviors absent. Breaks done-sentence (2) and STABLE normal-use usability. Smallest fix: in lib.rs's graph interaction path retain a selected task index, bound j/k or the frame's up/down events to graph length, pass the index into graph::lines, and call graph::panel for that task on Enter using the existing overlay mechanism when available (otherwise a minimal closable details state). About 45–70 lines plus an injected-key fixture test proving both marks and panel contents. Keep the general frame/tabs/mouse rewrite in T-156.

F2 [P1, blocking] Review and merging rows are regrouped instead of preserving prototype order — `crates/gear-board/src/graph.rs:162`. The prototype uses one rank-preserving group for stages review OR merging. order() emits every Review before every Merging. In the prototype fixture T-141 is merging and precedes reviewing T-142; with that acceptance verdict populated the candidate reverses them. The submitted fixture omits T-141's acceptance verdict, hiding the discrepancy. Breaks done-sentence (2)'s exact prototype view and the graph-lines acceptance. Smallest fix: in order(), append queued and working groups, then a single filter matching Review | Merging, then sorted done (about 5–8 lines). Seed T-141 as accepted and assert its order before T-142; extend the fixture comparison to cover the prototype's merging row rather than treating both as review.

F3 [P1, blocking] Stored rounds suppress the review outcome — `crates/gear-board/src/graph.rs:383`. Once rounds_of_task returns any rows, the panel prints only role/attempt/CLI/time/tokens. Unlike its attempt fallback, it never reads the review verdict. A normal failed review with a stored review round therefore loses the prototype's `→ fail: rework`; passed reviews likewise lose `→ pass`, and pending reviews have no running marker. The agreed prototype README requires rounds with time, tokens, and verdict. Breaks done-sentence (2) and STABLE usability: users cannot tell how a review round ended from its details. Smallest fix: for each stored review round in panel(), look up get_review_verdict(project, task, round.attempt) and append pass/fail: rework, or a pending/running marker if absent (about 10–15 lines). Add stored-round fixtures for pass, fail, and pending; retain the rounds-first rule.

## Set aside

- The frame, five-tab navigation, mouse input, and other views are explicitly assigned to T-156/T-158 and later tasks. F1 is limited to connecting this task's graph selection and task details, not requiring those other views here.
- Alphabetical seat order differs from the fake config's lane order, but the contract requires one column per lane and this board API receives store data; not raised independently.
- Rework marks count attempts; start_attempt still increments attempts for subsequent work in this candidate, so no separate rework-count finding.
- Nine unchanged tests exceeded 10 s: fixtures_contract::the_fixtures_build_replace_and_pass_their_gates (11.1); merge tests a_failed_merge_is_retried_from_the_plan_and_its_item_closes (10.2), a_merge_waits_for_the_task_its_commits_carry (15.1), a_merge_waits_for_an_earlier_attempt_of_the_task_it_carries (15.8), a_push_race_is_retried_and_the_merge_lands (11.8), waiting_merges_take_turns_and_a_chain_lands_in_order (12.6); multi_repo_contract::a_lane_in_a_child_repo_runs_from_onboard_to_merge_in_that_repo (10.6); parallel_contract::fresh_file_sets_get_fresh_ids_and_ring_through_the_engine (11.6), periodic_scan_and_new_hand_in_trigger_at_their_required_cadence (12.3). Per ops/REVIEW.md these unchanged tests are set aside. The suite remains below the 300 s red line.

## Questions for the captain

None. T-156 may supply the common interaction frame, but contract v1's graph interaction must be connected before this behavior is complete.

## Verdict

Fail: F1, F2, and F3 are blocking. Mechanical build/test/merge checks pass.
