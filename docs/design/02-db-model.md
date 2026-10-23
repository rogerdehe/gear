# 02 Database and State Model

Implementation: [lib.rs](../../crates/gear-store/src/lib.rs), [state.rs](../../crates/gear-model/src/state.rs).
Invariants upheld: single authoritative state; request replay; an old generation cannot overwrite newer state (together with 03); one writer per write resource (resources table).

## Responsibilities

- Open the single SQLite database per execution host (path determined by the registered execution host/session, see 14; WAL, busy_timeout 5000ms).
- Table creation and schema version check (upgrade only, never downgrade; refuse to run on version mismatch).
- Transactions, ID allocation, request replay, error codes.
- All state enums and the legal transition table ([state.rs](../../crates/gear-model/src/state.rs)); transitions are checked with `require_transition`.

## Not done

No business judgement; no Herdr calls; no ORM. Neither Herdr queries nor git sampling happen inside a write transaction.

## External interface

**request_id**: every write entry point (send, reply, plan apply, admin write commands) goes through `run_request`. The CLI generates the ID before executing and immediately prints it to stderr (`request: R-…`), then commits; the caller may also specify it with `--request-id`. If the process dies after COMMIT, the caller reruns with that ID to get the original result back.

## Table structure

See [schema.sql](../../crates/gear-store/src/schema.sql) for the table definitions, including the attempt lifecycle timestamps added by T-38.

**resources**: one row per worktree. The key is the worktree path returned by Herdr `worktree.create` / `worktree.open` (04); GEAR never constructs worktree paths itself. The row records the holder seat, the branch checked out, and the Herdr workspace id. A long-lived lane's row is created once at onboarding and held long-term (its branch changes as task branches are switched inside it); a temp worker's row is created per new task; after the result is merged, `worktree.remove` removes the checkout and the Herdr workspace (the branch is kept) and `workspace_id` becomes NULL; `worktree.open` on rework sets it again (17 §4). `seats.worktree` points to the resources row of the worktree Herdr created for that seat.

**views**: one row per `gear` home/show call, recording what that call actually displayed to a seat: seat, generation, timestamp, the message ids shown (`shown_msgs`), the operation ids whose content was shown (`shown_ops`), and the task/attempt/contract shown, if any. It is the evidence used to confirm deliver/doorbell operations (07, 08, 13). Indexed on (seat, generation, ts).

Counter-type state (cooldown start, continuation count, reminder count, check budget) is always derived from the persistent records in operations / attempts / reviews, so an engine restart does not reset it.

## Legal transitions (summary)

- **Tasks**: `DRAFT→READY`; `READY→RUNNING` (delivery confirmed started); `RUNNING→WAITING|VERIFYING`; `RUNNING→READY` (preemption safe point checked); `WAITING→RUNNING` (resume condition met, continue the original attempt); `WAITING→READY` (only when the old attempt has been explicitly settled/terminated and needs re-dispatch); `VERIFYING→DONE` (acceptance passed); `VERIFYING→READY` (acceptance failed, with rework note); `DONE→READY` (only when plan apply explicitly proposes same-item rework with a reason; historical ACCEPTED and Verdict are kept); non-terminal `→CANCELLED|FAILED` (plan draft only; cancelling a running task only stops new dispatch, does not automatically ABANDON or release resources, the safe point must be checked, and if unknown the resource stays held).
- **Attempts**: `DELIVERED→STARTED|UNKNOWN`; `STARTED⇄STOPPED`; `STARTED|STOPPED→SUBMITTED`; `SUBMITTED→ACCEPTED|REJECTED` (acceptance only); `STARTED|STOPPED→ABANDONED` **only via** a preemption safe-point check or cancellation; `UNKNOWN→STARTED|STOPPED|SUBMITTED` (recovered to the proven state via linked evidence); `UNKNOWN` cannot go directly to ABANDONED or be released because of a wait timeout.
- **Operations**: `INTENDED→EXECUTING→SUBMITTED (Herdr call returned)→CONFIRMED (linked evidence confirms)`; `EXECUTING→FAILED` (definitely not executed) `|UNKNOWN`; `INTENDED→VOID` (pre-execution check found it stale); `UNKNOWN→CONFIRMED` (linked evidence) `|FAILED` (verified not executed). Both `SUBMITTED` and `UNKNOWN` occupy the seat's single delivery position.
- Seat and message transitions: see 03, 07.

## Error codes

`NOT_REGISTERED`, `STALE_GENERATION`, `STALE_VERSION`, `NO_TARGET`, `FORBIDDEN`, `BAD_FORMAT`, `BAD_TRANSITION`, `LOCKED`, `REQUEST_CONFLICT`, `QUOTA`, `UNCONFIRMED` (exit code 3), `ENGINE_DOWN` (informational only, does not block DB writes).

## Test points

Exhaustive transition table; the three request replay cases, and replay does not authorize a new generation; concurrent IDs never collide; lock timeout leaves no partial writes; the same worktree cannot be held by two seats; budget counters do not reset after restart.


### Voyage measurements (T-39)

`voyages.departure_estimate_min` stores the first known total estimate at departure.
It is nullable and migrated additively. Later plan edits preserve it; existing
voyages recover it from the oldest applicable stored plan during a plan apply.
Missing historical estimates remain unknown. The captain home page and board
use the same read-only `gear_board::voyage` measurements and five-line renderer.
Pace samples only completed tasks with both estimates and recorded attempt spans.
Projection is elapsed + remaining estimates × pace, rounded up to whole minutes;
the JSON view includes both minutes from departure and the projected epoch time.
