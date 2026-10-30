<!-- DRAFT, not applied: applying it departs M6. The goal, done clauses, bar, eta and staffing are agreed with the user first; each task's contract is then checked against merged code (M5 lessons 28, 30, 32) before departure. -->
# Goal
A fleet that runs a whole voyage without the captain nursing it, on a machine it does not load. Every failure mode the two fleets hit in M5 (deliveries that never land, reviewers locked or pointed at the wrong commit, false alarms, dialogs that stop a seat) is fixed at its root. A seat's model or agent can be switched mid-voyage without losing work, by a handover the incoming agent reads first; the captain itself can hand over. Reviewer and lane counts stretch for a backlog and return by themselves. GEAR runs near idle, and it watches the fleet's and the machine's health. Released at each station: S1 deliveries and reviews that never stall (v0.6.0); S2 switching models, agents and captains, elastic counts, dialogs that never block, and role charters that fit the chosen models (v0.6.1); S3 GEAR near idle and the health watch (v0.6.2); S4 the captain's plan tools (v0.6.3).

# Milestone M6: a fleet that needs no nursing
done: (1) every M6 task merged; (2) the voyage never stops: while some lane has a task it can start, no lane sits idle; (3) every hand fix or hand recovery the captain makes during M6 (a few lines of code, a nudge, a release, an answered dialog) is one decision line saying what and why, and a hand fix's code goes through the merge gate; the voyage report shows no false stall item; (4) the engine and each board stay under 10 % of one core and 50 MB on average over the last station, measured from the probe lines; (5) released at each station, the last as v0.6.x
bar: DELIVERABLE: an error in normal use blocks the task; edge cases outside normal use are logged
eta: 12h
station S1: deliveries and reviews never stall, released as v0.6.0
station S2: models, agents and captains switch with a handover; elastic counts; dialogs never block; role charters fit the chosen models, released as v0.6.1
station S3: GEAR near idle and the fleet's health watched, released as v0.6.2
station S4: the captain's plan tools, released as v0.6.3

# Decisions
- 2026-10-30 09:30 scope: done (3) and (4) of the first draft are one rule: a hand fix (the captain's few lines of code) and a hand recovery (a nudge, a release, an answered dialog) are allowed to clear a block at once, each recorded as one decision line; no task, contract or report is required for it, and the captain opens a task only when it judges the cause needs more than a few lines (user: the opening exists for speed, paperwork around it would defeat it); the engine and board target is 10 % of one core and 50 MB (measured 2026-10-30: engines 0-0.5 %, boards 0.4 % to 10 %, all under 30 MB) | why: user | else: no hand recovery in the last station, 3 %
- 2026-10-30 09:30 scope: every dialog times out after 10 minutes and the work goes on without it; lanes and reviewers may not stop on dialogs, the engine closes them and points the seat to the captain; counts scale by themselves within the configured ones, and going above them is a user-started config change with an end condition, after which the engine returns to the configured counts | why: user | else: per-role timeouts, captain items for lane dialogs
- 2026-10-30 09:30 scope: dropped as done in M5.1 (T-438 verdict needs the review branch on the project's remote); moved to gear/voyages/M7/backlog.md: builds cached across worktrees, `gear issue file`, remote builds; added: the reviewer told when a hand-in closes its direction check, `gear queue` naming a verdict that waits for the captain | why: the goal is a fleet without nursing; those three are speed and convenience | else: a longer M6
- 2026-10-30 09:30 model: two lanes and the models of M5 (hull, mast on pi with tokenrhythm/deepseek-v4.1-flash-1017; reviewers on pi with tokenrhythm/deepseek-v4-pro-0813); bar DELIVERABLE; eta 12h; the whole voyage decomposed before departure; handovers follow gear/voyages/M6/handover.md | why: user | else: three lanes

- 2026-10-30 10:30 scope: the captain cannot lower the bar alone: goal, done, bar, eta, a new milestone's departure and moves between milestones go through one confirmed path with a decision line; cancel lines change the state only; landing needs every done clause met with its evidence, or the user's word; the user confirms in the conversation with the captain, GEAR enforces the path and the record and shows every such change; the board's voyage list hides plan-less milestones | why: user | else: a skill sentence the captain could ignore
- 2026-10-30 11:40 hand-fix: the departure sheet: new skill topic `gear --skill departure` (references/departure.md) with its form; captain.md shows it before every voyage departs, after the whole plan is written, and starts nothing before the user's word; it ships with M6's release | why: user: captains never showed the voyage before departing, and this voyage departs with it
- 2026-10-30 12:10 model: both lanes on pi with the self-hosted hk-dgx-1/DeepSeek-V4.1-Flash-1017, thinking high (args), 600K; reviewers unchanged; measured before departure: one request reading merge.rs (~80K tokens) took 71 s alone, two at once 54 s and 65 s, both correct | why: user: try the self-hosted model | else: one lane if two at once prove too slow in the voyage, then tokenrhythm/deepseek-v4.1-flash-1017
- 2026-10-30 11:50 scope: moved to the trial voyage M5.3: the reviewer told when a hand-in closes its direction check, the board's voyage list without plan-less milestones, and the scratch-tag half of the gear-cli test task; M6 departs after M5.3, at 14:30 if the trial goes well | why: user: try the self-hosted model first | else: -

# Queue
<!-- Lanes as in M5: hull owns crates/gear-engine, gear-app, gear-store, gear-model, gear-usage; mast owns gear-cli, gear-herdr, gear-board, gear-forge, skills, docs, Makefile. Estimates are first cuts; each contract is written in full, against merged code, before departure. -->

## NEW [now] lane=mast milestone=M6 station=S1 required
mechanical: one sweeping `cargo fmt` before any lane branches, then the check in the Makefile; the gate proves it
goal: [S1] The tree is rustfmt-clean and stays so (M5 F-140): one `cargo fmt --all` commit at the voyage's start, then `cargo fmt --check` in `make check` and `make gate`.
accept: `cargo fmt --all --check` exits 0 on dev; `make -n check` and `make -n gate` show it. The merge gate runs `make gate`.
estimate: 10

## NEW [now] lane=hull milestone=M6 station=S1 required
goal: [S1] (M5.1's T-434 already types a command and its Enter in one step; build on it.) A delivery counts as landed only when the agent shows it took it: the attempt's first check-in, or the agent's idle-to-busy turn after the text was submitted with an empty composer, within a bounded window; one resend after the window, then a captain item. Covers: text left in the composer without its Enter, a command joined to the packet ('/new' + packet), a delivery typed into a busy agent (today's false stall items), and a fresh Codex conversation that starts empty. Each step that types confirms its Enter took (the composer emptied) before the next step types.
accept: Engine tests with FakeHerdr for each shape above; no stall item while a delivery is inside its window; a lost delivery is resent once and then rung. Run make check CRATE=gear-engine.
estimate: 60

## NEW [now] lane=hull milestone=M6 station=S1 required
goal: [S1] A notice to a working agent waits for its next idle moment, so it never interrupts a running command (M5: overrun and quiet notices stopped a 9-minute check twice). Urgent captain instructions keep their own path.
accept: Engine tests: a notice to a busy seat is held and delivered at idle; an urgent captain instruction is not held. Run make check CRATE=gear-engine.
estimate: 30

## NEW [now] lane=hull milestone=M6 station=S1 required
goal: [S1] A review round resumes an earlier conversation only when the same seat ran it; any other seat starts fresh (Codex keeps the writer lock of every conversation a process opened, so T-423's live-session check is not enough).
accept: Session tests: a review round whose earlier round ran on another seat chooses Fresh; the same seat chooses Resume. Run make check CRATE=gear-engine TESTS='test(/session::/)'.
estimate: 20

## NEW [now] lane=hull milestone=M6 station=S1 required
goal: [S1] A review seat is free only when its agent is idle and its last job's verdict or direction result is recorded; until then the next job waits or takes another seat, so a live review never has its worktree switched under it (T-283, F-215). A job that loses its binding is told at once.
accept: Engine tests: a seat whose agent still works after its verdict gets no new job; a rebind tells the old job to stop. Run make check CRATE=gear-engine.
estimate: 40

## NEW [now] lane=hull milestone=M6 station=S1 required
goal: [S1] Every review and direction packet opens with the seat's binding (task, attempt, current candidate, branch, worktree) and says which kind of job it is; the reviewer checks pwd, branch and HEAD against it before reading, and a hand-in for any other candidate is refused. A rework round's packet shows the latest applied rework first, as the text that governs where it differs from the goal (four cases in M5).
accept: Engine tests on the rendered packets: a direction packet's first line names it a direction check and gives only the direction command; a rework packet shows the rework above the goal; a hand-in for a stale candidate is refused. Run make check CRATE=gear-engine.
estimate: 45

## NEW [now] lane=mast milestone=M6 station=S1 required
goal: [S1] The codex profile handles Codex 0.165's startup screens: after `/new` it answers "Where should the new conversation run?" with the current worktree, on a resume it chooses the seat's own directory, and Herdr's done/idle on those screens is read as blocked. Slash commands are typed in one go.
accept: gear-herdr tests with the two screens captured from M5; a real Codex start in a test worktree reaches an empty composer with no dialog left. Run make check CRATE=gear-herdr.
estimate: 40

## NEW [now] lane=hull milestone=M6 station=S1 required
goal: [S1] Smaller M5 fleet bugs: a lane that reports blocked gets its next startable task; a held seat may still report a finished hand-in; a repeated confirmed goal/bar/eta change records its second decision; the lost-merge scan runs before the old filters; the slow-step report includes a failed observation.
accept: One engine or app test per item. Run make check CRATE=gear-engine and CRATE=gear-app.
estimate: 50

## NEW [now] lane=mast milestone=M6 station=S1 required
goal: [S1] `gear admin init` names each reviewer by its entry's CLI and checks its model against that CLI (the scratch-tag half moved to M5.3).
accept: An init with [temps] kind pi and a codex review entry prints "reviewers: … codex …". Run make check CRATE=gear-cli.
estimate: 15

## NEW [now] lane=mast milestone=M6 station=S1 required
goal: [S1] `gear queue` and the board name an attempt whose review is in and waits for the captain as "waits for your verdict (M-<id>)", not only "submitted" (M5.1: another project's captain read a finished review as a stall).
accept: CLI and board tests: an attempt with a recorded fail verdict and an open item shows the text with the item id. Run make check CRATE=gear-cli and CRATE=gear-board.
estimate: 15

## NEW [now] lane=hull milestone=M6 station=S1 required
goal: [S1] A captain cannot lower the bar on its own; every such change goes through the user's word in the conversation and stays visible (2026-10-30: another project's captain cancelled T-70 into a new milestone `M1.1-cancelled`; today a captain can also write a new milestone with an easy done sentence, move hard tasks into it, or cancel tasks until "every task merged" holds). The user confirms in the conversation with the captain; GEAR cannot see that conversation, so it enforces the path and the record, not the user's identity. Change crates/gear-app (plan.rs, goal.rs, voyage.rs and the plan parser as needed), no other design: (1) one confirmed path: the goal, done, bar or eta of a departed milestone, the departure of a new milestone, and moving a task between milestones are applied only with `--confirmed` and a `# Decisions` line `- <time> confirmed: <what changes, before -> after> | why: <the user's reason>` in the same apply; without both, the apply is refused and nothing is written; (2) a cancel line changes the state only: a cancel line that also changes the task's milestone, lane or any other field is refused; (3) the goal check is written by the captain, one line per done clause, `- done (<n>): met | evidence: <what proves it>` or `- done (<n>): open | <why>`, stored as today's `goal-check:<project>:<milestone>` meta the board already reads; (4) `station <id>: land` is refused while a done clause is not met or while the tasks cancelled or moved out since departure exceed 20 % of the departure's tasks, unless the apply is `--confirmed` with a `confirmed:` line naming the open clauses or the cut; (5) the voyage report lists every `confirmed:` line, every cancelled or moved task with its line, and each done clause with its evidence. The skill's captain reference says in one paragraph: these changes are the user's, asked in the conversation; the decision line is the record. The code change is in gear-app only, plus that paragraph in skills/gear/references/captain.md (an allowed edit).
accept: App tests: a goal change, a new milestone's departure and a move between milestones are refused without `--confirmed` and its line, applied with them; a cancel line that changes the milestone is refused; `done (1): met | evidence: …` lines reach the meta the board reads; a landing with an open clause or with 3 of 10 tasks cut is refused, and applied with `--confirmed` and its line; the voyage report lists them. Run make check CRATE=gear-app.
estimate: 60

## NEW [now] lane=hull milestone=M6 station=S2 required
goal: [S2] Switch a seat's model in place: when the new model runs on the seat's current CLI, the engine switches it through the CLI's own model command at the next idle moment; the agent and its conversation stay, and no handover is written.
accept: Engine tests with FakeHerdr: a lane whose chain head changes model on the same CLI gets the model command, keeps its session and is not restarted. Run make check CRATE=gear-engine.
estimate: 40

## NEW [now] lane=hull milestone=M6 station=S2 required
goal: [S2] Switch a seat's agent with a handover: either at the next safe point, or at once: the engine tells the agent to wrap up and commit, waits the grace period (default 10 minutes, configurable), then exits it whether or not it handed in. The incoming agent's packet starts with the handover: it takes over another agent's work; the previous session id or transcript path to read first; the worktree's state (uncommitted changes, unpushed commits, branch, last commit); then the task packet. The handover follows gear/voyages/M6/handover.md part A (facts the engine fills when the agent cannot, the agent's own summary, free notes); the template ships in the skill.
accept: Engine tests: an at-once switch waits the grace period and exits; the new packet carries the handover block with the worktree state of a dirty tree. Run make check CRATE=gear-engine.
estimate: 60

## NEW [now] lane=mast milestone=M6 station=S2 required
goal: [S2] Captain handover: on the user's request (confirmed in the one dialog) the outgoing captain writes the handover document from a template GEAR prints (goal, done clauses, bar, eta, state, open threads, the user's standing orders, files to read first); `gear captain take` asks the previous captain for it and, when that captain does not answer within a bounded time, points the new captain at the previous session to read and summarise. The document follows gear/voyages/M6/handover.md part B, and `gear captain handover --facts` prints its part 1. The skill's captain reference says how.
accept: gear-cli tests for the template and the take path's request; the skill text. Run make check CRATE=gear-cli.
estimate: 45

## NEW [now] lane=hull milestone=M6 station=S2 required
goal: [S2] Elastic counts. Within the configured counts (e.g. 2 lanes, 4 reviewers) the engine starts and releases seats by itself as work comes and goes, with nobody asked. Going above them is a config change the user starts and confirms in the one dialog: a plan line raises a count with its reason and end condition (e.g. 4 to 8 reviewers until no hand-in waits for a review); when the condition holds, the engine releases idle extra seats only, never a running review, until the count is back to the configured one, and records both changes; after that it scales within the configured counts as before.
accept: Engine and app tests: reviewers raised to 8 start while hand-ins wait; once none wait, idle extra reviewers are released down to 4 and a running review is not interrupted. Run make check CRATE=gear-engine and CRATE=gear-app.
estimate: 50

## NEW [now] lane=hull milestone=M6 station=S2 required
goal: [S2] Dialogs never block. A lane or reviewer may not stop on a dialog at all: when its agent shows a blocking screen (an approval or a question the CLI asks), the engine closes it at once and tells the seat "Dialogs are not allowed for your role; ask the captain with gear send --kind question." Every other dialog (the captain's to the user) is cancelled when nobody answers it within 10 minutes, and the work goes on as it would have without it. The skill says the captain opens a dialog only for a change the user asked for.
accept: Engine tests: a blocked lane or reviewer screen is closed in the same tick and the seat gets the text; a captain dialog unanswered for 10 minutes is cancelled. Run make check CRATE=gear-engine.
estimate: 40

## NEW [now] lane=mast milestone=M6 station=S2 required
goal: [S2] Role charters fit the models the user chose. At kickoff the captain proposes, and the user confirms with the staffing, one charter per role (captain, each lane, reviewers) from the CLI and model each runs: a lane on a fast, simple agent (pi on a flash model) implements the packet without designing, as today's rules/lane.md says; a lane on a strong agent and model (e.g. Claude on Opus) designs inside its own task and owned paths, and still never decides for the whole project; the captain always holds the whole picture. The charter generalises today's fixed rules files (skills/gear/references/rules/lane.md and review.md): it is written into gear.toml per lane and for the reviewers, rendered into the seat's rules file, and given to an agent once per conversation (at a new session, never again in a resumed one). The skill drops the absolute "the captain writes no product code" and says, in skills/gear/references/captain.md, exactly: "When a few lines of your own clear a block faster than any packet could (a merge conflict, a one-line premise, a stopped fleet), change them yourself at once: commit with a message that says why, let the merge gate run, and write one decision line (`- <time> hand-fix: <what> | why: <why>`). Nothing more: no task, no contract, no report, no tests from a lane for it. Open a task only when you judge the cause needs more than a few lines. The opening is there for speed; paperwork around it would defeat it." The same text holds for a hand recovery (a nudge, a release, an answered dialog): one decision line.
accept: gear-app tests: a config with two charters renders two rules files; a resumed session gets no charter again, a new one does. The skill's captain reference and conventions describe the charter, and captain.md carries the small-change text above word for word. Run make check CRATE=gear-app and CRATE=gear-cli.
estimate: 50

## NEW [now] lane=hull milestone=M6 station=S3 required
goal: [S3] GEAR near idle: read the M5 probe lines (T-407) and fix the slow operations they show, starting with every multi-step operation that waits a whole tick between steps; the engine under 10 % of one core and 50 MB on average.
accept: A probe summary before and after, from a real voyage's log; engine tests for each changed step. Run make check CRATE=gear-engine.
estimate: 60

## NEW [now] lane=mast milestone=M6 station=S3 required
goal: [S3] The board near idle: it redraws only what changed and reads the store incrementally (M5 measured 20-34 % CPU per board); under 10 % of one core and 50 MB.
accept: A measured board run before and after; board tests. Run make check CRATE=gear-board.
estimate: 50

## NEW [now] lane=hull milestone=M6 station=S3 required
goal: [S3] Fleet and machine health watch, costing almost nothing: the CPU and memory of each seat's own processes (children included) and the machine's load, memory and disk, sampled each minute; a seat over the safe line is interrupted and stopped, and the captain is told; a machine short for other reasons is an urgent captain item naming the top processes (never touched).
accept: Engine tests with injected readings for both cases; the watch's own cost measured under 1 % of one core. Run make check CRATE=gear-engine.
estimate: 60

## NEW [now] lane=hull milestone=M6 station=S4 required
goal: [S4] Plan tools: `state: hold` keeps a task out of delivery until set ready; a milestone departs only on an explicit `depart` line; a milestone with no task and no history can be removed; a plan apply warns when two Ready tasks name one file.
accept: App tests for each. Run make check CRATE=gear-app.
estimate: 50

## NEW [now] lane=hull milestone=M6 station=S4 required
goal: [S4] Branch hygiene and coverage: a cancelled task's branch is deleted; a merged task's branch is deleted even when its tip moved past the merged candidate (its extra commits listed to the captain first); `gear report coverage` lists accepted attempts not on the integration branch.
accept: Engine and app tests. Run make check CRATE=gear-engine and CRATE=gear-app.
estimate: 40

## NEW [now] lane=mast milestone=M6 station=S4 required
goal: [S4] Release M6's stations: version, notes and the skill review for v0.6.0 to v0.6.3, one task per station in the form of M5's T-367 to T-369 (written when M6 departs, one per station).
accept: As M5's release tasks. The merge gate runs `make gate`.
estimate: 15
