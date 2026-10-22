# 02 Database and State Model

Files: `gear/core/db.py`, `gear/core/model.py`. Budget 280–340 lines.
Invariants upheld: single authoritative state; request replay; an old generation cannot overwrite newer state (together with 03); one writer per write resource (resources table).

## Responsibilities

- Open the single SQLite database per execution host (path determined by the registered execution host/session, see 14; WAL, busy_timeout 5000ms).
- Table creation and schema version check (upgrade only, never downgrade; refuse to run on version mismatch).
- Transactions, ID allocation, request replay, error codes.
- All state enums and the legal transition table (`model.py`); other modules must write state through `transition()`.

## Not done

No business judgement; no Herdr calls; no ORM. Neither Herdr queries nor git sampling happen inside a write transaction.

## External interface

```python
def connect(db_path: Path) -> sqlite3.Connection
@contextmanager
def tx(conn) -> Iterator[sqlite3.Connection]                 # BEGIN IMMEDIATE
def next_id(conn, project: str, kind: str) -> str            # T/M/R/V/O
def run_request(conn, request_id: str, caller_key: str, action: str, args: dict,
                fn: Callable[[], dict]) -> dict
    """args_hash = hash(project, caller identity incl. generation, action, normalized args).
    Same id, same hash -> return original result (no re-authorization); same id, different hash -> REQUEST_CONFLICT;
    otherwise run fn in the same transaction and save the result."""
class GearError(Exception): code: str; detail: str

class TaskState(StrEnum):    DRAFT, READY, RUNNING, WAITING, VERIFYING, DONE, FAILED, CANCELLED
class AttemptState(StrEnum): DELIVERED, STARTED, STOPPED, SUBMITTED, ACCEPTED, REJECTED, ABANDONED, UNKNOWN
class SeatRole(StrEnum):     CAPTAIN, LANE, TEMP
class SeatState(StrEnum):    ACTIVE, STANDBY, PARKED, HELD, RETIRING, RETIRED, MISSING, NEEDS_CHECK,
                             UNAVAILABLE, HANDOVER, QUEUED, STARTING, RELEASING, RELEASED, UNKNOWN
class MsgKind(StrEnum):      PROGRESS, QUESTION, BLOCKED, RESULT, ACK, INSTRUCTION, REPLY, SYSTEM
class MsgState(StrEnum):     OPEN, WAITING, RESOLVED, INFO
class OpState(StrEnum):      INTENDED, EXECUTING, SUBMITTED, CONFIRMED, FAILED, UNKNOWN, VOID
def transition(kind: str, old: str, new: str) -> None
```

**request_id**: every write entry point (send, reply, plan apply, admin write commands) goes through `run_request`. The CLI generates the ID before executing and immediately prints it to stderr (`request: R-…`), then commits; the caller may also specify it with `--request-id`. If the process dies after COMMIT, the caller reruns with that ID to get the original result back.

## Table structure

```sql
CREATE TABLE meta      (key TEXT PRIMARY KEY, value TEXT);   -- schema_version, engine_heartbeat, engine_session
CREATE TABLE projects  (id TEXT PRIMARY KEY, herdr_session TEXT, config_toml TEXT, config_version INT,
                        quota_lanes INT, quota_temps INT, quota_includes_captain INT,
                        plan_version INT DEFAULT 0, goal TEXT, created INT);
CREATE TABLE milestones(project TEXT, id TEXT, title TEXT, scope TEXT, state TEXT, PRIMARY KEY (project, id));
CREATE TABLE seats     (id TEXT PRIMARY KEY, project TEXT, role TEXT, name TEXT, profile TEXT,
                        generation INT, state TEXT, state_since INT, state_reason TEXT,
                        park_until INT, park_task TEXT, hold_reason TEXT,
                        task_id TEXT,          -- the task a temp worker is bound to (for life)
                        worktree TEXT);        -- resources.path of the worktree Herdr created for this seat
CREATE TABLE bindings  (seat TEXT, generation INT, session TEXT, pane_id TEXT, kind TEXT,
                        session_ref TEXT, status TEXT,    -- pending / active / void
                        handover_id TEXT, created INT, PRIMARY KEY (seat, generation));
CREATE TABLE resources (path TEXT PRIMARY KEY,           -- normalized worktree path as returned by Herdr (unique across projects)
                        project TEXT,
                        branch TEXT,                     -- branch checked out (lane: home/<lane> or current task branch; temp: task branch)
                        workspace_id TEXT,               -- Herdr workspace id (for worktree.remove); NULL while not open in Herdr
                        holder_seat TEXT, state TEXT,         -- free / held / needs_check
                        last_task TEXT, note TEXT, updated INT);
CREATE TABLE plans     (project TEXT, version INT, draft TEXT, applied INT, by_seat TEXT, by_gen INT,
                        PRIMARY KEY (project, version));
CREATE TABLE tasks     (project TEXT, id TEXT, rank INT, state TEXT, priority TEXT, lane TEXT,
                        milestone TEXT, required INT, contract_version INT, deps TEXT,
                        followup_of TEXT, wait_on TEXT, wait_cond TEXT, rework_note TEXT,
                        updated INT, PRIMARY KEY (project, id));
CREATE TABLE contracts (project TEXT, task TEXT, version INT, goal TEXT, accept TEXT,
                        evidence_files TEXT, evidence_due_min INT, body TEXT, created INT,
                        PRIMARY KEY (project, task, version));
CREATE TABLE attempts  (project TEXT, task TEXT, n INT, seat TEXT, generation INT,
                        contract_version INT, adopted_version INT, pending_update INT,
                        state TEXT, candidate TEXT, report TEXT, safe_point TEXT,
                        started INT, updated INT, PRIMARY KEY (project, task, n));
CREATE TABLE messages  (project TEXT, id TEXT, thread TEXT, task TEXT, attempt INT,
                        sender TEXT, sender_gen INT, recipient TEXT, kind TEXT, urgent INT,
                        body TEXT, reply_to TEXT, needs_ack INT,
                        state TEXT, wait_on TEXT, wait_until INT, result TEXT,
                        created INT, resolved INT, PRIMARY KEY (project, id));
CREATE TABLE views     (project TEXT, seat TEXT, generation INT, ts INT,
                        shown_msgs TEXT,                 -- JSON array of message ids actually displayed
                        shown_ops TEXT,                  -- JSON array of operation ids whose content was displayed
                        shown_task TEXT, shown_attempt INT, shown_contract INT);
CREATE TABLE operations(project TEXT, id TEXT, seat TEXT, generation INT, kind TEXT,
                        task TEXT, attempt INT, ref_msg TEXT, plan_version INT,
                        payload TEXT, state TEXT, result TEXT, check_budget INT,
                        created INT, updated INT, PRIMARY KEY (project, id));
CREATE TABLE verdicts  (project TEXT, task TEXT, attempt INT, contract_version INT, candidate TEXT,
                        passed INT, evidence TEXT, by_seat TEXT, created INT);
CREATE TABLE reviews   (project TEXT, id TEXT, cause TEXT, state TEXT, detail TEXT,
                        decided TEXT, plan_version INT, checkpoint INT, reminders INT,
                        created INT, updated INT, PRIMARY KEY (project, id));
CREATE TABLE samples   (project TEXT, seat TEXT, task TEXT, ts INT, head TEXT, status_digest TEXT,
                        content_digest TEXT, complete INT, evidence TEXT);
CREATE TABLE reviews_of_work (project TEXT, task TEXT, ts INT, base_head TEXT, content_digest TEXT,
                        judgement TEXT, next_check INT, by_seat TEXT);
CREATE TABLE requests  (id TEXT PRIMARY KEY, args_hash TEXT, result TEXT, created INT);
CREATE TABLE audit     (ts INT, project TEXT, actor TEXT, action TEXT, detail TEXT);
```

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
