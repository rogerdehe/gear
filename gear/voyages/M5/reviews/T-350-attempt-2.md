# Review of T-350

Status: done
Verdict: pass
Reviewed: review/t350 @ 11a107dd
Reviewer: Codex (gear:review-11) · Round: 2 · Contract: v1

## Mechanical acceptance

- Build/tests: independently ran only `make check CRATE=gear-board` on candidate code. Exit 0; zero warnings; compilation 5m 07s; 180 passed, 0 failed, 0 skipped in 189.776s. This includes cancelled_tasks_are_not_counted_in_the_totals, both voyage_select_contract tests, both voyage_scope_contract tests and all golden-frame tests.
- Allowed files: ok. The requested `git diff dev...11a107dd` and fetched `origin/dev...11a107dd` contain production changes only in crates/gear-board/src/lib.rs, three crate test files, and the required task report. voyage.rs already provides reports_including_finished; gear-app home code and tests are unchanged.
- Merge: `git merge-tree --write-tree dev 11a107dd` and `git merge-tree --write-tree origin/dev 11a107dd` both exit 0, without conflicts. Both produce tree 32dd446761671aed214a73f9b8a428a72cfb2d8f. `git diff --check` passes against both integration refs.
- Preserved the remote attempt-1 review commits by merging origin/review/t350; its only added file is the prior review. `git diff --exit-code 11a107dd HEAD -- crates/gear-board` passes. Candidate source remains untouched.

## Spec compliance

The attempt-1 review passed with no findings. Reviewed the attempt-2 delta and its integration with T-415: after merging origin/dev, the sole production rework removes the Cancelled filter from named-voyage task selection. T-415's totals guard remains in place. Cancelled tasks therefore retain lane-queue membership without contributing to task totals; graph filtering remains unchanged. The updated shared M1/M2 fixture asserts exactly that behavior for the landed M1, alongside the existing voyage selection, landing time, retired lane and decision checks.

This follows the explicit attempt-2 rework instruction, which supersedes the original contract's cancelled-lane exclusion. The public API, default board voyage scope, home-page delegation, voyage metadata and landed-lane projection are preserved.

## Findings

None. No blocking or logged findings in the rework delta.

## Set aside

- The original sentence excluding Cancelled from lane tasks conflicts with the explicit rework packet requiring them to stay. The latter governs this attempt; restoring lane membership is intentional.
- Cancelled tasks remain excluded from the graph-based Tasks frame by existing graph::tasks behavior. This pre-existing limitation was already logged in T-415 and is outside this rework; the updated T-350 assertion accurately claims lane-queue membership.
- Current provider, inbox and seat operational data, CLI controls and freezing historical duration calculations were set aside in the accepted first review. They are unchanged by this delta.
- No new tests were added to candidate code; the required complete crate run verifies the T-415 regression and the revised T-350 fixture together.

## Questions for the captain

None.

## Verdict

Pass. Mechanical acceptance succeeds; no finding breaks the rework contract, M5 done sentence or DELIVERABLE bar.

Friction: the required check took over eight minutes on the busy shared machine at one build/test job, and the remote review branch required preserving the attempt-1 review commits before the push. The review exceeded the ten-minute packet budget.
