# Holding the course (`gear --skill course`)

For the captain. A fleet of agents never runs out of things to do: every reviewer finds more, every lane can polish more. Your job is to keep all of it pointed at the voyage's goal, so the fleet does not circle one reef and forget where it was going.

## 1. The goal is written down, not remembered

- Each voyage (milestone) carries three lines in the plan, agreed with the user at kickoff: `done:` (the one checkable sentence that says it is finished), `bar:` (what blocks a task, what is only logged) and `eta:` (the user's expectation of style, see §3). Every packet and every reviewer sees them.
- The bar is the user's choice. Nobody — you, a lane, a reviewer — raises it mid-voyage.
- Log each decision in `# Decisions` as `- <YYYY-MM-DD HH:MM> <kind>: <what> | why: <reason> | else: <alternative> | changes: <scope/time/quality/risk>`; `else` and `changes` are optional. Put `★` after the dash only for a changed deliverable, accepted defect in delivered behavior, user resources or contract, or eta change over 20%. New ideas go into the next voyage.

## 2. Reviewers are screws; you mind the ship

A reviewer looks at one task and will always find more. Judge each finding with one question: **does this stop the voyage from landing (break `done:`, break the `bar:`, lose data)?**

- Yes → rework, with only that finding and the **smallest exact fix** (file, function, what to change). The lane executes it as written.
- No → log it (a backlog line) and pass.
- One review round per task is the normal case. A second round needs a reason you can write in one line.
- A finding that belongs in another lane's files goes into that lane's task, never across lanes.
- Never turn reviews off to avoid judging them.

## 3. The eta is a style; calibrate it, then hold it

- At kickoff the user says "quick landing" or "do it properly"; the eta number only sets that style. Nobody can estimate agent time up front.
- After the first two or three tasks, read `gear report time`: the measured ratio actual/estimate. Scale the remaining estimates by it.
- If the rest no longer fits the style, **shrink each remaining task to its core** (time-box it, cut the extras, move the rest to the next voyage) — do not ask for more time and do not keep the old depth. A voyage that lands at 8.5 h against an expected 8 h is on course.
- Read pace two ways: dev time alone flatters; wall clock (elapsed against the estimates of finished tasks) includes reviews, waits and merges. Decide on the wall clock.

## 4. Plan a short horizon, steer in small turns

- Keep 2–3 independent, valuable tasks queued per lane — not a full plan up front. Re-plan a lane when its horizon runs down.
- Small corrections, rarely a reversal: the rowers keep rowing while you steer.
- Never fill a queue with busywork. When a lane has no valuable work left, declare the tail (`- tail: <lanes>` in `# Decisions`) and let it stand idle — this turns its idle warnings off, so a lane that only waits for a dependency is never in the tail.

## 5. Never-stop is not never-stop growth

- Value over occupancy. A lane that keeps producing code nobody needs is worse than an idle lane.
- Skeleton first, then flesh: the bar of an early voyage blocks only what breaks the chain.
- Watch size against the goal: a task whose diff is far bigger than its goal needs is drifting.

## 6. Find serial waits before they bite

Every reasonable-looking "wait until it is safe / merged / reviewed / decided" can make every lane wait for the slowest step. For each wait in your plan ask: **who is idle while this waits, and does it have to be?** Dependencies: `gear --skill decompose` rule 3.

## 7. The signals, and what you do

The engine rings you for each of these. Answer each the same hour.

| Signal | What it means | Your move |
|---|---|---|
| Station report (25/50/75 % of eta, every 3 tasks done) | pace and projected finish against the eta | `station M-<n>: keep course` / `time-box` / `cut` / `move` in `# Decisions` |
| A lane runs dry in ~N min | its queue ends soon | add valuable work, or declare the tail |
| A lane is busy with no task while work waits | a deadlock: it never idles, so it never gets work | look at its screen; end the turn |
| Two lanes change the same files | a merge will fail | order the two tasks, or merge one first |
| A quota stop parked a lane | the lane is stopped for hours | switch it to another model, or let it wait |
| A fail verdict | a reviewer found a problem | §2: rework with the smallest fix, or log and pass |
| A merge failed | it did not land on the integration branch | `- remerge T-<n>` under `# Staff`, then `gear plan apply`: it retries the accepted candidate (a flaky gate, a push race). A real conflict or a genuinely red candidate is a code fix first, in a new task |
| Ready to land (every task done) | the voyage is finished | land it (§9) |

## 8. Rework is a contract defect first

Lanes only execute; when a task comes back, ask what the task did not say. Write these into a task's goal/accept when they apply:

1. **Purpose** — why it exists and who uses the result.
2. **Environment** — what runs at the same time; where it may be interrupted.
3. **Invariants** — the rules this code must not break.
4. **Adversarial cases** — empty data, a change mid-flow, a restart, one item failing.
5. **Real proof** — the command the writer runs in the real environment before handing in.
6. **Facts to read first** — the SOP or document that already holds the answer.

## 9. Land the voyage

When every task is done, run `gear report voyage` and write the voyage report in `# Decisions`. Review the report, ★ decisions and friction reports that led to change with the user first; record corrections as `- <date> user: <text>`. Then ask about reclaiming the lanes and release, close the item with `station <id>: land`, and freeze the voyage file. The next voyage starts with its own kickoff.
