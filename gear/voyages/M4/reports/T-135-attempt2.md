# T-135 — schema v6 and the M4 store API, attempt 2

Scope: T-135 attempt 2 — the review of attempt 1: F1, F2, F3 and F4. F5 is
not this task; the four slow tests are left alone, as the rework note says.

Branch: `deck/t135` (the same branch as attempt 1, per the conventions; code @
the hand-in commit; `origin/dev` merged before verification, the gate green on
the merged result)

## The findings, one by one

**F1 — the build had a warning.** The test was named
`a_friction_report_is_F_1_then_F_2_and_can_be_closed`; Rust wants
`non_snake_case`. It is now `a_friction_report_is_f_1_then_f_2_and_can_be_closed`.
`cargo build --workspace --all-targets` is clean, with no warning.

**F2 — a quota pause was dropped while a declared wait was open.**
`open_pause` looked for an open pause of the *seat*, not of the *kind*, so a
quota park that began while a declared wait was open was silently lost (and
`close_pause` for it then said `NoTarget`). The lookup is now
`AND kind=?3`: the stretches of one kind do not overlap, and a pause of
another kind — a quota park and a declared wait are independent — neither
stops the other nor is dropped by it.

**F3 — `NewFriction` and `NewDecision` required `created`.** §13.1 defines
both inputs as "the columns of §1 without id/seq/created", so a caller that
writes the specified fields did not compile. Both types now carry exactly the
specified fields; `insert_friction` and `insert_decision_if_new` take the time
themselves (`strftime('%s', 'now')`), and the row types keep it. The tests
build both inputs without `created` and read the stored time back.

**F4 — the same-second pause was moved into the future.** The old code
invented a start second to dodge the primary key, so a pause could end before
it began. The key of `seat_pauses` is now **(project, seat, kind, start_at)**
(the captain's decision in the rework note): the kind is in the key, so the
two kinds of a seat may be there at once. The loop that shifted the start is
gone — the time the caller gives is the time that is stored. A pause of the
same kind in the same second reopens that row (with its reason), and
`close_pause` ends a pause at `max(at, start_at)`, never before it began.

The migration carries the change: a store that a first attempt of this task
migrated has the older key (without the kind), and `step_5_to_6` rebuilds the
table with the key of §5.4 and its rows.

## Tests

`crates/gear-store/tests/it/v6_contract.rs` (53 tests in the crate, all pass):
- `a_quota_pause_and_a_declared_wait_of_one_seat_overlap` — the regression of
  F2: both stretches are there, each is closed on its own, and both durations
  survive (an hour of quota wait, 100 s of wait);
- `a_pause_of_the_same_kind_in_the_same_second_reopens_that_row` — the
  regression of F4: one row, reopened, and a pause that is closed before it
  began ends at its own start;
- `a_pause_table_of_the_older_key_is_rebuilt_by_the_migration` — a store with
  the older key is migrated: the row survives and a wait may be open beside
  the quota park;
- the friction and the decision tests build their inputs without `created`
  and check the stored time (F3);
- the friction test has the name of F1.

## Verification

- `cargo build --workspace --all-targets` → clean, **no warning** (F1).
- `cargo test -p gear-store` → 53 passed, 0 failed.
- `scripts/test.sh` → **969 tests run, 969 passed, 0 skipped**, and **no test
  over 10 s** (T-139's and T-136's tests came in with the merges of
  `origin/dev`).
- The tests over 10 s are the machine, not the change (F5): the load average
  was 33–41 while the suite ran (four lanes work at once), and the same tests
  vary from run to run — the final run had **no** test over 10 s, the runs
  before it had one to three of the git-heavy `merge::tests::*` at 10.1–12.3 s.
  Alone, they take 3.7–5.2 s (`a_merge_waits_for_the_task_its_commits_carry`
  4.70 s, `a_merge_waits_for_an_earlier_attempt_of_the_task_it_carries` 5.23 s,
  `waiting_merges_take_turns_and_a_chain_lands_in_order` 4.83 s,
  `fixtures_contract::the_fixtures_build_replace_and_pass_their_gates` 3.70 s).
  Per the rework note, those four tests are left alone.

## Open questions

None.

## Out of scope

- F5: the four slow tests (they are the machine's load, and the rework note
  leaves them to another task).
- The writers of the new columns (the engine, the plan): the other M4 tasks.
