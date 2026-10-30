<!-- DRAFT, not applied: applying it departs M6. The goal, done clauses, bar, eta and staffing are agreed with the user first; each task's contract is then checked against merged code (M5 lessons 28, 30, 32) before departure. -->
# Goal
A fleet that runs a whole voyage without the captain nursing it, on a machine it does not load. Every failure mode the two fleets hit in M5 (deliveries that never land, reviewers locked or pointed at the wrong commit, false alarms, dialogs that stop a seat) is fixed at its root. A seat's model or agent can be switched mid-voyage without losing work, by a handover the incoming agent reads first; the captain itself can hand over. Reviewer and lane counts stretch for a backlog and return by themselves. GEAR runs near idle, and it watches the fleet's and the machine's health. Released at each station: S1 deliveries and reviews that never stall (v0.6.0); S2 switching models, agents and captains, elastic counts, dialogs that never block, and role charters that fit the chosen models (v0.6.1); S3 GEAR near idle and the health watch (v0.6.2); S4 the captain's plan tools (v0.6.3).

# Milestone M6: a fleet that needs no nursing
done: (1) every M6 task merged; (2) the voyage never stops: while some lane has a task it can start, no lane sits idle; (3) every hand fix the captain makes during M6 is either turned into an M6 task and merged, or, only when it needs a sweeping change, written into gear/voyages/M7/backlog.md; (4) every recovery the captain makes by hand during M6 (a nudge, a release, an answered dialog) is recorded as a decision and becomes an M6 task that is merged, and the voyage report shows no false stall item; (5) the engine and each board stay under 10 % of one core and 50 MB on average over the last station, measured from the probe lines; (6) released at each station, the last as v0.6.x
bar: DELIVERABLE: an error in normal use blocks the task; edge cases outside normal use are logged
eta: (to agree with the user)
station S1: deliveries and reviews never stall, released as v0.6.0
station S2: models, agents and captains switch with a handover; elastic counts; dialogs never block; role charters fit the chosen models, released as v0.6.1
station S3: GEAR near idle and the fleet's health watched, released as v0.6.2
station S4: the captain's plan tools, released as v0.6.3

# Decisions

# Queue
<!-- Lanes as in M5: hull owns crates/gear-engine, gear-app, gear-store, gear-model, gear-usage; mast owns gear-cli, gear-herdr, gear-board, gear-forge, skills, docs, Makefile. Estimates are first cuts; each contract is written in full, against merged code, before departure. -->

## NEW [now] lane=mast milestone=M6 station=S1 required
mechanical: one sweeping `cargo fmt` before any lane branches, then the check in the Makefile; the gate proves it
goal: [S1] The tree is rustfmt-clean and stays so (M5 F-140): one `cargo fmt --all` commit at the voyage's start, then `cargo fmt --check` in `make check` and `make gate`.
accept: `cargo fmt --all --check` exits 0 on dev; `make -n check` and `make -n gate` show it. The merge gate runs `make gate`.
estimate: 10

## NEW [now] lane=hull milestone=M6 station=S1 required
goal: [S1] A delivery counts as landed only when the agent shows it took it: the attempt's first check-in, or the agent's idle-to-busy turn after the text was submitted with an empty composer, within a bounded window; one resend after the window, then a captain item. Covers: text left in the composer without its Enter, a command joined to the packet ('/new' + packet), a delivery typed into a busy agent (today's false stall items), and a fresh Codex conversation that starts empty. Each step that types confirms its Enter took (the composer emptied) before the next step types.
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

## NEW [now] lane=hull milestone=M6 station=S1 required
goal: [S1] A verdict is accepted only when the review branch is on the remote at the reviewed commit; otherwise the engine pushes the seat's local branch once, and tells the captain if that fails (T-265, T-270).
accept: Engine tests: a verdict with no remote review branch triggers one push; a failed push is an item, not an accepted verdict. Run make check CRATE=gear-engine.
estimate: 25

## NEW [now] lane=mast milestone=M6 station=S1 required
goal: [S1] The codex profile handles Codex 0.165's startup screens: after `/new` it answers "Where should the new conversation run?" with the current worktree, on a resume it chooses the seat's own directory, and Herdr's done/idle on those screens is read as blocked. Slash commands are typed in one go.
accept: gear-herdr tests with the two screens captured from M5; a real Codex start in a test worktree reaches an empty composer with no dialog left. Run make check CRATE=gear-herdr.
estimate: 40

## NEW [now] lane=hull milestone=M6 station=S1 required
goal: [S1] Smaller M5 fleet bugs: a lane that reports blocked gets its next startable task; a held seat may still report a finished hand-in; a repeated confirmed goal/bar/eta change records its second decision; the lost-merge scan runs before the old filters; the slow-step report includes a failed observation.
accept: One engine or app test per item. Run make check CRATE=gear-engine and CRATE=gear-app.
estimate: 50

## NEW [now] lane=mast milestone=M6 station=S1 required
goal: [S1] gear-cli's two tests that share the scratch tag "stale" each get their own (F-153), so plain `cargo test` passes; `gear admin init` names each reviewer by its entry's CLI and checks its model against that CLI.
accept: `cargo test -p gear-cli` passes twice in a row; an init with [temps] kind pi and a codex review entry prints "reviewers: … codex …". Run make check CRATE=gear-cli.
estimate: 20

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
goal: [S2] Elastic counts: a plan line raises a lane or reviewer quota for a while with its reason and end condition (e.g. until no hand-in waits), confirmed with the user; the engine returns to the normal count by itself when the condition holds and records both changes.
accept: Engine and app tests: temps raised to 8 starts extra reviewers while hand-ins wait and releases them back to 4 when none wait. Run make check CRATE=gear-engine and CRATE=gear-app.
estimate: 50

## NEW [now] lane=hull milestone=M6 station=S2 required
goal: [S2] Dialogs never block: a lane's or reviewer's blocking screen becomes a captain item; the engine closes it if the captain has not handled it within a default time (15 minutes); a captain dialog left unanswered for 20 minutes is cancelled by the engine. The skill says the captain opens a dialog only for a change the user asked for.
accept: Engine tests: a blocked reviewer rings once and is closed after the timeout; the captain timeout cancels. Run make check CRATE=gear-engine.
estimate: 40

## NEW [now] lane=mast milestone=M6 station=S2 required
goal: [S2] Role charters fit the models the user chose. At kickoff the captain proposes, and the user confirms with the staffing, one charter per role (captain, each lane, reviewers) from the CLI and model each runs: a lane on a fast, simple agent (pi on a flash model) implements the packet without designing, as today's rules/lane.md says; a lane on a strong agent and model (e.g. Claude on Opus) designs inside its own task and owned paths, and still never decides for the whole project; the captain always holds the whole picture. The charter generalises today's fixed rules files (skills/gear/references/rules/lane.md and review.md): it is written into gear.toml per lane and for the reviewers, rendered into the seat's rules file, and given to an agent once per conversation (at a new session, never again in a resumed one). The skill drops the absolute "the captain writes no product code": the captain may change a few lines itself when that removes a blocker faster than any packet could (a conflict, a one-line premise the lanes would guess at), records it as a hand fix, and the change still goes through review and the merge gate.
accept: gear-app tests: a config with two charters renders two rules files; a resumed session gets no charter again, a new one does. The skill's captain reference and conventions describe the charter and the captain's small-change rule. Run make check CRATE=gear-app and CRATE=gear-cli.
estimate: 50

## NEW [now] lane=hull milestone=M6 station=S3 required
goal: [S3] GEAR near idle: read the M5 probe lines (T-407) and fix the slow operations they show, starting with every multi-step operation that waits a whole tick between steps; the engine under 3 % of one core and 50 MB on average.
accept: A probe summary before and after, from a real voyage's log; engine tests for each changed step. Run make check CRATE=gear-engine.
estimate: 60

## NEW [now] lane=mast milestone=M6 station=S3 required
goal: [S3] The board near idle: it redraws only what changed and reads the store incrementally (M5 measured 20-34 % CPU per board); under 3 % of one core and 50 MB.
accept: A measured board run before and after; board tests. Run make check CRATE=gear-board.
estimate: 50

## NEW [now] lane=hull milestone=M6 station=S3 required
goal: [S3] Fleet and machine health watch, costing almost nothing: the CPU and memory of each seat's own processes (children included) and the machine's load, memory and disk, sampled each minute; a seat over the safe line is interrupted and stopped, and the captain is told; a machine short for other reasons is an urgent captain item naming the top processes (never touched).
accept: Engine tests with injected readings for both cases; the watch's own cost measured under 1 % of one core. Run make check CRATE=gear-engine.
estimate: 60

## NEW [now] lane=mast milestone=M6 station=S3 required
goal: [S3] Builds cache across worktrees: sccache hits our own crates (the M5 hit rate was 1.4 % because paths differ per worktree), by a shared target directory or path remapping, with the build-slot shares unchanged.
accept: A second worktree's build of the same commit hits the cache for our crates (measured); `make check` unchanged in result. The merge gate runs `make gate`.
estimate: 40

## NEW [now] lane=hull milestone=M6 station=S4 required
goal: [S4] Plan tools: `state: hold` keeps a task out of delivery until set ready; a milestone departs only on an explicit `depart` line; a milestone with no task and no history can be removed; a plan apply warns when two Ready tasks name one file.
accept: App tests for each. Run make check CRATE=gear-app.
estimate: 50

## NEW [now] lane=hull milestone=M6 station=S4 required
goal: [S4] Branch hygiene and coverage: a cancelled task's branch is deleted; a merged task's branch is deleted even when its tip moved past the merged candidate (its extra commits listed to the captain first); `gear report coverage` lists accepted attempts not on the integration branch.
accept: Engine and app tests. Run make check CRATE=gear-engine and CRATE=gear-app.
estimate: 40

## NEW [now] lane=mast milestone=M6 station=S4 required
goal: [S4] `gear issue file` lets a captain file a bug issue through the engine's forge ops (once, retried, recorded in the plan), for projects whose policy asks for one.
accept: gear-forge tests against the fake forge; the skill's github reference. Run make check CRATE=gear-forge.
estimate: 35

## NEW [now] lane=mast milestone=M6 station=S4 required
goal: [S4] Release M6's stations: version, notes and the skill review for v0.6.0 to v0.6.3, one task per station in the form of M5's T-367 to T-369 (written when M6 departs, one per station).
accept: As M5's release tasks. The merge gate runs `make gate`.
estimate: 15
