# 03 Identity, Seats, Generations, Manual Captain Replacement

Files: `gear/core/identity.py` (+ the seat/captain subcommands in `cli/admin.py`). Budget 170–210 lines.
Invariants upheld: an old generation cannot overwrite newer state; one continuation controller per lane; no writes when identity is unclear.

## Concepts

- **Seat**: a role position in a project — the captain, a long-lived lane, a temp worker. Seats are persistent; the session sitting in one can change.
- **Generation**: incremented by 1 each time a seat onboards a new session; the old binding is voided at the same time.
- **Onboard**: the only way GEAR makes an Agent session the occupant of a seat. **In the MVP every managed Agent is started by GEAR** (fresh, or resumed per profile); GEAR does not take over existing sessions it did not start.

## Fixed call context (replaces "look up the latest generation by pane on every call")

At onboarding, the engine first sets the environment in the target pane's shell, then starts the Agent:

```text
pane run <pane> "export GEAR_CTX=<project>:<seat>:<gen>; export GEAR_DB=<state_dir>/gear.db"
agent start <name> --kind <kind> --pane <pane> -- <args or resume args>
```

The Agent process and all its children (including every `gear` it invokes) inherit `GEAR_CTX`. The Agent does not need to remember any arguments.

The onboarding target must be a checked, usable shell pane: if an existing Agent has not exited or its state is unknown, do not type the export/start commands; first finish stopping the old session and checking resources. Environment setup and start are confirmed as separate steps; `pane run` returning does not mean the shell has finished the export; environment inheritance is enabled only after passing the probe in 15.

- On a write request, `gear` reads `GEAR_CTX` and checks inside the transaction: the seat exists, this generation = the seat's current generation, the binding is not voided, and the seat state allows the operation. Generation mismatch → `STALE_GENERATION`.
- Late commands spawned by an old session carry the old generation and are always rejected after rebinding; `gear` **never** reads and adopts the latest generation by pane.
- `HERDR_PANE_ID` and the read-only `herdr agent get` fingerprint are only additional consistency checks (pane mismatch, occupant type mismatch → reject), not a source of identity. Herdr queries are done **outside** the write transaction; inside the transaction only the database binding is re-read.
- Calls without `GEAR_CTX`:
  - from the user entry point `gear admin …` (run by the user in their own shell; requires `GEAR_ADMIN=1` or interactive confirmation) → the user;
  - anything else → no role, read-only `gear --all`, all writes rejected. **Not finding a seat never makes the caller the user.**
- Migrating an existing project: existing Agents must be restarted and onboarded by GEAR (CLIs that support resume use resume to keep context).

> This guards against misrouting and mistaken operations by old sessions, not against malicious forging of environment variables under the same account.

**Item references**:
- `result` and reports that change task state (including `blocked`) must explicitly reference the original task/attempt/contract (the task packet gives the full command); they are not inferred from the seat's current task. After the same session finishes T-1 and takes T-2, a late result/blocked for T-1 still belongs to T-1's original attempt.
- `ack` is checked according to the type of the item being replied to: task-related ACKs (contract update, preemption) take the reference from that item and check it; **a handover ACK references only the handover item and its pending binding/generation, and needs no task arguments**; ordinary question replies reference the original item.
- Unreferenced `progress` and `question` are only seat-level information/questions; they belong to no attempt and do not change task state.
- **An incoming captain awaiting handover** (pending binding) may only read its own handover packet and submit that handover's ACK; all other writes are rejected.

## External interface

```python
@dataclass
class Caller:
    kind: Literal["seat", "admin", "anonymous"]
    project: str | None
    seat: Seat | None
    generation: int | None
def resolve_caller(conn, env: Mapping[str,str], herdr_ro: HerdrReadOnly) -> Caller   # queries outside the transaction
def require(conn, caller, *, roles=None, states=None) -> Seat   # re-read and check generation inside the transaction

def onboard_intent(conn, seat_id, pane_id, kind, profile, resume_ref: str|None) -> str
    # inside a transaction, reserve a unique new generation and create a pending binding, return the op id; retries/restarts reuse the same operation and generation
def onboard_complete(conn, op_id, fingerprint: Fingerprint) -> None
    # only confirms the start result of the same binding, no further increment; ordinary seats activate on confirmation and the old binding is voided;
    # an incoming captain stays pending until the ACK for this handover
def mark(conn, seat_id, state: SeatState, reason: str) -> None
```

Data (02's `seats` and `bindings` tables): one binding per onboarding (seat, generation, execution host/session, pane, kind, session_ref, status active/void/pending).

## Seat state transitions

```text
Captain:   ACTIVE → UNAVAILABLE (engine-detected, permissions frozen; no automatic recovery)
           ACTIVE/UNAVAILABLE → HANDOVER (user-initiated) → ACTIVE (new generation, after the incoming captain confirms)
           UNAVAILABLE → ACTIVE also requires explicit re-onboarding by the user (even the same session gets a new generation)
Lane:      ACTIVE ⇄ STANDBY ⇄ PARKED; ACTIVE/STANDBY → HELD → ACTIVE; → RETIRING → RETIRED;
           any → MISSING (engine-detected) → NEEDS_CHECK (after user re-onboards, write resources need check) → ACTIVE (user confirms)
Temp:      QUEUED → STARTING → ACTIVE → RELEASING → RELEASED (Agent has exited; its worktree is kept until the result is merged, resumable)
           RELEASED → STARTING (resume or new session); → RETIRED
           STARTING/RELEASING outcome unknown → UNKNOWN (counts against quota, needs check)
```

Seats in RELEASED or QUEUED are expected to have no Agent; 08 does not judge them MISSING.

## Manual captain replacement (the only way in the MVP)

User: `gear admin captain replace --project oxford --pane w5:p2 --kind codex [--resume <session_ref>]`

1. **In one transaction**: seat → `HANDOVER`; the old binding is voided (onboard_intent reserves a new generation for the new session; all writes from the old generation are rejected from then on); record the incoming target (pane, kind, this handover's version); classify in-flight operations one by one and write them to the database:
   - INTENDED, not executed → kept, continue after the new captain confirms;
   - EXECUTING/UNKNOWN → marked "needs check";
   - messages the captain replied to but the other side has not confirmed → kept as is;
   - actions by the old captain outside gear → the engine does not infer them, records "out-of-gear actions unchecked".
   During handover: `gear reply` and `gear plan apply` are rejected for the project; lanes' `gear send` is stored as usual; authorized execution tasks that do not depend on the handover outcome continue; **operations affected by the handover or with unclear impact stay frozen**.
2. The engine starts the new captain in the target pane via the onboarding flow (with a new `GEAR_CTX`, binding status pending), generates the handover packet `handover.md` (goal, plan version, unhandled messages, waiting items, last 20 decisions, operations needing check, frozen items, next review point, the skill version required this time), and delivers a pointer (merged into the same delivery as the skill entry pointer, 16). The handover ACK also confirms that skill version.
3. **Confirmation**: the new captain runs `gear send --kind ack --reply-to <handover item ID>`. Only a confirmation from the incoming target (the pending binding's generation) for this handover item is accepted. After confirmation, activate atomically: binding active, seat ACTIVE, captain writes unfrozen. Confirmation does not automatically release "needs check" operations and write resources; they stay on the new captain's home page.
4. The new Agent fails to start or does not confirm → stay in HANDOVER, shown prominently on the board; there are never two valid captains. When the old captain recovers it is not automatically reinstated.

## Captain unavailable (engine-detected, no automatic replacement)

Detection: Herdr reports exit/close; the screen repeatedly matches quota/auth signatures; there are pending items and, after the last doorbell was confirmed started, there is still no provable handling. Result: seat `UNAVAILABLE`, captain-side writes and derived operations frozen, doorbell stopped, shown on the board, `notify` the user. Temporary rate-limit states are only annotated, generation not frozen (04 classification). Recovery only by explicit user re-onboarding (resume of the original session or a new session), through the same checks.

## Test points

A late write from an old session's child process after a replacement in the same pane is rejected (three cases: same brand, no session_ref, rebinding between query and transaction); an Agent call without GEAR_CTX does not become admin; captain writes rejected during handover while lane sends work; only the incoming target can confirm; handover state can be rebuilt after restart; UNAVAILABLE does not recover automatically.
