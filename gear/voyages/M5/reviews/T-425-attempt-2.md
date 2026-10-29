# Review of T-425 attempt 2

Status: done
Verdict: fail
Reviewed: review/t425 @ e4069a84dc47c3e339323a31b7e152400adef4c3
Reviewer: gear:review-11 (Codex)
Contract: v1

## Mechanical acceptance

- Required check: `make check CRATE=gear-engine TESTS='test(/review::/)'` exits 0 on the exact candidate: 76 passed, 0 failed, 724 skipped; test time 43.559s. Compilation completed in 1m 15s without warnings. The new prior-F1 regression passed.
- Scope: `origin/dev...e4069a84` changes only review.rs and the task report. Local dev is stale; its three-dot diff includes inherited integration changes.
- Both `git merge-tree --write-tree dev e4069a84` and the corresponding origin/dev command exit 0 without conflicts. Integration ref checked: 97e87a091ec518423f7f2530f8906101a12a58cd. `git diff --check origin/dev...e4069a84` is clean.
- Review branch preserves the previous review artifact; candidate source is unchanged.

## Spec compliance

Prior F1 is resolved: the stuck attempt now releases an onboarding seat or clears the warm seat's reservation, and the added regression exercises the warm case. Per-seat failure accounting, third-failure retirement, the once-only removal notice, and two-distinct-seat stuck detection follow the contract. The merge cleanup still has an early return before the required hard reset.

## Findings

F2 [P1, blocking] A failed merge abort can prevent the required reset and retire a recoverable warm worktree — `crates/gear-engine/src/review.rs:920`. `merge --abort` errors are ignored, but the fallible evidence stash runs before `reset --hard`. When an interrupted conflicted merge includes a cleanly merged file subsequently edited in the worktree, abort fails with `Entry 'b' not uptodate. Cannot merge.` The stash then fails with `a: needs merge` / `could not write index`; its `?` returns before reset. The same recoverable worktree fails each tick and is retired on the third failure. This breaks contract (3), which requires both cleanup commands to run with errors ignored so a half-done merge does not fail checkout, and the DELIVERABLE bar for normal use.

Verified reproduction (real Git, under this worktree's target/tmp): initialize main with files a and b containing `base`; branch other and commit both files as `other`; return to main and commit a as `main`; merge other (a conflicts, b merges cleanly), then edit b again without staging. Execute the candidate's sequence: `git merge --abort` exits 128 (`Entry 'b' not uptodate`), `git status --porcelain` reports `UU a` and `MM b`, and `git stash push --include-untracked -m probe` exits 1 (`could not write index`). The candidate returns at this stash failure. Running the next intended command manually, `git reset --hard`, exits 0 and leaves a clean worktree. No lock corruption or additional external fault is needed: this is an unfinished merge with ordinary edits.

Smallest fix: in prepare_review_tree, ensure reset runs after the ignored abort even when the initial stash fails, and continue preparation if the reset recovered the tree. For example, retain the initial stash result, always attempt reset, then retry stash after reset if the first stash failed (about 3–5 lines). Alternatively place the ignored reset immediately after the ignored abort before the fallible stash, as the contract's cleanup permits. Add a regression using the merge state above to verify checkout succeeds instead of counting a seat failure.

## Set aside

- Retired warm seats' live panes remain for the captain's cleanup; the contract explicitly requests retirement and a worktree-removal notice, without a pane lifecycle redesign.
- Historical task-scoped counters and already-stuck attempts are not migrated; historical recovery is outside the contract.
- Local dev's inherited integration changes and workspace gate tests are outside this task; the merge gate owns make gate.

## Questions for the captain

None.

## Verdict

Fail: F2 blocks under contract (3) and the normal-use DELIVERABLE bar. Prior F1 is resolved. The required candidate check passed. The additional real-Git reproduction above demonstrates the uncovered merge state; its scratch directory was removed.
