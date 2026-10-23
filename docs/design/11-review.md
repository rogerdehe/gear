# 11 Review Scheduling

File: `gear/engine/review.py`. Budget 90–120 lines. Decisions are pure functions; reminders go through 08 ops.
Invariants upheld: reviews do not depend on the captain remembering; no endless nagging; running out of reminders does not equal completion.

## Five triggers (computed by the program, no tokens spent)

| cause | Condition (initial values, configurable) |
|---|---|
| `timer` | ≥ 45 minutes since the last review conclusion |
| `milestone` | All tasks marked `required` in the milestone are DONE (05 block header field) |
| `low_ready` | Number of ready tasks < number of available long-lived lanes, for ≥ 5 minutes |
| `idle_lanes` | A long-lived lane has been STANDBY for ≥ 10 minutes |
| `staff_change` | A lane entered MISSING / RETIRING / RETIRED / NEEDS_CHECK |

## Merging and lifecycle

```python
def evaluate(snapshot: ProjectSnapshot, open_reviews, now) -> list[ReviewChange]   # pure function; the caller reads the snapshot first
```

- At most one open item per cause at a time (`V-n`, including ones already decided and waiting for a checkpoint); while the condition persists only the detail is updated, no new item is created.
- The captain writes the conclusion in the draft's `# Review`: `V-9 keep plan=<version>: reason; next=time` or `V-9 change plan=<version>: …; next=time` (or `gear plan apply --keep V-9 --next <time> "reason"`). `plan apply` validates that the conclusion is linked to the current plan version and to that item, writes `decided` and `checkpoint`; the item is paused.
- At the `checkpoint`, if the condition still holds → the item becomes due again; if the condition has gone → closed.
- A conclusion only closes the review items it lists; it does not close any message.

## How to get the captain to do it

- The captain home page (13) always shows due reviews and the snapshot at the top (each lane's state, standby duration, ready count, backlog, recent output, blockers), **before the message batch**.
- Reminder: captain idle (observation confirms a turn ended normally), a review is due, no in-flight delivery (shares the single delivery position with the doorbell) → deliver one line "Review due (cause…), run gear". Shares the one-delivery-per-lane constraint with the doorbell.
- At most 2 reminders (10 minutes apart); still no conclusion → board marked red + `notify` the user; the item stays due and is not marked complete.
- Do not interrupt the captain while it is working; but once a review is due, the doorbell content changes to "handle the review first", so ordinary messages cannot keep pushing it aside.

## Test points

Same cause not created twice; continuous updates; checkpoint due reopens; home page still shows the review first while messages keep arriving; after two reminders the user is notified and the item is not closed.
