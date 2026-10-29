# Review of T-420

Status: done
Verdict: pass
Reviewed: review/t420 @ 2dba532b   Reviewer: gear:review-9 (Codex)   Round: 3
Contract: v3

## Mechanical acceptance

- Build/tests: `make check CRATE=gear-engine TESTS='test(/onboard|restart|review::/)'` exits 0: 159 passed, 0 failed, 629 skipped; test time 164.061s. Compilation finished in 3m 01s with zero warnings. Both TMPDIR environment tests, the reviewer cleanup regression, and the existing onboard scratch tests passed.
- Allowed files: fetched `origin/dev...2dba532b` contains only onboard.rs, restart.rs, review.rs (including unit tests) and gear/voyages/M5/reports/T-420.md. The broader requested `dev...2dba532b` includes inherited integration/T-413 changes. T-420's production rework commit d5f534a3 changes review.rs only.
- Merge: `git merge-tree --write-tree dev 2dba532b` and `git merge-tree --write-tree origin/dev 2dba532b` both exit 0 without conflicts. `git diff --check origin/dev...2dba532b` is clean.
- The candidate source is untouched. The review branch adds this review and preserves the prior round's review through a merge of origin/review/t420, allowing a normal fast-forward push.

## Spec compliance

The shared pub(crate) onboard::tmpdir_prefix preserves scratch resolution and shell quoting, and onboarding, reviewer start and restart/model-switch environment construction use it. Attempt 2's F1 is resolved: start_reviewer records its resolved checkout under onboard::worktree_path_key before preparing the tree and submitting the environment. Release reads this same key to resolve the default scratch path. The new regression seeds only reviewer-specific path metadata, starts the reviewer, checks the emitted TMPDIR prefix and newly recorded worktree key, then releases it and verifies the scratch directory disappears only after the seat is free.

## Findings

None. Prior F1 is resolved; no new blocking or logged findings.

## Set aside

- The regression uses an existing reviewer checkout under target/tmp because FakeHerdr's fresh checkout path is synthetic. It still exercises the actual start/release metadata seam without seeding the key whose absence caused F1.
- The helper's quoting and configured-directory behavior remain covered by the existing onboard tests; all three callers share that implementation.
- Captain initialization, legacy seats with missing worktree metadata, failed mkdir permissions, and inherited integration/T-413 changes are outside this attempt's delta.
- No unrelated suite or make gate was run; the merge gate owns make gate. The prescribed filter includes the new reviewer cleanup regression.

## Questions for the captain

None.

## Verdict

Pass: mechanical acceptance passes, prior F1 is resolved, and no new findings block the contract or DELIVERABLE bar.
