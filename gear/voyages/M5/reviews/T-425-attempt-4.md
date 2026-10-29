# Review of T-425

Status: running
Verdict: pass
Reviewed: review/t425 @ a9274e45   Reviewer: gear:review-11 (deepseek-v4-pro-0813)   Round: 4

## Mechanical acceptance
- build: `cargo check -p gear-engine` → clean (zero warnings)
- tests: `make check CRATE=gear-engine TESTS='test(/review::/)'` → 82 passed / 0 failed, 732 skipped
- allowed files: ok — `origin/dev...a9274e45` changes only `crates/gear-engine/src/review.rs` and this task's report
- merges into dev: clean — `git merge-tree --write-tree origin/dev a9274e45` exits 0

## Spec compliance
- ok — attempt 4 is the dev merge the rework asked for. The production merge kept all three tasks' changes: T-425's per-seat failure count, third-failure retirement and once-only notice, end_stuck_seat_assignment and the ignored `merge --abort` + `reset --hard` before the stash; T-427's oldest-hand-in-first sort; T-429's warm-reviewer model/CLI release. The one test-module conflict was resolved by keeping T-429's two tests before T-425's own, and all of them pass.

## Findings
- F1 [P2, logged] unchanged from attempt 3: `git reset --hard` runs before `stash_review_tree`, so a reused seat's tracked-but-uncommitted changes are discarded where the old order stashed and reported them — `crates/gear-engine/src/review.rs:943` — untracked evidence still survives and is stashed; this is outside normal use and is the recovery contract (3) mandates, so it does not block.

## Set aside
- Pre-existing slow tests (e.g. `direction_checks_follow_pushes_and_report_results` ~11s in this run, `a_report_alone_starts_no_direction_check_but_a_report_and_a_source_file_do`) were not touched by this candidate.
- The writer's report says 0 skipped while my run reports 732 skipped; this is a report wording difference, not a failed fact — the required filter passes.

## Questions for the captain
- none