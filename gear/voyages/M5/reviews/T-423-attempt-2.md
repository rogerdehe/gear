# Review of T-423

Status: done
Verdict: pass
Reviewed: review/t423 @ e724d495018a9c67c13d1eee8cb29a072f7fac70   Reviewer: gear:review-9 (Codex)   Round: 2
Contract: v1

## Mechanical acceptance

- Build/tests: `make check CRATE=gear-engine TESTS='test(/session::/)'` exits 0; 35 passed, 0 failed, 770 skipped. Build 1m 30s; tests 0.483s. Zero compiler warnings.
- Allowed files: implementation changes only crates/gear-engine/src/session.rs. Fetched origin/dev at 97e87a091ec518423f7f2530f8906101a12a58cd; its three-dot comparison adds only that file and the task report. Local dev is stale, so its packet diff also includes previously integrated work.
- Merge: `git merge-tree --write-tree dev e724d495` and `git merge-tree --write-tree origin/dev e724d495` both exit 0 without conflicts. Candidate whitespace check passes.
- Additional verification: standalone Rust probe against the freshly compiled candidate library independently repeated the original F1 scenario through public prepare_lane and prepare_review, with a live replacement agent and an external live holder, plus controls without that holder. All four assertions passed. Scratch source and binary were removed.
- Candidate code untouched; review commits add only this review. Only the required make check was run; make gate belongs to the merge gate.

## Spec compliance

Compliant. Re-reviewed the delta and the earlier F1 finding. The shared without_a_held_resume helper preserves the choose-level rule and also guards existing-round S1/recorded-action selection in both lane and review preparation, within session.rs. Both required facts builders retain their own-pane exclusion.

F1 is resolved. With another pane holding the round session, the original live-agent reproduction now persists action fresh with target null and issues no pane commands. Without the external holder, both lane and review preparation still persist resume targeting the round session and initiate the normal exit sequence. The two new existing-round regression tests also pass, as do the original acceptance cases and existing session tests.

## Findings

None. No new blocking or logged finding in the rework delta.

## Set aside

- The report retains attempt-1 history about the bypass; its explicit attempt-2 section documents the correction. This is historical reporting, not an unresolved implementation gap.
- Fresh execution semantics remain as explicitly prescribed by the contract; runner and pane lifecycle changes are outside this task.
- Observation failure and another agent acquiring a session after the snapshot remain separate failure/race scenarios; neither is required for the original stable-holder reproduction, which now passes.
- Already integrated changes inherited from origin/dev are outside this rework and are not extra task implementation.

## Questions for the captain

None.

## Verdict

Pass. F1 is resolved, mechanical acceptance passes, and no remaining finding blocks the M5 DELIVERABLE bar.
