# 08 Engine Loop, Observation, Side Effects

Files: `gear/engine/loop.py`, `observe.py`, `ops.py`. Budget 220–280 lines.
Invariants upheld: the only process that executes Herdr side effects; an unknown outcome is not resent; an observation failure is not treated as "nothing happened"; single instance.

## Startup (`gear run`, run by the plugin pane)

1. Single-instance lock: `<state_dir>/engine.lock` (`fcntl.flock` non-blocking). If it cannot be taken → print "engine already running (pid, heartbeat, pane)" and exit, leaving no second resident process.
2. Open the DB, check the schema version, read all project configs and display the effective values.
3. **Recovery check**: all `EXECUTING` operations become `UNKNOWN` (the last process died mid-execution); `SUBMITTED` deliveries stay pending check; nothing is resent. Check that the execution host/session bound in the database matches the current Herdr; refuse to run on mismatch.
4. Enter the loop.

## Loop tick

```python
def run_forever(conn, cfg):
    while True:
        t0 = now()
        snap = observe.snapshot(project_sessions)     # one agent list per session
        for project in round_robin(projects):          # rotate through projects, each has a time budget this round
            obs = observe.reconcile(conn, project, snap)   # seats <-> Herdr reality
            run_due(conn, project, obs)               # 09 continuation/dispatch, 07 doorbell, 10 staff, 11 review, 12 sampling
        ops.drain_intended(conn, budget=…)            # execute the intent queue (with concurrency limit)
        write_heartbeat(conn); board.refresh(conn)
        sleep(max(0, cfg.tick - (now()-t0)))         # tick default 4 seconds
```

- Low-frequency jobs: output sampling default 20 minutes, review timer check 1 minute, expired park/wait check every round.
- A project times out this round (e.g. Herdr is slow) → remaining work is pushed to the next round, other projects proceed as usual.
- Slow calls do not stall the main loop: long calls such as `agent_start` and `worktree_create` run in a bounded worker thread pool (concurrency ≤ 4); the main loop only registers them and collects results; neither Herdr calls nor git sampling hold the SQLite write lock.

## Observation and reconciliation (`observe.py`)

```python
@dataclass
class SeatObs:
    seat_id: str
    present: bool | None          # None = observation failed
    status: str | None            # idle/working/blocked/done/unknown
    screen_flags: set[str]        # 04 classify_screen: busy/ratelimit/quota/auth (screen read only when needed)
    fingerprint_ok: bool          # whether the occupant matches the binding
    since: int                    # how long this state has lasted

def snapshot(sessions) -> dict[str, HerdrResult]
def reconcile(conn, project, snap) -> dict[str, SeatObs]
```

Reconciliation rules:

| Observation | Action |
|---|---|
| agent list call failed | all of this project's seats `present=None` this round, no judgement made, board marks "observation failed" |
| seat pane gone / Agent exited | lane (ACTIVE/STANDBY/PARKED/HELD/RETIRING) → `MISSING` (handled by 10); **captain → `UNAVAILABLE`** (03, relevant permissions frozen); expected changes in QUEUED, RELEASED, STARTING/RELEASING are checked against the corresponding operation and do not count as missing |
| occupant fingerprint changed | first check whether there is an onboard/release/handover operation for it: an expected start or exit is checked against that operation; only an unexpected change with no corresponding operation revokes the binding and sets `NEEDS_CHECK`, no dispatch, notify |
| status idle/done | read the screen once (tail N lines) to confirm no busy signature, then hand to 09 as "idle" |
| ratelimit/quota/auth matched | confirmed only after `confirm_rounds` consecutive rounds; captain matching quota/auth → 03's UNAVAILABLE |

## Side effects (`ops.py`)

Every side effect goes through three steps, all written to the database:

```python
def intend(conn, project, seat, generation, kind, *, task=None, attempt=None,
           ref_msg=None, plan_version=None, payload=None) -> str   # INTENDED, fixes the values needed for checking at intent time
def execute(conn, herdr: HerdrActor, op_id) -> None   # EXECUTING → SUBMITTED / FAILED / UNKNOWN
def confirm(conn, op_id, evidence: str) -> None       # SUBMITTED/UNKNOWN → CONFIRMED (requires linked evidence)
```

- kind: `onboard` (start and onboard, including resume; for a long-lived lane's first onboarding and for a temp worker's new task this includes `worktree.create`, for a temp worker's rework/resume `worktree.open`, all via Herdr, 04), `deliver` (dispatch/continuation), `doorbell`, `release` (make a temp worker exit; its worktree is kept until the result is merged, then removed with `worktree.remove`), `title`, `notify`. park/hold/retire are pure data targets completed by plan apply in its transaction and do not go through ops.
- Before execution, check current state against the generation, task/attempt, and plan version fixed at intent time; mismatch → `VOID`.
- `SUBMITTED` = the Herdr call returned success, which **does not mean** the Agent has started; `CONFIRMED` requires evidence linked to this operation: deliver/doorbell → a view record of the seat's current generation **actually displayed the content this delivery referred to** (13); ordinary progress, reports from an old round with the same attempt number, and other detail queries do not count; onboard → the expected Agent is observed and the fingerprint is recorded (and, where a worktree was created/opened, its path is recorded in 02 resources); release → the original Agent has exited and the needed records are saved; title/notify → an explicit success receipt is enough. working by itself is not evidence. When it cannot be reliably linked, keep UNKNOWN/needs check; do not relax.
- `UNKNOWN` (timeout, process killed, unparseable receipt): do not resend; keep observing for linked evidence within the check budget; beyond the budget → mark the seat "needs check" and notify.
- Only when Herdr explicitly rejects and it is known not to have executed (e.g. `agent_blocked`) is it marked `FAILED`.

## Test points

Fake Herdr: killed during execution and restarted does not resend; observation failure does not trigger MISSING; RELEASED is not judged missing; an occupant change is not inherited; working does not confirm a delivery; one slow project does not block another; failing to take the single-instance lock exits.
