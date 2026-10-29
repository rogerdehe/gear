# Review of T-384

Status: done
Verdict: pass
Reviewed: review/t384 @ cbd4e0b3a71333ea457feebe76502faf5167bbaf
Reviewer: Codex (gear:review-9)
Round: 2
Contract: v1

## Mechanical acceptance

- Build: both prescribed commands completed with zero compiler warnings. No additional cargo checks or merge gate were run.
- Tests: `make check CRATE=gear-store` exited 0: 72 passed, 0 failed, 0 skipped; compilation 9.38 s, tests 2.475 s.
- Tests: `make check CRATE=gear-board` exited 0: 157 passed, 0 failed, 0 skipped; compilation including build-lock wait 21.31 s, tests 5.373 s. All seven quiet-contract tests passed, including the new F1 reproduction, and all golden tests passed.
- Allowed files: T-384 production edits are confined to the two allowed lib.rs files, with tests and registrations in their existing crate locations and the required task report. The requested local `dev...candidate` diff additionally contains T-345 golden tests and its report/review, inherited through the explicit integration merge eb725440. Their originating commits are ec24cdd0 and 4cb12b1f; these paths are already on current remote dev and disappear from `origin/dev...candidate`. They are not new T-384 scope.
- Merges into dev: clean. `git merge-tree --write-tree dev cbd4e0b3a71333ea457feebe76502faf5167bbaf` exited 0. After fetching dev, the same check against origin/dev @ d9fb53436374324039f01e1dbd9a72d524becbe6 also exited 0. `git diff --check dev...cbd4e0b3a71333ea457feebe76502faf5167bbaf` passed.
- Candidate code remained untouched. The review branch preserves the earlier review through a merge of origin/review/t384; the only differences from the candidate are the earlier review artifact and this review artifact.

## Spec compliance

F1 from attempt 1 is resolved. In run_with, the p handler now normalizes shown against the cached project count, and the rebuild branch normalizes it against the newly read count before rotating. The added `a_project_registered_later_does_not_move_the_shown_one` test opens two projects, presses p twice, registers a third project through another Store connection, and proves the last header still names a with three projects and exactly two total builds. This test passed independently in the prescribed board check.

The original acceptance remains satisfied: ten idle refresh periods build once, an external write causes the next refresh to rebuild, keys redraw without rebuilding, expiry causes one rebuild, unchanged agents do not rebuild, changed agents rebuild, and the store test proves data_version changes across external commits. Production defaults remain 60 s for expiry and 10 s for agent observation. The rework changes only project-index normalization and its regression coverage; the cache design and redraw paths are retained.

## Findings

None. Earlier blocking F1 is resolved; no new blocking or logged findings.

## Set aside

- T-345 changes inherited from integration were checked for provenance and test success, rather than reviewed as new T-384 work.
- Projects inserted before the current project in sorted order can affect the numeric selection on a rebuild; the pre-candidate loop also selected by index, so preserving project identity across arbitrary reorderings is outside this rework.
- A concurrent write between the data-version probe and board snapshot can cause an extra rebuild next refresh; this follows the prescribed separate probe and does not conceal the write.
- Minute-old time and engine-status observations, panel refresh behavior, and the public thread-local test hook were considered in the original review. This delta does not change them; the contract expressly allows the hook and prescribes the rebuild triggers.

## Questions for the captain

None.

## Verdict

Pass. Mechanical acceptance succeeds, F1 is fixed and covered by a passing reproduction, and no remaining finding breaks the DELIVERABLE bar or the done sentence.
