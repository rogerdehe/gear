# Field notes: serial traps

Experience post, not a rule. It records how parallel work quietly turned serial while GEAR ran its own development (2026-10-22). Whether any of this reaches the skill or the conventions is decided later, in the overall review — the skill must stay small.

The user's line: **never-stop is GEAR's soul.** Lanes keep doing valuable work; review and merge run beside them, never in front of them. And never-stop is not busywork: every queued task must move the goal forward.

## The traps we hit

| # | Trap | How it showed | What removed it |
|---|---|---|---|
| 1 | **The engine itself waited for review.** A lane that handed in a result got no new task until the verdict, because the "safe point" check was a placeholder hard-coded to `false`. | All three lanes stood idle after their first hand-in; looked like "the lanes finished their work". | A handed-in attempt (candidate pushed) is a safe point by definition; the engine assigns the next task while review runs. |
| 2 | **Queues one task deep.** Each lane got one or two tasks. | The moment a task was handed in, the lane had nothing left. | Keep 2–3 ready tasks per lane; the engine should warn the captain when a lane runs low (planned). |
| 3 | **Dependencies on tasks under review.** "T-6 depends on T-2" only because they touch nearby files. | T-6 could not start until T-2 was reviewed and merged. | Depend only on code a task cannot be built without. Nearby files are handled by "merge dev before handing in", not by waiting. |
| 4 | **Rework queued behind new work.** The sent-back task ranked after a new one. | The lane would start new work while its rejected task waited. | Rework goes to the head of its lane's queue (planned in the verdict settlement). |
| 5 | **The captain in the middle of every step.** Reviews started by hand, verdicts and merges by hand. | Every hand-in waited for the captain's turn; the captain's context filled with routine. | Engine starts reviewers, settles verdicts, merges (planned: T-5, T-12, T-13). |
| 6 | **A contract change that never reached the lane.** | The lane finished to v1 after v2 was applied; the pass then had to be attested by hand. | Deliver contract changes mid-attempt (planned: T-8). |
| 8 | **Code that assumes one task per seat.** After trap 1 was fixed, five places still took "the first open attempt" — the handed-in one — so a lane's new task was never confirmed and could not be handed in. | Lanes asked "may I take the next task?" / "settle the old one so I can hand in". | One ordering at the source: the worked-on attempt first, handed-in last. When a rule changes (one task → several in flight), grep every place that relied on the old shape. |
| 7 | **Filling queues with busywork.** Over-correcting trap 2. | A task that only duplicated another (a board "DRY" marker next to the low-queue warning). | Rank by value; cancel duplicates. Never-stop on valuable work only. |

## The pattern behind them

Each trap was a reasonable-looking safety choice ("wait until it is safe", "wait until it is merged", "let the captain decide") that, added up, made every lane wait for the slowest step. Find them at kickoff, before the lanes start: for every wait in the flow, ask "who is idle while this waits, and does it really have to?"
