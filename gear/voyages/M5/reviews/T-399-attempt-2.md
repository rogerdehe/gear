# Review of T-399

Status: done
Verdict: pass
Reviewed: review/t399 @ 797df492   Reviewer: Codex (gear:review-11)   Round: 2
Contract: v1

## Mechanical acceptance

- Build: independently ran `make check CRATE=gear-engine` at the candidate; compilation completed in 24.12 seconds with zero warnings.
- Tests: the same command exited 0; 729 passed, 0 skipped in 279.775 seconds. All five T-399 regressions passed, including the new refused-close test. The existing `standby_reviewer_releases_before_moving_to_another_repo` test also passed.
- Allowed files: ok. Freshly fetched `origin/dev...797df492` contains only `crates/gear-engine/src/release.rs` (production and tests) and the task report. Extra files in the packet's local `dev...797df492` are already integrated work from other tasks.
- Merge: `git merge-tree --write-tree dev 797df492` and the equivalent against fetched `origin/dev` both succeeded without conflicts. `git diff --check origin/dev...797df492` passed.
- Candidate code untouched. Review branch preserves attempt 1's review commits; new commits contain only the attempt 2 review artifact.

## Spec compliance

F1 from attempt 1 is resolved. In `release.rs:591–606`, the pending reservation clear and transition to Releasing share one `Store::write`; the store commits both or rolls both back. A refused close returns before that transaction. The added regression verifies that a refused close leaves both Standby and the pending task intact, then a successful retry changes both together.

The Standby rule retains the strict age greater than 60 seconds, requires a live pane binding and `agent_get = Ok(None)`, and leaves an existing agent or an observation error alone. The release implementation is shared through `release_seat`, passing no task and enabling reservation clearing only for the dead-Standby path. Existing `release_one` callers retain their reservations for repo/model moves. This is the same targeted release lifecycle requested by the contract; no other production file changes.

## Findings

None. No blocking or logged findings; prior F1 is resolved.

## Set aside

- Exactly 60 seconds remains within the grace period, as required by “more than 60 s.”
- The surplus-Standby policy remains unchanged. It may independently release an unreserved surplus seat; the requested retention cases are exercised with a pending review so that policy cannot mask the result.
- A seat without a live binding remains owned by the existing dead-temp sweep; no observable pane is available for this rule.
- An absent pending key becomes an empty key on this release path. Existing readers treat both as no reservation; this has no behavioral impact.
- The local-dev whitespace check reports an existing trailing blank line in T-347's report. The fetched-integration candidate diff is whitespace-clean; that report is outside T-399.
- Milestone board approval and release obligations belong to their own tasks. Only the required crate check ran; the merge gate owns `make gate`.

## Questions for the captain

None.

## Verdict

Pass. Mechanical acceptance and Contract v1 are satisfied, including the atomic-write rework. No finding breaks the milestone done sentence or DELIVERABLE bar.
