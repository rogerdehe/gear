# T-218 — attempt 2: the retry that fails while it is sent

Task: T-218, attempt 2, contract v1. Lane: keel. Branch: `keel/t218`.
Rework: F1 of the review of attempt 1 (ops/reviews/T-218.md).

## What the review asked

An automatic retry Deliver op that fails during the execution (`Failed`, e.g.
`agent-gone`) must spend its pending marker and open the one stall item, the
same as a retry that times out; the review's reproduction (observation and
execution split around an agent restart) becomes a test. Nothing else.

## The hole

`confirm_pending_with` settles Submitted/Unknown ops, so a retry that failed
while it was sent (`Failed` with `agent-gone`) never reached it: its `pending`
marker stayed. The outstanding-failure sweep suppressed every seat whose retry
was pending, and `lane_nudge` refuses a second redelivery. The attempt stayed
`Delivered` and no captain item ever opened — one agent restart during the
automatic retry left the work stuck silently.

## What I changed

### `crates/gear-engine/src/lane_nudge.rs`

New `retry_failed(tx, seat, task, attempt)`: true when the one automatic
retry of that delivery (the op whose payload carries `redeliver`) is already
`Failed`.

### `crates/gear-engine/src/lib.rs` (the outstanding-failure sweep, step 10)

Before the sweep suppresses a pending retry, it looks at that retry op. When
the retry already failed, the marker is cleared (the one retry is spent) and
the sweep returns an actionable outstanding failure, so the existing
flagged-seat path opens the item — even when there is no prior `stall-active`
item. A retry that is `allowed` or genuinely in flight (`pending`) is still
suppressed, and the independent flags (a missing agent, an escalation) are
untouched.

### `crates/gear-engine/tests/it/lane_nudge_contract.rs`

- `a_retry_that_fails_while_it_is_sent_opens_the_stall_item` (new): the
  review's reproduction. The fixture of `the_first_failure_waits_for_the_retry`
  up to its first tick at T+10; then the observation and the planning of the
  retry are split from the execution, the agent of the pane exits at that
  boundary (`h.add_shell`) and a replacement is there before the next
  observation. The test keeps the assertion that exactly one redelivery was
  created, checks that it is `Failed` with `agent-gone`, and then ticks at
  T+140, T+160 and T+1000: exactly one stall item, the marker spent (the
  cleared empty value), and no second item.
- `failed_packet_world` (new helper): the world of the retry tests.

Without the fix the new test fails (`left: 0, right: 1` — no captain item);
with the fix it passes.

## Results

- `cargo build --workspace --all-targets`: exit 0, zero warnings.
- `scripts/test.sh`: exit 0 — 1400 tests run, 1400 passed, 0 failed, 0
  skipped. Suite 94 s (under the 300 s red line).
- The twelve `lane_nudge_contract` tests pass, including the new regression.
- No new clippy warning in the changed files (the two in the test file are the
  pre-existing ones of the unchanged tests).
- `origin/dev` (accc7460, "M4: T-218 round 1 fail, rework F1") is merged into
  the branch.

## Out of scope

- The first delivery of an attempt that fails while it is sent (not the
  retry) is not touched: the review's finding is about the retry marker, and
  the missing-agent path covers a pane without an agent.
- The working-agent-without-a-check-in case stays as the review set it aside:
  adding a deadline on the `allowed` marker would change the retry policy.
- The uncommitted `crates/gear-app/src/queue.rs` clippy cleanup of the earlier
  T-221 turn is still parked in the stash
  `t218: leftover clippy cleanup from t221`; it is not mine.
