# Review of T-273

Status: done
Verdict: pass
Reviewed: review/t273 @ 89ed2703   Reviewer: gear-review-11 (Codex)   Round: 2
Contract: v2

## Mechanical acceptance
- Build: independent `make check CRATE=gear-engine` compiled successfully with zero warnings (47.16s build).
- Tests: the same command exited 0; 653 passed, 0 failed, 0 skipped, 321.785s test runtime. All five T-273 behavior tests passed, including the new no-reviewers regression.
- Allowed files: ok. Both `git diff dev...89ed2703` and the fetched `origin/dev...89ed2703` change only `crates/gear-engine/src/review.rs`, its `lib.rs` tick integration, and the required task report.
- Merges into dev: clean. `git merge-tree --write-tree dev 89ed2703` and `git merge-tree --write-tree origin/dev 89ed2703` both exited 0. Fetched origin/dev: 70c24ebb.
- Whitespace: both local-dev and origin/dev three-dot diffs pass `git diff --check`.
- Candidate integrity: `git diff --quiet 89ed2703 -- crates` passes. Review commits contain only this review file.

## Spec compliance
- F1 from attempt 1 is resolved. The no-[temps] branch now gathers eligible hand-ins and runs `start_hand_in_reviews` before notifying the captain. The per-hand-in temps guard runs after mechanical settlement, so settlement needs no reviewer configuration.
- The new `a_mechanical_hand_in_settles_without_reviewers` regression reproduces the earlier failing configuration and passes: Accepted attempt, engine pass evidence, queued merge op, one mechanical Info message, and no manual-settlement request.
- Ordinary no-reviewer hand-ins retain the existing once-per-attempt captain notification; its existing regression passes. With reviewers, non-mechanical and stale-contract hand-ins retain ordinary review, as verified by the passing behavior tests.
- The re-review delta in production code is confined to these two guard changes in review.rs; lib.rs is unchanged from attempt 1. The candidate retains current-contract direction suppression, settlement through the existing app API with the attempt's candidate/adopted contract, exact evidence/message text, and settlement-error collection into the tick report.

## Findings
None. Earlier blocking F1 is resolved; no new blocking or logged findings.

## Set aside
- Previously reviewed unchanged behavior is retained; this round reviews the rework delta and checks the complete mechanical acceptance again.
- Database read failures/corrupt rows and already active reviewers remain outside the normal new-hand-in route considered in attempt 1; this rework introduces no new behavior there.
- The broader voyage board, release, and scheduling requirements are outside this engine change; no unrelated findings.

## Questions for the captain
None.

## Verdict
Pass: F1 is fixed, mechanical acceptance passes, and no finding breaks the M5 done sentence or DELIVERABLE bar.
