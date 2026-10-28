# T-143 — Warm reviewer seats (design §3.6), attempt 2

Scope: T-143 attempt 2 — the review of attempt 1 (F1) and the captain's four
decisions on the points the contract left unclear (M-2603).

Branch: hull/t143 (the same branch as attempt 1, per the conventions; code @
the hand-in commit)

## F1 — the verdict that ends a job is the verdict of that job

`handed_a_verdict` accepted **any** attempt of the task that the seat had
judged. So the verdict of attempt 1 ended the review of attempt 2 that the
same seat was doing: the seat went to `Standby`, lost its job binding and —
with W = 0 — lost its pane, while attempt 2 had no verdict at all. A rework
that the same seat reviews again could not finish.

The job of a reviewer is the attempt **its own packet named** — the seat's
last `Deliver` op — and the verdict that ends it must be that attempt's.
A seat with no packet (a legacy seat, a seat of an older engine) has no job
to name: the latest attempt of the task is its job then. The verdict is still
the one **this seat** wrote.

The scan of every attempt is gone; the job's attempt is read from the
delivery op, and the op's task is checked against the seat's own task (a
packet of another task is not this seat's job).

Two regression tests in `crates/gear-engine/src/release.rs`:

- `an_earlier_verdict_does_not_end_the_review_that_runs_now` — attempt 1 is
  judged, the seat reviews attempt 2: without the verdict of attempt 2 the
  seat stays `Active`, bound to its task, its pane open; with it, the seat
  goes to `Standby` and its job binding ends.
- `the_verdict_of_the_job_ends_it_even_when_a_newer_attempt_exists` — the
  seat judged attempt 1 and the writer has already started attempt 2: the
  verdict of attempt 1 is the verdict of its job, so the seat waits for the
  next review.

## M-2603 (1) — the temps quota counts `Releasing` too

The slots are held by the seats with a live agent or one that is starting:
`Active`, `Standby`, `Starting` **and `Releasing`** (a reviewer on its way
out still has an agent leaving). A `Released` seat holds no slot, and a
parked (`Unknown`) one holds its slot only while **Herdr shows an agent in
its pane**. The check asks Herdr directly now (`shows_an_agent`); a pane
that cannot be read is not an absence, so the seat keeps its slot.

## M-2603 (2) — the sweep leaves the reviewer that just got its verdict

The release sweep touches only the seats that were **already** `Standby`
before this tick (`state_since < now`). A reviewer that got its verdict in
this very tick stays `Standby` with its pane open until the next tick, and
the next tick releases it when it is the idle longest. The count of the
standby reviewers (against W) still covers all of them — the seat that just
arrived is one of them, and it holds its slot.

## M-2603 (3) — the cancelled start voids the ops of the gone job

"Its start op is voided" means: the `Intended` and the `Submitted` ops of
that seat **for the gone job** go to their voided state. An op in flight
(`Executing`, `Unknown`) ends on its own, and an op of another job is left
alone. With none of them, the pane is closed and the seat is `Released` —
as before.

## M-2603 (4) — the model knows the transitions

`crates/gear-model/src/state.rs` learns the transitions of §3.6 that a
reviewer seat makes:

- `Active → Standby` — the verdict ends the job, the seat waits;
- `Standby → Releasing` — the idle reviewer the rule (or the captain's
  `- release <reviewer>`) releases;
- `Starting → Released` — the cancelled start (B03-05).

The engine checks them with `require_transition` instead of writing the seat
state raw. The captain's own `- release <reviewer>` (`gear-app/src/plan.rs`)
states its transition locally; the table knows it now too, and that file was
not touched (it is not in the allowed edits).

## Files

- `crates/gear-model/src/state.rs` — the three transitions of a temp seat.
- `crates/gear-engine/src/review.rs` — `handed_a_verdict` (the job's attempt
  comes from the seat's own packet); `occupies_quota` counts `Releasing`;
  `has_free_slot` asks Herdr about a parked seat (`shows_an_agent`);
  `standby_after_verdict` checks the transition with `require_transition`.
- `crates/gear-engine/src/release.rs` — the sweep touches only the seats that
  were already `Standby`; `cancel_start` voids the `Intended` and `Submitted`
  ops of the gone job; `release_one` and `cancel_start` check their
  transitions with `require_transition`.

## Tests

- `an_earlier_verdict_does_not_end_the_review_that_runs_now`,
  `the_verdict_of_the_job_ends_it_even_when_a_newer_attempt_exists` (F1).
- `a_reviewer_that_just_got_its_verdict_waits_for_the_next_tick` (decision 2).
- `a_standby_reviewer_holds_a_temp_slot` (decision 1: the count, with a
  reviewer on its way out).
- `a_starting_reviewer_whose_job_is_gone_is_cancelled` (decision 3: the
  `Intended` and the `Submitted` op of the gone job are voided).
- The tests of attempt 1 (the standby transition, the W rule, the cancel) are
  unchanged and green.

## Verification

- `scripts/test.sh` → 1103 tests run: 1103 passed, 0 skipped (61 s; the
  machine was busy — the same suite ran in 54 s for the review of attempt 1).
- `cargo build --workspace --all-targets` → clean.
- `git diff --check` → clean.
- The five tests over 10 s the script warns about are the pre-existing ones
  (the merge tests, the fixtures contract, the parallel contract): none of
  them is mine, and every test of this task runs in well under a second.

## Open questions: none.

## Out of scope

- The seat a re-review takes (design §3.3 V1/V2) is T-146's.
- The board's count of the temps in use (`gear-board`) is deck's T-144.
