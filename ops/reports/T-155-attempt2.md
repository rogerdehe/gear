# T-155 — why a lane is idle, attempt 2

Scope: T-155 attempt 2 — the review of attempt 1: F1, the work that waits for
its review in the background.

Branch: `deck/t155` (the same branch as attempt 1, per the conventions; code @
the hand-in commit; `origin/dev` merged before verification)

## F1 — the work in review hid the reason of the lane

The next task of the lane was the first task that was not finished, so a task
that was handed in (in review) sat in front of the queue: the lane that had
handed its work in and had nothing left got no reason at all (`None`), and the
task that was queued behind it was never named.

The next task is now the first **queued** one (Ready or Waiting). A task that
was handed in waits for its review in the background — the engine gives the
lane its next task meanwhile — so it does not hide the work that is left:

- a lane whose only task is in review → `NoWork` (`nothing left planned`);
- a lane with a task in review and a queued `T-3` → `Undelivered(T-3)`
  (`next T-3 is ready but not delivered`);
- a lane with a task in review and a queued task that waits for a dependency
  → `DepWait(T-3, T-9)`.

The order of §8.2 is unchanged: the seat's own state, the declared wait
(§5.3, before the working check, because the attempt of a waiting task stays
Started), the working check, and then the queue.

## Tests

The reviewer's two cases are in, plus the successor that waits for a
dependency:

- `a_lane_whose_work_is_in_review_has_nothing_left` — the task in review
  with a Submitted attempt: `NoWork`;
- `the_task_after_the_one_in_review_is_the_next_one` — the task in review and
  `T-3` behind it: `Undelivered(T-3)`;
- `the_task_after_the_one_in_review_may_wait_for_a_dependency` — `T-3` waits
  for `T-9`: `DepWait(T-3, T-9)`.

## Verification

- `cargo test -p gear-board` → **41 passed, 0 failed** (the board's 28 and the
  13 of the idle module).
- `cargo build --workspace --all-targets` → clean, **no warning**.
- `scripts/test.sh` → **1024 tests run, 1024 passed, 0 skipped**.
- The tests over 10 s that the suite warns about are the machine (the load
  average was 54–59 while the suite ran): the git-heavy engine tests, which
  this branch does not touch.

## Open questions

None.

## Out of scope

- The board's rendering of the reason, the callers (`gear queue`, the
  imbalance item) and the queue commands: the other M4 tasks.
