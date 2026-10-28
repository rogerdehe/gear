# Review of T-321

Status: done
Verdict: pass
Reviewed: review/t321 @ f4419793390bfe67bcf0b50e0059c4c9248e544c   Reviewer: Codex   Round: 2
Contract: v2

## Mechanical acceptance
- Build: both required commands completed with zero compiler warnings.
- Tests: `make check CRATE=gear-app` exited 0: 515 passed, 0 failed, 0 skipped (109.062 seconds of test execution).
- Tests: `make check CRATE=gear-cli` exited 0: 156 passed, 0 failed, 0 skipped (24.512 seconds of test execution).
- Allowed files: ok. `git diff origin/dev...f4419793` changes only admin.rs, home.rs and main.rs, app_home.rs and CLI admin_init_contract.rs tests, and the task report.
- Merge: `git merge-tree --write-tree dev f4419793` and `git merge-tree --write-tree origin/dev f4419793` both exited 0 without conflicts. Fetched origin/dev was 4d9382b6f74c1d6475a0c6a0495b47fbaa832043. Both three-dot diffs pass `git diff --check`.

## Spec compliance
- Attempt 2 resolves the reported CLI test merge conflict. The canonical-path test and T-304 quota test both retain their bodies and separate closing braces; both passed independently in this run.
- The task implementation still matches contract v2: the public helper returns `config.path.<project>`; successful real CLI initialization stores the canonical path; the snapshot compares file text with applied config text and emits the exact drift or unreadable warning on its second line. Equal text and absent metadata produce no warning. No schema change.
- The home implementation and its five tests are unchanged from the previously passing candidate. All required home cases passed again: equality, absent path, edit, applied text matching again, and removed file. The real-init test verifies canonical storage with a nested/../ path.
- The --check early return and failed-init propagation remain before the metadata write; successful reinitialization records the supplied path again.

## Findings
None. No blocking or logged findings.

## Set aside
- The literal local-dev three-dot diff includes inherited tasks because local dev is stale. The fetched origin/dev comparison isolates T-321 to its permitted scope. Review of the attempt-2 merge resolution confirms no extra T-321 behavior change.
- The prior review already considered whole-text comparison, captain-only snapshots, symlink canonicalization and the separate metadata transaction. These follow the explicit contract and were not changed in this rework.
- The remote review branch contained attempt 1. It was merged into the review branch to preserve that artifact and permit a normal push; comparison with the candidate confirms only review files changed.
- The full workspace gate belongs to the merge gate; only the two requested crate checks were run.

## Questions for the captain
None.

## Verdict
Pass. The requested merge-conflict rework is complete, mechanical acceptance passes, and no finding breaks the M5 done sentence or DELIVERABLE bar.
