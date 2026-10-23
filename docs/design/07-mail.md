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

- Conditions: the recipient has OPEN or due WAITING items, **or has new REPLY/INSTRUCTION not yet shown** (judged from view records); the recipient is observed idle (idle/done and no busy on screen); the seat has **no delivery occupying its position** (a deliver or doorbell in INTENDED/EXECUTING/SUBMITTED/UNKNOWN); time since the last doorbell was confirmed finished ≥ cooldown. Ordinary INFO replies, once shown, no longer ring and need no ACK; progress does not ring per item; needs_ack items stay OPEN even after being shown, until a valid receipt.
- Content is one line: `You have 5 pending items (2 urgent), run gear first`; when a review is due it becomes "handle the review first" (11).
- After delivery the operation is SUBMITTED; it becomes CONFIRMED only with **a view record linked to this doorbell** (13: that home page call actually displayed the item IDs the doorbell referred to). Without related evidence: do not resend; beyond the check budget (default 3 tick periods × cooldown) → mark the seat "needs check" and notify. working by itself does not count as confirmation.

## Test points

Ten items, one handled; two questions on the same task, only one answered; under an urgent flood the reserved ordinary place is not squeezed out by the same sender; a single sender flooding many threads is capped; new arrivals after fetching are not closed; replies do not trigger infinite confirmation; ack only closes listed items; stored during handover without ringing; an unconfirmed doorbell is not resent even after cooldown; replay after a lost receipt post-COMMIT.
