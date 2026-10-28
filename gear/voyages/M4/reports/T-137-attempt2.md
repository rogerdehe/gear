# T-137 — the crate `gear-usage`, attempt 2

Scope: T-137 attempt 2 — the review of attempt 1: F1, F2, F3 and F4. The
captain's correction of design §4.3 (the Codex details) is the text this
attempt implements.

Branch: `deck/t137` (the same branch as attempt 1, per the conventions; code @
the hand-in commit; `origin/dev` merged before verification)

## The findings, one by one

**F1 — the first total of a session was lost.** `codex_rollout` dropped the
first count of a file, so a fresh session with one count reported nothing.
The first valid total now counts **against zero** (`CodexTokens::zero`): a
count with input=1000, cached=200 and output=100 contributes input=800,
cache_read=200 and output=100. The predecessor moves **before** the window is
looked at, so a count before the window is still the predecessor of the one
inside it.

**F2 — the context of a session was lost.** `codex_last_context` returned
`None` for a single count and `0` when the last two totals were the same (a
session looked empty). The context is now:
- `info.last_token_usage` of the last valid line when the line carries it
  (its input — the cached input is in it — plus what was written to the
  cache);
- otherwise the difference to the count before it (its input plus the cache
  write);
- a count that repeats the totals of the one before it adds nothing and
  **keeps** the context of the last turn that had one.

**F3 — a count with a null `info` was counted as zeros.** `CodexTokens::of`
fell back to the `info` object itself, so a `token_count` whose `info` was
null became an all-zero total and the difference to it was negative. An
`info` that is not an object, or that has no object `total_token_usage`, is
not a count: it changes neither the totals nor the predecessor nor the
context.

**F4 — the time of an OpenCode message was read from the wrong place.** The
reader needed a SQL column (`time_completed` and the like) and never read
`data.time.completed`, the timestamp of §4.3 — a table with only
`(id, sessionID, data)` was not read at all. The reader now selects the JSON
of the messages and takes their time from `data.time.completed`; the table
needs no column of its own. `last_context` takes the messages in the order
they were **completed**, not in the order of the rows.

## Tests

The reviewer's fixtures are in, and the expectations that counted on the
first total being dropped were changed:

- `the_first_total_counts_against_zero` — one count: input 800, cache_read
  200, output 100, and a context that is there;
- `a_count_before_the_window_still_moves_the_predecessor` — the count before
  the window is not counted, but it is the predecessor of the one inside it;
- `a_count_without_info_is_not_a_count` — valid, null, valid: no negative
  record, and the context of the last turn survives;
- `a_repeated_total_keeps_the_context_of_the_last_turn` — the context stays
  500 instead of becoming zero;
- `the_context_is_the_usage_of_the_last_request_when_the_count_carries_it` —
  `info.last_token_usage` decides;
- the OpenCode fixture is a **JSON-only table** (`session_message(id,
  sessionID, data)`) with `data.time.completed`, and the rows are inserted
  out of their completion order;
- `a_total_is_never_counted_twice` and `the_window_cuts_the_records_at_its_edges`
  count the first total now.

## Verification

- `cargo test -p gear-usage` → **24 passed, 0 failed**.
- `cargo build --workspace --all-targets` → clean, **no warning**.
- `scripts/test.sh` → **993 tests run, 993 passed, 0 skipped**.
- The tests over 10 s that the suite warns about are the machine (the load
  average was 63–81 while the suite ran): the git-heavy engine tests, which
  this branch does not touch.

## Open questions

None.

## Out of scope

- The quota readers (§9.1): T-138, which carries them on its own branch.
- The prices of §4.4 and the callers of the crate: the other M4 tasks.
