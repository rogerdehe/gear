<!-- gear-plan project=gear base=1287 -->
# Goal
A project can run with its captain alone: one agent plans, works and lands a small voyage on GEAR with no lanes and no reviewers, and the board and the captain's page tell the truth about it. Built by running GEAR's own M7.2 that way, and shipped as a v0.7.x patch live for every project on this machine.

# Milestone M7.2: the captain alone, released
done: (1) a captain-only project plans without lanes: a task with no `lane=` is the captain's, and no apply is refused as a stop because no lane has work; (2) the captain records its own work: a task is marked done with the commit that holds it, and it reads as merged once that commit is on the integration or release branch; (3) the board and `gear` show a captain-only voyage as it is: its tasks, its progress and its landing, and no stale noise from earlier voyages; (4) the skill tells a captain how to work alone; (5) released as a v0.7.x patch, installed, skills reinstalled, both engines on it, the Prometheus captain told
bar: DELIVERABLE: an error in normal use blocks the task; edge cases outside normal use are logged
eta: 4h
depart: now

# Decisions
- station M-8891: land | why: all 6 M7.2 tasks are done and merged on dev; the five done clauses are met below; released as v0.7.23, live on both engines
- done (1): met | evidence: T-684 (a task with no lane= is the captain's; captain_alone_contract) and the feed fix in T-685's commit (no stopped-voyage item for a captain alone; feed_contract); M7.2 itself was planned without lanes
- done (2): met | evidence: T-685: `- T-n done commit=<sha> evidence=` records the task done and merged, refuses an unpushed commit and a lane's task (captain_alone_contract); every M7.2 task was recorded this way
- done (3): met | evidence: T-686, T-687, T-688: capt row, no balance line for one row, cancelled tasks out of TO HANDLE, the friction item in one line, a voyage a later departure follows reads landed, the board restarts on a new binary; GEAR's board shows only M7.2 sailing, Lawrence's TO HANDLE is empty
- done (4): met | evidence: T-689: captain.md "Working alone"; skill copies reinstalled for claude, codex, pi, opencode, grok, agy and codex-home
- done (5): met | evidence: v0.7.23 tagged and merged to main, installed, both engines started 0.7.23 at 15:57Z, the Prometheus captain told
- 2026-11-11 09:58 note: voyage report: 6 of 6 tasks done in about 50m against the 4h eta, no goal change, nothing cut, no rework; the captain alone, no lanes or reviewers. Pain points met on the way and fixed in it: the lane= refusal, no way to record own work, the stopped-voyage item, the unassigned row, the goal-check overlap; plus the user's call that one person works in one worktree of its own with no branch per task | why: the landing record
- 2026-11-11 09:10 confirmed: M7.2 departure, not departed -> departs now with the scope above, eta 4h | why: the user asked for a light mode for one agent working alone, built by running this voyage alone and fixing what hurts
- 2026-11-11 09:10 note: the pain points come from the Prometheus captain's report (alone on its M5) and from running this voyage the same way; each new one becomes a task here

# Queue
## T-684 [must] milestone=M7.2 required
goal: A plan task of a project with no lanes needs no `lane=`: it is the captain's. Today plan apply refuses `a NEW block needs lane=`.
accept: a captain-only project applies a NEW block without `lane=`; a project with lanes still refuses it.
estimate: 30
deps: -

## T-685 [must] milestone=M7.2 required
goal: The captain records its own task as done with the commit that holds it: a `# Verdicts` line `- T-n done commit=<sha> evidence=<text>` for a task with no lane. GEAR fetches the integration branch, refuses a commit that is not on it (or on the release branch), and otherwise records the task Done with an accepted captain attempt merged now, in one apply.
accept: tests: a done line on a lane-less task whose commit is on the integration branch makes it Done and merged; a commit not there is refused and nothing is written; a done line on a lane task is refused.
estimate: 60
deps: -

## T-686 [must] milestone=M7.2 required
goal: A cancelled task never shows under TO HANDLE: the board's stage of a cancelled task is cancelled, not "passed" from its last accepted attempt (Lawrence M4: T-308 and T-314, cut at landing, read "round 3/4: the captain scopes what is left").
accept: a board test: a cancelled task with an accepted attempt and several rounds is not in TO HANDLE.
estimate: 30
deps: -

## T-687 [should] milestone=M7.2 required
goal: The `gear` home shows an open friction item as one line (the count and `gear friction`), not every report in its body (Prometheus: 251 reports printed on every call).
accept: a home test: an open friction item with many reports prints one line naming the count and the command.
estimate: 30
deps: -

## T-688 [must] milestone=M7.2 required
goal: The board and the land path work for a captain-only voyage: its tasks show under the captain, its progress counts them, and the landing needs no lanes, no seat branch and no records merge. Fixed as found while landing M7.2.
accept: M7.2 itself shows on the board with its tasks and lands; a test for each fix.
estimate: 45
deps: -

## T-689 [must] milestone=M7.2 required
goal: The skill tells a captain how to work alone: a config with no lanes, tasks without `lane=`, work on the integration branch in the checkout (a worktree only for a second task at the same time), run the gate, push, then the done line; the engine's wake when the plan still has work.
accept: skills/gear/references/captain.md carries the section; the skill copies are reinstalled.
estimate: 20
deps: -
