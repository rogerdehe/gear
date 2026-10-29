# Review of T-425

Status: done
Verdict: pass
Reviewed: review/t425 @ fafd11ed   Reviewer: gear:review-11 (deepseek-v4-pro-0813)   Round: 3

## Mechanical acceptance
- build: `cargo check -p gear-engine` → clean (zero warnings)
- tests: `make check CRATE=gear-engine TESTS='test(/review::/)'` → 80 passed / 0 failed, 729 skipped
- allowed files: ok — `origin/dev...fafd11ed` changes only `crates/gear-engine/src/review.rs` and this task's report
- merges into dev: clean — `git merge-tree --write-tree origin/dev fafd11ed` exits 0

## Spec compliance
- ok — prior F1 and F2 are both resolved. Per-seat failure counts, retirement on the third failure (with worktree path and git error in the reason), once-only worktree-removal notice, two-distinct-seat stuck detection, and the ignored `merge --abort` + `reset --hard` now run before the fallible stash.

## Findings
- F1 [P2, logged] `git reset --hard` now runs before `stash_review_tree`, so a reused seat's tracked-but-uncommitted changes are discarded where the old order stashed and reported them — `crates/gear-engine/src/review.rs:941` — untracked evidence (a reviewer's normal output) still survives the reset and is stashed, but an unclean tracking state is silently lost. This is outside normal use and is the exact recovery the contract's (3) mandates, so it does not block.

## Set aside
- Pre-existing slow tests (e.g. `direction_checks_follow_pushes_and_report_results` ~27s, `a_report_alone_starts_no_direction_check_but_a_report_and_a_source_file_do` ~25s) were not touched by this candidate and do not count against it.
- Legacy task-scoped failure counters and already-stuck attempts are not migrated; the contract asks for new accounting, not historical recovery.
- Retired seats keep their live panes for the captain to clean up; pane lifecycle is outside this task's no-other-design scope.

## Questions for the captain
- none