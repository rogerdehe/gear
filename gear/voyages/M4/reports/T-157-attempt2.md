# T-157 — the board's view 2 (Graph), attempt 2

Scope: T-157 attempt 2 — the review of attempt 1: F2 and F3. F1 (the keys and
the Enter panel) is left to T-156, as the rework note says: T-156's contract
carries the frame, the tabs and the keys now.

Branch: `deck/t157` (the same branch as attempt 1, per the conventions; code @
the hand-in commit; `origin/dev` merged before verification)

## F2 — the work that waits for a verdict was regrouped

The order of the graph had one group for the tasks in review and another for
the tasks that are being merged, so a task that is being merged and a task
that waits for its verdict stood in the wrong order (the prototype keeps them
in one group, in the order of the plan).

`order()` now appends the queued tasks, the working ones, **one group of the
work that waits for a verdict** (review *or* merging, in the order of the
plan) and then the merged ones with the newest first — the order of the
prototype.

The fixture now seeds T-141 as accepted (so it is being merged, as in the
prototype's data) and the test says that T-141 comes before T-142.

## F3 — a stored review round lost its verdict

When the rounds of the store were there, the panel printed the role, the
attempt, the CLI, the time and the tokens — and nothing about how the review
ended. A review that came back (`→ fail: rework`) looked like a review that
passed, and a review that was still running looked like both.

A review round now carries its verdict: `→ pass`, `→ fail: rework`, or
`(running)` while there is no verdict yet (read from the review verdicts of
the store).

## Tests

`crates/gear-board/tests/it/graph_contract.rs`:

- the fixture seeds T-141 as accepted, and the order of the rows says that
  the work that waits for a verdict is one group (T-141 before T-142);
- the row of a task in review (T-142) and the row of a task that is being
  merged (T-141) both draw the line of the review, with the info column
  saying `in review` and `merging`;
- `a_stored_review_round_shows_how_the_review_ended` — the three cases of a
  stored review round: a verdict that passed (`→ pass`), one that came back
  (`→ fail: rework`) and none yet (`(running)`).

## Verification

- `cargo test -p gear-board` → **35 passed, 0 failed** (the board's 28 and the
  7 of this task).
- `cargo build --workspace --all-targets` → clean, **no warning**.
- `scripts/test.sh` → **1027 tests run, 1027 passed, 0 skipped**.
- The tests over 10 s that the suite warns about are the machine (the load
  average was 46–62 while the suite ran): the git-heavy engine tests, which
  this branch does not touch.

## Open questions

None.

## Out of scope

- The keys, the selection and the panel that `Enter` opens: T-156's frame
  (the rework note moved them there).
- Views 3 to 5 (Workload, Decisions, Time & tokens): T-158 and the tasks
  after it.
