# Review of T-380

Status: done
Verdict: pass
Reviewed: review/t380 @ 93f7c239   Reviewer: Codex (gear:review-11)   Round: 2
Contract: v3

## Mechanical acceptance

- Build: the required scoped check compiled the test profile in 2m 05s with zero warnings.
- Tests: `make check CRATE=gear-engine TESTS='test(/^review::tests::/) or test(a_lane_in_a_child_repo_runs_from_onboard_to_merge_in_that_repo)'` exited 0: 70 passed, 0 failed, 707 skipped, in 42.268s. This includes the previously failing `a_result_in_starts_one_reviewer` and the new default/upstream packet test.
- Allowed files: ok. The fetched `origin/dev...93f7c239` diff changes review.rs, the existing multi_repo_contract.rs test expectation, skills/gear/references/review.md, and the required task report. Attempt 2 changes only the assertion identified in F1 and the report.
- Merge compatibility: `git merge-tree --write-tree origin/dev 93f7c2393bc67b40bd6dbff0c1576c6c36e0e575` exited 0 against origin/dev at ff6fd65b. `git diff --check origin/dev...93f7c2393bc67b40bd6dbff0c1576c6c36e0e575` passed.
- Reviewer branch preserves the earlier review commits. Its difference from the candidate contains review artifacts only; candidate code remains untouched.

## Spec compliance

The production change passes config.conventions.remote from review_packet_for into review_packet. The diff line fetches that remote and compares its integration ref against the candidate. No other packet text changes. The reference document names the remote three-dot diff after a fetch. Tests verify both default origin and custom upstream, along with child-repository and saved-integration behavior.

Attempt 2 resolves the only earlier finding with the requested one-line assertion update; independent scoped acceptance now passes.

## Findings

None. Earlier F1 is resolved at crates/gear-engine/src/review.rs:4085: a_result_in_starts_one_reviewer expects `git fetch origin && git diff origin/dev...abc1234` and passes.

## Set aside

- Fetch failure handling and unusual remote names: the contract specifies the emitted command exactly and requests no other design. No new issue was identified within normal use.
- The child-repository test took 20.049s; its only candidate change is an expected string. Existing fixture runtime does not block this task.
- The merge gate runs make gate. Only the required scoped make check was run here.

## Questions for the captain

None.

## Verdict

Pass. Mechanical acceptance and contract v3 are satisfied; no blocking or logged findings remain.
