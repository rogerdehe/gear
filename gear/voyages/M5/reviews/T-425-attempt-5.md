# Review of T-425

Status: running
Verdict: pass
Reviewed: review/t425 @ be577496   Reviewer: gear:review-9   Round: 5 (attempt 5)
Contract: v1

## Mechanical acceptance
- build: `cargo check -p gear-engine --all-targets` → clean, zero warnings
- tests (whole crate, per the rework): `make check CRATE=gear-engine` → 822 run / 822 passed / 0 failed / 0 skipped, exit 0
- tests (contract acceptance): `make check CRATE=gear-engine TESTS='test(/review::/)'` → 82 passed / 0 failed, 740 skipped (one run showed a transient "1 leaky" flag that did not reproduce on two reruns, both clean)
- allowed files: ok — `origin/dev...be577496` changes only `crates/gear-engine/src/review.rs` (inherited from attempts 1–4, the contract's stated file), `crates/gear-engine/tests/it/dead_temp_slot_contract.rs` (the rework's rewrite), and `gear/voyages/M5/reports/T-425.md`
- merges into dev: clean — `git merge-tree --write-tree origin/dev be577496` exits 0; `git diff --check origin/dev...be577496` clean

## Spec compliance
- ok — the rework asked only to rewrite the integration test to the per-seat rule with no source change; the attempt-5 commit `780a1ef1` touches only `dead_temp_slot_contract.rs`. The rewritten test drives the full contract sequence: two failures on `review-1` neither stick T-1 nor retire it; the third retires `review-1` with the worktree path and git error in the reason and a once-only "remove that worktree" Info notice, pending cleared; the same hand-in goes to `review-2` in the same tick; `review-2`'s failure is what writes `engine.review_checkout_stuck`, with one captain System/Info notice and nothing in `actionable_for`; a later tick hands T-2 to the freed seat and starts no further `review/t1` checkout. The whole crate passes.

## Findings
- F1 [P2, logged] `git reset --hard` runs before `stash_review_tree`, so a reused seat's tracked-but-uncommitted changes are discarded rather than stashed and reported — `crates/gear-engine/src/review.rs:941` — untracked evidence still survives and is stashed. This is the exact recovery contract (3) mandates and is outside normal use (a warm seat should be clean), so it does not block. Carried from attempts 3/4.
- F2 [P3, logged] the rewritten test weakens an old quota assertion from `== 1` to `<= 1` — `crates/gear-engine/tests/it/dead_temp_slot_contract.rs:408` — the "one reviewer, one slot, no leak" intent is still covered by the surrounding assertions, but the exact count is no longer pinned. Cosmetic; does not block.

## Set aside
- The attempt's `engine.review_checkout_seats.<task>.<attempt>` list is written on a failed checkout and not cleared if the same seat later succeeds. Considered, not flagged: an attempt that has checked out successfully stays with its review seat (the engine does not re-hand it), so a stale entry cannot mark it stuck in normal use, and a new attempt carries its own key.
- The writer notes `tell_captain`'s existing one-message-per-task-per-attempt rule can suppress the stuck notice when a queue notice came first; that is unchanged existing behaviour outside the rework, and the required case lands where the stuck notice is first.
- Retiring a warm seat leaves its agent/pane for the captain's cleanup; the contract requests retirement plus a worktree-removal notice, and pane lifecycle is outside this task's no-other-design scope.
- A transient nextest "1 leaky" flag appeared on the first `review::` filter run and did not reproduce on the two following runs; not attributed to this candidate.

## Questions for the captain
- none