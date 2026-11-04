# Review of T-567 (attempt 2)

Status: done
Verdict: pass
Reviewed: review/t567 @ 66938240   Reviewer: pi-temp (<model>)   Round: 2

## Mechanical acceptance
- build: `cargo build -p gear-board --all-targets` → clean (no warnings)
- tests: `make check CRATE=gear-board` → 272 passed / 0 failed
- allowed files: ok — `crates/gear-board/src/forecast.rs`, `crates/gear-board/src/frame.rs`, `gear/voyages/M6.2/reports/T-567.md` (plus the attempt-1 review file on the branch)
- merges into dev: clean (`git merge-tree` exits 0)

## Rework check (attempt 1 findings)
- F1 (blocking) — resolved. `forecast.rs:143` now records a job in `blocked` only when one of its own deps is absent from `merged` (`!merged.contains_key(dep)`), so a job blocked through another queued job (X waits for Y, Y waits for Z) is no longer named; the line names `Y waits for Z`. Covered by the new test `a_job_blocked_through_another_queued_job_is_not_named` (hull empty, mast `Y→Z`), and the attempt-1 cases (T-570→T-665, T-571→T-665) still pass.
- F3 (logged) — resolved. `slowest_lane_clause` (frame.rs:912) returns nothing when no lane has a forecast, so an all-blocked header no longer prints `(slowest lane: )`; covered by `a_header_with_no_lane_to_name_prints_no_slowest_lane`.
- F2 (logged) — unchanged, as instructed; still non-blocking.

## Spec compliance
- ok: a lane end at or past `NO_DEP` is left out of `expected`; the other lanes' max is used.
- ok: the time line names the blocked task and its dep (first such, `+N more`), folded and unfolded; the forecast of the other lanes is untouched.

## Findings
- F2 [P3, logged] The count suffix is ` (+N more)` (parenthesised, leading space) where the contract's example is `+N more` — `crates/gear-board/src/frame.rs:902` — wording only, not blocking.

## Set aside
- A dependency in `Held` or `MergeFailed` stage is not inserted into `merged`, so it is treated as "no job speaks for" and named; this is pre-existing forecast semantics (such deps already produced `NO_DEP`), and it matches the contract's done-but-unmerged (merge keeps failing) case, so it is not a finding of this delta.
- The attempt-1 report/test-count discrepancy is gone: the tree and the report both say 272.

## Questions for the captain
- none
