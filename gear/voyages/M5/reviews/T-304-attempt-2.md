# Review of T-304

Status: done
Verdict: pass
Reviewed: review/t304 @ 1662e65f   Reviewer: Codex   Round: 2
Contract: v2

## Mechanical acceptance
- `make check CRATE=gear-app`: exit 0, 500 passed, 0 skipped, zero compiler warnings; build 30.01 s, tests 74.826 s.
- `make check CRATE=gear-cli`: exit 0, 154 passed, 0 skipped, zero compiler warnings; build 1m47s, tests 33.978 s.
- Allowed files: ok. Four permitted production files, four existing test files and the required task report against fetched origin/dev (90eb1085).
- `git merge-tree --write-tree dev 1662e65f` and the same command against fetched origin/dev: exit 0, clean. `git diff --check origin/dev...1662e65f` passes.
- Candidate code remains untouched. Review commits add only review artifacts, preserving attempt 1's remote review via a clean merge.

## Spec compliance
- Earlier F1 is resolved: apply_queue counts this project's non-Retired Lane seats once before revival and increments the count after every successful revival. Subsequent lanes in the same apply see the occupied slot and stay Retired with the required warning when quota is full.
- The new two-Ready-task regression passes: one start, one retained Retired seat, the exact full-quota warning and at most two running lanes under quota two. The existing full-quota refusal/free-slot revival and single-revival tests also pass.
- Contract v2 remains satisfied: config validation omits the quota check; registration and re-registration check the required helper before writes; CLI --check reads retired Lane names from the matching read-only store path, treats no store as empty, and returns BadFormat with the specified text. All requested acceptance cases pass.

## Findings
None (blocking or logged). F1 from attempt 1 is fixed and verified.

## Set aside
- Local dev is stale and its literal three-dot diff includes inherited integrated work; fetched origin/dev isolates this task.
- Attempt 2 merges integration changes already present in origin/dev. Its task-specific production delta is the cumulative revival count; the added regression directly reproduces prior F1. No unrelated changes were attributed to this task.
- Inspected quota equality, multiple revivals, project/role filtering, retained retired seats and no-store behavior. No new defect found. Other scheduling paths and broader M5 board/release obligations are outside this contract.

## Questions for the captain
None.

## Verdict
Pass. Required mechanical checks pass, earlier F1 is resolved, and no finding violates the M5 DELIVERABLE bar.
