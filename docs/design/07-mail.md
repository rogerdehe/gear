# 07 Messages: Send, Inbox, Reply, Doorbell

Implementation: [mail.rs](../../crates/gear-app/src/mail.rs), [doorbell.rs](../../crates/gear-engine/src/doorbell.rs).
Invariants upheld: messages are confirmed item by item; send success = stored in the recipient's inbox; fetching/reading/doorbell do not count as handling; one delivery needing check per lane (shared with 09).

## Send (`gear send`, via run_request)

1. Identity (03, fixed call context); generation checked inside the transaction. An incoming captain may only submit the handover ACK.
   Arguments validated by type: `result` and `blocked` must carry the original task/attempt/contract; `ack` takes its reference from the `--reply-to` item (a handover ACK requires no task arguments); unreferenced progress/question are recorded at seat level.
2. Recipient:
   - lane default → this project's captain seat;
   - `--about T-x`: T-x must be a dependency registered in this task's contract → T-x's current owning lane; no owning lane or not a registered dependency → captain. Routing itself does not create dependencies.
   - `--to <seat>`: captain only (kind=INSTRUCTION);
   - `--reply-to M-x`: back to M-x's sender seat (if that seat has changed generation, delivered to the current occupant and annotated).
   - Captain in HANDOVER: stored as usual, doorbell paused.
3. Thread: `reply_to` inherits the original thread; a new question opens a new thread `<task>:<seq>`; progress goes to the task's progress stream.
4. State: progress → INFO; question/blocked/result/instruction → OPEN; **ack/reply** see below.
5. Side actions (same transaction):
   - `result` → 06 `submit_result` (must reference the exact task/attempt/contract);
   - `ack --reply-to <contract update item>` → 06 `adopt_contract`;
   - `ack --preempted T-x --saved <ref>` → 06 `preempt_ack`;
   - `ack --reply-to <handover item>` → 03 handover confirmation (also confirms the skill version referenced by the handover);
   - `ack --reply-to <skill update item>` → 16 records the skill version adopted by the seat's current generation;
   - `blocked --about T-x --need "<interface/output/decision and version>"` → task WAITING, recording what is waited on and the resume condition.
6. Return: `Delivered to <recipient> inbox M-88 (thread T-31:2)` + doorbell status ("will ring when recipient is idle; engine last heartbeat 10:32:05" or "engine not online, will ring after it recovers").

### How a lane "finishes handling" an item it received

- An ordinary REPLY (answer) is INFO and needs no further confirmation, avoiding an endless loop of mutual confirmations.
- Items with `needs_ack` (contract update, preemption instruction, handover item, INSTRUCTION requiring action) stay OPEN until the recipient gives a valid receipt with `gear send --kind ack --reply-to <ID>` that passes the corresponding check; a valid receipt only closes the listed original items, and the receipt itself is recorded as INFO.

## Inbox selection (called by the captain's home page)

1. Candidates: OPEN and WAITING items due for recheck.
2. If ordinary threads are backlogged, **first reserve 1 place** for the oldest unhandled ordinary thread.
3. Remaining places: threads containing urgent messages first, ties broken by oldest unhandled time; at most `per_sender` threads per sender (including the reserved one).
4. Per thread: unhandled original messages (at most 3, the count shown beyond that), the latest reply/decision, a one-line summary of the owning task; `gear --show T-31:2` expands everything.
5. INFO progress is summarized in one line per lane only in the brief.

## Reply (`gear reply`, captain, via run_request)

- Only handles the exact listed IDs (this project, sent to the captain, OPEN/WAITING).
- `resolve`: →RESOLVED and record the result; `wait`: →WAITING, recording whom it waits on and when to recheck; `note`: reply only, no state change.
- With only `--downgrade`, the default is `note`: only adjust urgency, stay OPEN/WAITING; only an explicit `--resolve` closes. `--next-check` may be combined with resolve only when this recheck was actually done; merely changing the time does not count as completing the recheck.
- The reply is written to the original sender's inbox (same thread, INFO; if it is an instruction requiring action, send an INSTRUCTION with `gear send --to`).
- `--next-check` is used for output rechecks (12): records the recheck judgement and the next check time.
- A reply does not complete tasks, does not change contracts, does not transfer write rights, and does not change task/attempt state (these only go through plan apply).

## Releasing a wait

WAITING tasks/items are re-evaluated by the engine when: the awaited upstream task passes acceptance and its output version is satisfied; the linked item receives a valid reply; the recheck time arrives. An arbitrary reply or a single "done" does not unlock it.

## Doorbell (engine)

- Conditions: the recipient has OPEN or due WAITING items, **or has new REPLY/INSTRUCTION not yet shown** (judged from view records); the seat has **no delivery occupying its position** (a deliver or doorbell in INTENDED/EXECUTING/SUBMITTED/UNKNOWN); and either the message must reach the agent while it works, or the recipient is observed idle (idle/done and no busy on screen) and the time since the last doorbell was confirmed finished ≥ cooldown. Ordinary INFO replies, once shown, no longer ring and need no ACK; progress does not ring per item; needs_ack items stay OPEN even after being shown, until a valid receipt.
- **A message reaches a lane while it works (T-130).** An urgent message is typed at once, whatever the agent kind. A captain INSTRUCTION to a lane whose agent **queues input typed while it works** (pi, claude) is typed at once too: the agent reads it after the current step. A normal message to an agent that does not queue (opencode, codex) waits for idle, as before. A working agent is told only about the messages that must reach it now; the others wait for idle. Urgent bypasses the idle check and the cooldown, never the slot check (a prompt in the same moment as a delivery is swallowed, T-71). **A notice never goes into a working agent (T-454):** the engine's own messages (the overrun question, the no-push question, the think ask, every item the engine writes) wait for the seat's next idle moment and never cut its turn, whatever their urgency, because typed into a running command they stop it (M5: two notices stopped a 9-minute check twice). A seat's word — the captain's instruction, an urgent message — keeps its path above.
- **One doorbell per message, never repeated** (T-130): an id named by an earlier doorbell of a lane or temp seat is never named again — not after the cooldown, not when that doorbell was never confirmed. A doorbell that never reached the pane covers nothing: one the engine **voided** before calling Herdr, and one Herdr **definitely refused** (op FAILED — Herdr refused it before typing anything, so the message is rung again on a later tick; review F2). An uncertain (UNKNOWN) doorbell still covers: it may be in the pane, so it is never resent. The captain keeps its deliberate stale reminder (T-40): an unanswered item is reminded once per configured period, and that is the one exception.
- Content is one line naming the message ids: `GEAR: 2 item(s) waiting for you (1 urgent): M-3 M-7. Run gear now.`; when a review is due it becomes "handle the review first" (11).
- **The sender types it itself when the message must reach a working lane** (T-130): `gear send --urgent`, and every captain instruction to a lane whose agent queues input (pi, claude), types the same one-message doorbell into the recipient's pane with `herdr agent prompt` inside the `gear send` call, before it returns. It **claims the message first**, in one write transaction that also checks whether any earlier doorbell of that seat still covers it (review F1): if an engine tick has already intended that message's doorbell, the sender skips and the engine types it; otherwise the sender's own op is written (EXECUTING, holding the seat's slot) before Herdr runs, and the reply decides its end — SUBMITTED when Herdr took the text, FAILED when it refused it (so the engine rings the message on its next tick), UNKNOWN when the outcome is unknown (never resent). One message is typed once, whoever gets there first.
- After delivery the operation is SUBMITTED; it becomes CONFIRMED only with **a view record linked to this doorbell** (13: that home page call actually displayed the item IDs the doorbell referred to). Without related evidence: do not resend; beyond the check budget (default 3 tick periods × cooldown) → mark the seat "needs check" and notify. working by itself does not count as confirmation.

## Test points

Ten items, one handled; two questions on the same task, only one answered; under an urgent flood the reserved ordinary place is not squeezed out by the same sender; a single sender flooding many threads is capped; new arrivals after fetching are not closed; replies do not trigger infinite confirmation; ack only closes listed items; stored during handover without ringing; an unconfirmed doorbell is not resent even after cooldown; replay after a lost receipt post-COMMIT; a working pi lane gets a captain instruction's doorbell in the same tick; a working codex lane gets an urgent message at once and a normal one only when idle; no message is named by two doorbells; `gear send --urgent` types the doorbell before it returns and the next tick sends no second one; a Herdr failure leaves the message stored and the engine rings it.

### Station action items (T-49)

System messages from `engine` in `station:<milestone>` threads are immutable
course snapshots. They ring through the ordinary doorbell path but stay open
until an applied plan decision names their message ID. `gear reply --resolve`
and `--wait` reject these items (including mixed batches) before changing any
message. A note adds context without handling the station.
