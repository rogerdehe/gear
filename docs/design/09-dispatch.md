# 09 Continuation and Assignment Decisions

File: `gear/engine/dispatch.py`. Budget 140–180 lines. All pure functions; side effects go through 08 ops.
Invariants upheld: at most one needs-check delivery per lane; no resend when the outcome is unknown; one controller per lane.

## Input and output

```python
@dataclass
class LaneView:
    seat: Seat
    obs: SeatObs
    attempt: Attempt | None           # current non-terminal attempt
    pending_op: Operation | None      # unconfirmed deliver/doorbell for this seat
    last_deliver_at: int | None
    nudges_since_progress: int        # number of continues since the last verifiable progress
    ready_tasks: list[Task]           # this lane's ready queue (rank order)

class Action(NamedTuple):
    kind: Literal["continue", "assign", "none", "escalate"]
    task: str | None
    reason: str

def decide(v: LaneView, cfg: DispatchCfg) -> Action
```

## Decision order (`decide`)

1. Seat is not ACTIVE/STANDBY/RETIRING (PARKED, HELD, MISSING, NEEDS_CHECK, UNAVAILABLE, HANDOVER…) → `none`.
2. Observation is None (observation failed) → `none`.
3. The seat has any in-flight delivery (deliver/doorbell in INTENDED/EXECUTING/SUBMITTED/UNKNOWN) → `none` (wait for the check, do not resend).
4. Agent not idle (working / blocked / busy on screen) → `none`; blocked is handed to 10 and recorded as needing human attention.
5. Screen confirms ratelimit / quota / auth → `none`; the reason is written to the seat (handled by 10); no nudging.
6. Less than the cooldown since the last delivery (default 90 seconds, validated at startup to be > tick) → `none`.
7. **Three kinds of action**:
   - There is a current attempt in state STARTED/STOPPED (no result submitted) → `continue` (same task, same attempt); if `nudges_since_progress ≥ budget` (default 5) → `escalate` (give the captain one item "T-17 repeated continues with no progress"; stop nudging).
   - Current attempt SUBMITTED (awaiting acceptance) → do not re-implement; only `assign` the next one if 06's "safe conditions for taking the next item" are met, otherwise `none`.
   - No attempt in progress: ready tasks exist → `assign` the first one; none → `none` (10 sets STANDBY).
   - Extra requirements for `continue`: task not in WAITING, not held, no unresolved blocker, not pending verification.
   - Before `assign` (re-read inside the transaction at execution time): task READY, not wont, dependencies satisfied by version, seat state and generation, contract, write resources held by this seat, no in-progress/UNKNOWN attempt.
   - An `assign` to a long-lived lane carries the task branch name `<lane>/<task-id>-<slug>`, which the captain/engine creates from `dev` inside the lane's long-held worktree (17 §3, §4) before delivery; the lane works and commits only on that branch.
   - Temp workers only ever `continue` their own bound task, never `assign`.
   - RETIRING: `assign` is forbidden, but a `continue` related to safe wind-down/handover of the existing attempt is allowed.

## Delivery content (templates, versioned, validated to contain no screen marker)

- continue: `Continue T-17 (contract v4). Task packet: <path>. Report progress/blocked/done with gear send.`
- assign: `New task T-31 (contract v1). Branch: <lane>/T-31-<slug> (in your worktree). Task packet: <path>. Read it before starting; report with gear send.`
- Rules live only in the lane instructions and task packets; they are not broadcast with nudges.

## Determining "verifiable progress" (resets `nudges_since_progress`)

A new commit, worktree content change, evidence file update (sampling result from 12), or a result with a checkable evidence reference. Plain progress text and screen output do not reset the count.

## Captain seat

Also goes through `decide`, but has no task queue: pending items or a due review → handled by the 07 doorbell / 11 review reminder; no continue is sent.

## Test points (including regressions from the old keeper incident)

- Nudge text containing a rate-limit marker → rejected when the template is loaded.
- State done but screen busy → no nudge.
- Cooldown ≤ tick → error at startup.
- No new commit but the agent is still stopping each turn → keep continuing per the cooldown as usual; do not stop permanently because HEAD is unchanged; escalate when over budget.
- While an in-flight delivery exists, never deliver again (even after the cooldown has passed).
- Progress spam does not reset the no-progress count.
