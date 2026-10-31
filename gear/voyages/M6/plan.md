<!-- gear-plan project=gear base=932 -->
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
- 2026-10-30 12:05 scope: moved to M5.3: working hours (depart at an agreed time, run inside configured hours, notice 15 min before the end, release every agent at the end with worktree and session kept, bring them back at the next start with no confirmation, the voyage clock counts working time only) | why: user | else: -
- 2026-10-30 12:40 scope: dropped the task "a review round resumes only on the seat that ran it": session.rs rule V1 already resumes only when the earlier round ran on this seat with the same CLI; gear-app and gear-store move to mast (the goal-integrity and plan-tools tasks with them) so the loads are hull 575 min, mast 380 min; one release task per station; the captain formats the tree in one commit before departure | why: balance the lanes; a format commit after lanes branch would conflict with every branch | else: -
- 2026-10-30 12:45 scope: added to S1: a direction check on a rework attempt reaches the lane as information, not as an urgent interrupt (another project's F-310) | why: noise that interrupts a lane mid-fix | else: M7
- 2026-10-30 12:55 scope: added to S1: an instruction sent during a task does not pin the lane as helping after its hand-in (another project's M-5128) | why: it stops deliveries for 30 min | else: M7
- 2026-10-30 12:40 model: departs by itself at 14:00 (`[schedule] depart_at`), the build of M5.3's T-448 | why: user | else: -
- 2026-10-30 14:20 note: departs now instead of 14:00: the captain waited for a word the user had already given; `[schedule]` is left out of the config
- 2026-10-31 14:40 scope: the S4 branch-hygiene task also names direction branches with the project and deletes them when their job closes (another project's report) | why: stale refs without a project prefix | else: M7
- 2026-10-31 15:20 cut: T-479 cancelled: mast had already taken its default (b) before my answer arrived and wired the Codex answer step in session_runner within T-457; it stays there, reviewed with T-457 | why: the work is done once | else: move it to hull and lose an hour
- station M-6568: keep course — 60 min in, T-453 in review, T-457 in rework, T-454/T-459/T-460 in work; the first hour went to reading code and one gate failure, the pace is judged again at the next check

- station M-6577: keep course — rebalanced: mast's projection (944 min) is the bottleneck, hull's 363 min; T-461 and T-473 (both in gear-app, named in their contracts) move to hull
- 2026-10-31 15:50 scope: T-461 and T-473 move from mast to hull; their contracts name the gear-app files they change (a rebind serves only a multi-repo project) | why: station M-6577 projected 16.8 h against 12 h, mast the slowest lane | else: a third lane (the user's call)
- 2026-10-31 16:10 scope: added to S1: a rework's review goes to the seat that reviewed the round before, resumes its conversation, gets no direction check and sees the rework and the delta (another project's T-404) | why: user: a re-review should take minutes, not a full read | else: -
- 2026-10-31 16:12 cut: T-462 cancelled: T-480 starts no direction check on a rework attempt, so none can interrupt the lane | why: the same harm removed at its source | else: -
- 2026-10-31 16:30 scope: first in hull's queue: a lane keeps its conversation from task to task unless the captain marks `fresh: <why>`; a CLI change starts fresh with the earlier session named for reference | why: user: the M4 rule L7 (new conversation unless `followup-of`) reversed the user's design; the captain's error | else: -
- 2026-10-31 16:50 scope: audit of the user's decisions against the code (nine departures): sessions (T-481, T-480), new-session recovery mark (T-481), CLI switch handover (S2 agent-switch task and T-481's reference line), reviewer idle release by time (elastic counts task), strong-model lanes may design (role charter task), captain cannot lower the bar (T-461), reviewer named after its seat (new S4 task); the 'round cap escalates to the user' line was not the user's: docs/design/17-conventions.md now says the captain decides | why: user: priorities may move, his design never | else: -
- 2026-10-31 17:10 hand-fix: skills/gear/references/captain.md drops "you write no product code" and says the captain fixes a few-line block or an urgent fault itself (commit, merge gate, one decision line, release if users wait), a task only when more is needed and nothing urgent waits | why: user: in an emergency lane and review rounds defeat the tool's purpose, speed
- 2026-10-31 17:40 hand-fix: a reply resolves a late merge-failed item of a cancelled task (verdict_hint skips cancelled tasks) | why: another project's late O-4007 item on cancelled T-404 asked for a remerge or a fail verdict that a cancelled task cannot take
- station M-6645: keep course — projected 616m of 720m; hull carries 496m against mast's 232m, so when mast runs dry the next unstarted hull task that touches no engine file moves to mast
- 2026-10-31 18:20 hand-fix: deleted origin/review/t457 by hand | why: T-457 was merged (O-5340) but the merge left its review branch, which rang the 120-min branch item; root cause in the M6 backlog
- 2026-10-31 18:40 scope: added to S1 after T-480 (hull): a new attempt's review starts from a review branch at its candidate, and a merge deletes the review branch it merged | why: T-457's re-review could not push (F-238) and its merged review branch rang the branch item (M-6655); a review that cannot push is a stalled review, S1's goal | else: -
- station M-6669: keep course — projected 651m of 720m, but hull carries 516m against mast's 177m; T-461 (S1) and T-473 (S4), both in gear-app, move back to mast (hull 406m, mast 287m)
- station M-6702: move — projected 809m of 720m, hull the bottleneck (634m against mast's 252m); T-458, T-468, T-472 and T-482 move to mast (hull ~464m, mast ~422m); they touch engine files apart from hull's S1/S2 work, and the second to merge merges dev first

# Queue

## T-452 [now] lane=mast milestone=M6 station=S1 required
mechanical: the check in the Makefile; the captain commits the one `cargo fmt --all` before departure; the gate proves it
goal: [S1] The tree stays rustfmt-clean (M5 F-140; the captain formats the whole tree in one commit before departure): `cargo fmt --all --check` runs in `make check` and in `make gate`. Change the Makefile only.
accept: `cargo fmt --all --check` exits 0 on dev; `make -n check` and `make -n gate` show it. The merge gate runs `make gate`.
estimate: 5

## T-481 [now] lane=hull milestone=M6 station=S1 required
goal: [S1] A lane keeps its conversation from one task to the next, as the user decided: tasks on one lane continue the same context unless the work is unrelated (today rule L7 of gear/voyages/M4/design.md §3.3 opens a new conversation for every task without `followup-of`, and the captain almost never writes it, so every task re-reads the project; the default was the reverse of the user's). Change, no other design: (1) crates/gear-engine/src/session.rs fn lane_action: for attempt 1, when the seat's current session ran on the same CLI, the action is `keep` (then `compact` when the context is large, §3.5) instead of `new`; it is `new` only when the task's block has `fresh: <why>` (the captain's mark for unrelated work), the seat's agent was just started, or the CLI changed; L5/L6 (`followup-of`) keep their resume of F's session; (2) crates/gear-app plan parsing: a task block may carry `fresh: <why>`, stored with the task, shown in `gear plan show`; (3) when a rework (L4) or a follow-up (L7/fresh) cannot use the earlier session because the CLI changed, the packet adds one line: `Earlier work on this task ran in <cli> session <session_ref>; read it for reference if your CLI can.`; and the round is marked `new-session recovery` in `gear` and on the board (docs/design/10-staff.md:51-53); (4) skills/gear/references/captain.md: one sentence that tasks on a lane continue its conversation and `fresh: <why>` marks unrelated work; and the design table in gear/voyages/M4/design.md is not edited (history).
accept: Engine tests: a second task on a lane with the same CLI gets `keep` (and `compact` when large); a task with `fresh:` gets `new`; a lane whose CLI changed gets `new` with the reference line naming the earlier CLI and session; rework rules L2-L3 are unchanged. App test: `fresh:` parses and shows. Run make check CRATE=gear-engine TESTS='test(/session/)' and make check CRATE=gear-app TESTS='test(/plan/)'.
estimate: 40

## T-453 [now] lane=hull milestone=M6 station=S1 required
goal: [S1] (M5.1's T-434 already types a command and its Enter in one step; build on it.) A delivery counts as landed only when the agent shows it took it: the attempt's first check-in, or the agent's idle-to-busy turn after the text was submitted with an empty composer, within a bounded window; one resend after the window, then a captain item. Covers: text left in the composer without its Enter, a command joined to the packet ('/new' + packet), a delivery typed into a busy agent (today's false stall items), and a fresh Codex conversation that starts empty. Each step that types confirms its Enter took (the composer emptied) before the next step types.
accept: Engine tests with FakeHerdr for each shape above; no stall item while a delivery is inside its window; a lost delivery is resent once and then rung. Run make check CRATE=gear-engine.
estimate: 60

## T-454 [now] lane=hull milestone=M6 station=S1 required
goal: [S1] A notice to a working agent waits for its next idle moment, so it never interrupts a running command (M5: overrun and quiet notices stopped a 9-minute check twice). Urgent captain instructions keep their own path. One engine notice is urgent too: "The direction check of T-<n> attempt <n> is closed …" (T-444) reaches a reviewer that is still working on that check, and M5.3 and M6 showed the reviewer keeps working (review-11 on T-449 and T-459, M-6555); the engine interrupts that agent (its profile's interrupt key) and then delivers the notice, so it ends its turn at once.
accept: Engine tests: a notice to a busy seat is held and delivered at idle; an urgent captain instruction is not held. Run make check CRATE=gear-engine.
estimate: 30

## T-455 [now] lane=hull milestone=M6 station=S1 required
goal: [S1] A review seat is free only when its agent is idle and its last job's verdict or direction result is recorded; until then the next job waits or takes another seat, so a live review never has its worktree switched under it (T-283, F-215). A job that loses its binding is told at once.
accept: Engine tests: a seat whose agent still works after its verdict gets no new job; a rebind tells the old job to stop. Run make check CRATE=gear-engine.
estimate: 40

## T-456 [now] lane=hull milestone=M6 station=S1 required
goal: [S1] Every review and direction packet opens with the seat's binding (task, attempt, current candidate, branch, worktree) and says which kind of job it is; the reviewer checks pwd, branch and HEAD against it before reading, and a hand-in for any other candidate is refused. A rework round's packet shows the latest applied rework first, as the text that governs where it differs from the goal (four cases in M5).
accept: Engine tests on the rendered packets: a direction packet's first line names it a direction check and gives only the direction command; a rework packet shows the rework above the goal; a hand-in for a stale candidate is refused. Run make check CRATE=gear-engine.
estimate: 45

## T-457 [now] lane=mast milestone=M6 station=S1 required
goal: [S1] The codex profile handles Codex 0.165's startup screens: after `/new` it answers "Where should the new conversation run?" with the current worktree, on a resume it chooses the seat's own directory, and Herdr's done/idle on those screens is read as blocked. Slash commands are typed in one go.
accept: gear-herdr tests with the two screens captured from M5; a real Codex start in a test worktree reaches an empty composer with no dialog left. Run make check CRATE=gear-herdr.
estimate: 40

## T-458 [now] lane=mast milestone=M6 station=S1 required
goal: [S1] Smaller M5 fleet bugs: a lane that reports blocked gets its next startable task; a held seat may still report a finished hand-in; a repeated confirmed goal/bar/eta change records its second decision; the lost-merge scan runs before the old filters; the slow-step report includes a failed observation.
accept: One engine or app test per item. Run make check CRATE=gear-engine and CRATE=gear-app.
estimate: 50

## T-459 [now] lane=mast milestone=M6 station=S1 required
goal: [S1] `gear admin init` names each reviewer by its entry's CLI and checks its model against that CLI (the scratch-tag half moved to M5.3).
accept: An init with [temps] kind pi and a codex review entry prints "reviewers: … codex …". Run make check CRATE=gear-cli.
estimate: 15

## T-460 [now] lane=mast milestone=M6 station=S1 required
goal: [S1] `gear queue` and the board name an attempt whose review is in and waits for the captain as "waits for your verdict (M-<id>)", not only "submitted" (M5.1: another project's captain read a finished review as a stall).
accept: CLI and board tests: an attempt with a recorded fail verdict and an open item shows the text with the item id. Run make check CRATE=gear-cli and CRATE=gear-board.
estimate: 15

## T-461 [now] lane=mast milestone=M6 station=S1 required
goal: [S1] A captain cannot lower the bar on its own; every such change goes through the user's word in the conversation and stays visible (2026-10-30: another project's captain cancelled T-70 into a new milestone `M1.1-cancelled`; today a captain can also write a new milestone with an easy done sentence, move hard tasks into it, or cancel tasks until "every task merged" holds). The user confirms in the conversation with the captain; GEAR cannot see that conversation, so it enforces the path and the record, not the user's identity. Change crates/gear-app (plan.rs, goal.rs, voyage.rs and the plan parser as needed), no other design: (1) one confirmed path: the goal, done, bar or eta of a departed milestone, the departure of a new milestone, and moving a task between milestones are applied only with `--confirmed` and a `# Decisions` line `- <time> confirmed: <what changes, before -> after> | why: <the user's reason>` in the same apply; without both, the apply is refused and nothing is written; (2) a cancel line changes the state only: a cancel line that also changes the task's milestone, lane or any other field is refused; (3) the goal check is written by the captain, one line per done clause, `- done (<n>): met | evidence: <what proves it>` or `- done (<n>): open | <why>`, stored as today's `goal-check:<project>:<milestone>` meta the board already reads; (4) `station <id>: land` is refused while a done clause is not met or while the tasks cancelled or moved out since departure exceed 20 % of the departure's tasks, unless the apply is `--confirmed` with a `confirmed:` line naming the open clauses or the cut; (5) the voyage report lists every `confirmed:` line, every cancelled or moved task with its line, and each done clause with its evidence. The skill's captain reference says in one paragraph: these changes are the user's, asked in the conversation; the decision line is the record. The code change is in gear-app only, plus that paragraph in skills/gear/references/captain.md (an allowed edit).
accept: App tests: a goal change, a new milestone's departure and a move between milestones are refused without `--confirmed` and its line, applied with them; a cancel line that changes the milestone is refused; `done (1): met | evidence: …` lines reach the meta the board reads; a landing with an open clause or with 3 of 10 tasks cut is refused, and applied with `--confirmed` and its line; the voyage report lists them. Run make check CRATE=gear-app.
estimate: 60

## T-462 [now] lane=hull milestone=M6 station=S1 required
state: cancelled
goal: [S1] A direction check on a rework attempt never interrupts the lane (another project's M2, F-310: the check of attempt 2's first push judged work in progress, and its fail reached the lane as an urgent instruction restating the rework it was already applying). Change crates/gear-engine/src/review.rs fn finish_direction_results only, no other design: when the attempt's number is 2 or more, a failing direction result goes to the lane with `urgent: false` and `MsgState::Info`, its body prefixed "For information (your rework is the instruction): "; attempt 1 keeps today's urgent instruction; the captain's copy is unchanged.
accept: Engine tests: a failing direction result on attempt 2 reaches the lane as a non-urgent Info message with the prefix; on attempt 1 it stays an urgent Open instruction. Run make check CRATE=gear-engine TESTS='test(/review::tests::.*direction/)'.
estimate: 15

## T-463 [now] lane=mast milestone=M6 station=S1 required
goal: [S1] An instruction the captain sent while the lane worked on a task does not pin the lane as "helping the captain" after that task is handed in (another project's M2, 2026-10-30: M-5128, an informational instruction sent during T-387, had no reply, so after the hand-in `idle::helping` held the lane for up to 30 minutes and it got no next packet). Change crates/gear-board/src/idle.rs fn helping only, no other design: skip an instruction created while the lane had an attempt in progress (its task's attempt by this seat whose start is before the instruction and whose hand-in, merge or end is after it); every other rule stays.
accept: Board tests: an instruction sent during an attempt that was handed in afterwards does not make the lane helping; an instruction sent while the lane held no attempt still does, until its reply or the 30-minute limit. Run make check CRATE=gear-board TESTS='test(/idle|help/)'.
estimate: 15

## T-479 [now] lane=hull milestone=M6 station=S1 required
state: cancelled
goal: [S1] The engine answers Codex 0.165's "Where should the new conversation run?" screen after `/new` (T-457 gives the codex profile its markers and answer keys in gear-herdr; nothing sends them yet). Change crates/gear-engine/src/session_runner.rs only, no other design: after the `/new` step, when the pane's tail matches the profile's new-conversation dialog marker, send the profile's answer keys once (the current worktree) before the new session is recorded; a pane without the marker is untouched. Skip this task, with a note in the report, if T-457 removed the screen by a Codex config key.
accept: Engine test with FakeHerdr: a codex seat whose tail shows the dialog after `/new` gets the answer keys once and its new session is then recorded; a seat without the dialog gets no keys. Run make check CRATE=gear-engine TESTS='test(/session_runner/)'.
deps: T-453, T-457
estimate: 15

## T-480 [now] lane=hull milestone=M6 station=S1 required
goal: [S1] A rework's review is a short continuation, not a new review (another project's T-404, 2026-10-31: round 1 was reviewed by review-2; attempt 2's direction check then took review-2, so round 2's review went to review-1 in a new conversation and read the whole change again). Change crates/gear-engine/src/review.rs (and session.rs where the seat is chosen), no other design: (1) an attempt n >= 2 starts no direction check; (2) its review goes to the seat that reviewed attempt n-1 when that seat still exists, and waits for it while it is busy (never another seat while it lives), so rule V1 resumes that conversation; (3) its review packet adds, above the goal, the rework the captain wrote for it and `git diff <attempt n-1's candidate>..<attempt n's candidate>` as the change to judge first. (4) skills/gear/references/review.md no longer says a reviewer is "one fresh agent session": a re-review continues its own earlier review and judges the fixes of the earlier findings and the delta, not the whole change again.
accept: Engine tests: attempt 2 gets no direction check; its review waits for the seat that reviewed attempt 1 while that seat is busy and then starts on it with Resume; the packet of attempt 2 shows the rework and the diff command between the two candidates; attempt 1 is unchanged. Run make check CRATE=gear-engine TESTS='test(/review|session/)'.
estimate: 40

## T-483 [now] lane=hull milestone=M6 station=S1 required
goal: [S1] A review of a new attempt can push its review. Today origin/review/t<n> still holds the earlier attempt's review, the new candidate is not its descendant, and the reviewer's push is refused (T-457 attempt 2); a merge also left review/t457 on origin after it landed. When the engine starts the review of an attempt, it points the task's review branch at that attempt's candidate (the earlier review stays reachable in the attempt's record), and a confirmed merge deletes the review branch it merged. No other design change.
accept: Engine tests: a second attempt's review starts with origin review/t<n> at the new candidate and the reviewer's fast-forward push of its review commit succeeds; after a confirmed merge neither the lane branch nor the review branch is on the remote. Run make check CRATE=gear-engine TESTS='test(/review_branch|merge/)'.
estimate: 30

## T-475 [now] lane=mast milestone=M6 station=S1 required
goal: [S1] Release v0.6.0, station S1's release. Change, no other design: (1) the version is 0.6.0 in the workspace Cargo.toml (`[workspace.package] version`), Cargo.lock (the workspace crates' entries, as `cargo build` rewrites them), herdr-plugin.toml and every version string README.md names; (2) docs/releases/v0.6.0.md, in the form of docs/releases/v0.5.6.md: one line per task merged on origin/dev since v0.5.6 when you start your final check (run git log origin/dev --grep='^merge(T-' then), its T-number and what it changed for a user, from its goal; then "Skill": one line per change to skills/ since v0.5.6 (git log origin/dev -- skills/), or "Nothing outside the tasks above."; "Upgrading" as in v0.5.6. The captain merges dev into main, tags v0.6.0 and publishes the release; this task does not. The code change is in these files only.
accept: `cargo build --release -p gear-cli` then `target/release/gear --version` prints "gear 0.6.0"; docs/releases/v0.6.0.md lists every task merged on origin/dev since v0.5.6 at the hand-in. Run only `make check CRATE=gear-cli`; the merge gate runs `make gate`.
deps: T-452, T-453, T-454, T-455, T-456, T-457, T-458, T-459, T-460, T-461, T-463, T-480, T-481
estimate: 15

## T-464 [now] lane=hull milestone=M6 station=S2 required
goal: [S2] Switch a seat's model in place: when the new model runs on the seat's current CLI, the engine switches it through the CLI's own model command at the next idle moment; the agent and its conversation stay, and no handover is written.
accept: Engine tests with FakeHerdr: a lane whose chain head changes model on the same CLI gets the model command, keeps its session and is not restarted. Run make check CRATE=gear-engine.
estimate: 40

## T-465 [now] lane=hull milestone=M6 station=S2 required
goal: [S2] Switch a seat's agent with a handover: either at the next safe point, or at once: the engine tells the agent to wrap up and commit, waits the grace period (default 10 minutes, configurable), then exits it whether or not it handed in. The incoming agent's packet starts with the handover: it takes over another agent's work; the previous session id or transcript path to read first; the worktree's state (uncommitted changes, unpushed commits, branch, last commit); then the task packet. The handover follows gear/voyages/M6/handover.md part A (facts the engine fills when the agent cannot, the agent's own summary, free notes); the template ships in the skill.
accept: Engine tests: an at-once switch waits the grace period and exits; the new packet carries the handover block with the worktree state of a dirty tree. Run make check CRATE=gear-engine.
estimate: 60

## T-466 [now] lane=mast milestone=M6 station=S2 required
goal: [S2] Captain handover: on the user's request (confirmed in the one dialog) the outgoing captain writes the handover document from a template GEAR prints (goal, done clauses, bar, eta, state, open threads, the user's standing orders, files to read first); `gear captain take` asks the previous captain for it and, when that captain does not answer within a bounded time, points the new captain at the previous session to read and summarise. The document follows gear/voyages/M6/handover.md part B, and `gear captain handover --facts` prints its part 1. The skill's captain reference says how.
accept: gear-cli tests for the template and the take path's request; the skill text. Run make check CRATE=gear-cli.
estimate: 45

## T-467 [now] lane=hull milestone=M6 station=S2 required
goal: [S2] Elastic counts. Within the configured counts (e.g. 2 lanes, 4 reviewers) the engine starts and releases seats by itself as work comes and goes, with nobody asked. Going above them is a config change the user starts and confirms in the one dialog: a plan line raises a count with its reason and end condition (e.g. 4 to 8 reviewers until no hand-in waits for a review); when the condition holds, the engine releases idle extra seats only, never a running review, until the count is back to the configured one, and records both changes; after that it scales within the configured counts as before. A seat is released only after it has been idle for its role's `release_idle_min` (config per role, default 30), not at once when the lanes stop working (today release.rs releases standby reviewers beyond the working lanes at once), so a re-review finds its reviewer warm (the user's seat model, gear/voyages/M4/backlog.md:81).
accept: Engine and app tests: reviewers raised to 8 start while hand-ins wait; once none wait, idle extra reviewers are released down to 4 and a running review is not interrupted. Run make check CRATE=gear-engine and CRATE=gear-app.
estimate: 50

## T-468 [now] lane=mast milestone=M6 station=S2 required
goal: [S2] Dialogs never block. A lane or reviewer may not stop on a dialog at all: when its agent shows a blocking screen (an approval or a question the CLI asks), the engine closes it at once and tells the seat "Dialogs are not allowed for your role; ask the captain with gear send --kind question." Every other dialog (the captain's to the user) is cancelled when nobody answers it within 10 minutes, and the work goes on as it would have without it. The skill says the captain opens a dialog only for a change the user asked for.
accept: Engine tests: a blocked lane or reviewer screen is closed in the same tick and the seat gets the text; a captain dialog unanswered for 10 minutes is cancelled. Run make check CRATE=gear-engine.
estimate: 40

## T-469 [now] lane=mast milestone=M6 station=S2 required
goal: [S2] Role charters fit the models the user chose. At kickoff the captain proposes, and the user confirms with the staffing, one charter per role (captain, each lane, reviewers) from the CLI and model each runs: a lane on a fast, simple agent (pi on a flash model) implements the packet without designing, as today's rules/lane.md says; a lane on a strong agent and model (e.g. Claude on Opus) designs inside its own task and owned paths, and still never decides for the whole project; the captain always holds the whole picture. The charter generalises today's fixed rules files (skills/gear/references/rules/lane.md and review.md): it is written into gear.toml per lane and for the reviewers, rendered into the seat's rules file, and given to an agent once per conversation (at a new session, never again in a resumed one). skills/gear/references/captain.md already carries the captain's hand-fix and urgent-fix text (written by the captain on 2026-10-31); leave it as it is.
accept: gear-app tests: a config with two charters renders two rules files; a resumed session gets no charter again, a new one does. The skill's captain reference and conventions describe the charter, and captain.md still carries its hand-fix paragraph. Run make check CRATE=gear-app and CRATE=gear-cli.
estimate: 50

## T-476 [now] lane=mast milestone=M6 station=S2 required
goal: [S2] Release v0.6.1, station S2's release. Change, no other design: (1) the version is 0.6.1 in the workspace Cargo.toml (`[workspace.package] version`), Cargo.lock (the workspace crates' entries, as `cargo build` rewrites them), herdr-plugin.toml and every version string README.md names; (2) docs/releases/v0.6.1.md, in the form of docs/releases/v0.5.6.md: one line per task merged on origin/dev since v0.6.0 when you start your final check (run git log origin/dev --grep='^merge(T-' then), its T-number and what it changed for a user, from its goal; then "Skill": one line per change to skills/ since v0.6.0 (git log origin/dev -- skills/), or "Nothing outside the tasks above."; "Upgrading" as in v0.5.6. The captain merges dev into main, tags v0.6.1 and publishes the release; this task does not. The code change is in these files only.
accept: `cargo build --release -p gear-cli` then `target/release/gear --version` prints "gear 0.6.1"; docs/releases/v0.6.1.md lists every task merged on origin/dev since v0.6.0 at the hand-in. Run only `make check CRATE=gear-cli`; the merge gate runs `make gate`.
deps: T-464, T-465, T-466, T-467, T-468, T-469
estimate: 15

## T-470 [now] lane=hull milestone=M6 station=S3 required
goal: [S3] GEAR near idle: read the M5 probe lines (T-407) and fix the slow operations they show, starting with every multi-step operation that waits a whole tick between steps; the engine under 10 % of one core and 50 MB on average.
accept: A probe summary before and after, from a real voyage's log; engine tests for each changed step. Run make check CRATE=gear-engine.
estimate: 60

## T-471 [now] lane=mast milestone=M6 station=S3 required
goal: [S3] The board near idle: it redraws only what changed and reads the store incrementally (M5 measured 20-34 % CPU per board); under 10 % of one core and 50 MB.
accept: A measured board run before and after; board tests. Run make check CRATE=gear-board.
estimate: 50

## T-472 [now] lane=mast milestone=M6 station=S3 required
goal: [S3] Fleet and machine health watch, costing almost nothing: the CPU and memory of each seat's own processes (children included) and the machine's load, memory and disk, sampled each minute; a seat over the safe line is interrupted and stopped, and the captain is told; a machine short for other reasons is an urgent captain item naming the top processes (never touched).
accept: Engine tests with injected readings for both cases; the watch's own cost measured under 1 % of one core. Run make check CRATE=gear-engine.
estimate: 60

## T-477 [now] lane=mast milestone=M6 station=S3 required
goal: [S3] Release v0.6.2, station S3's release. Change, no other design: (1) the version is 0.6.2 in the workspace Cargo.toml (`[workspace.package] version`), Cargo.lock (the workspace crates' entries, as `cargo build` rewrites them), herdr-plugin.toml and every version string README.md names; (2) docs/releases/v0.6.2.md, in the form of docs/releases/v0.5.6.md: one line per task merged on origin/dev since v0.6.1 when you start your final check (run git log origin/dev --grep='^merge(T-' then), its T-number and what it changed for a user, from its goal; then "Skill": one line per change to skills/ since v0.6.1 (git log origin/dev -- skills/), or "Nothing outside the tasks above."; "Upgrading" as in v0.5.6. The captain merges dev into main, tags v0.6.2 and publishes the release; this task does not. The code change is in these files only.
accept: `cargo build --release -p gear-cli` then `target/release/gear --version` prints "gear 0.6.2"; docs/releases/v0.6.2.md lists every task merged on origin/dev since v0.6.1 at the hand-in. Run only `make check CRATE=gear-cli`; the merge gate runs `make gate`.
deps: T-470, T-471, T-472
estimate: 15

## T-473 [now] lane=mast milestone=M6 station=S4 required
goal: [S4] Plan tools: `state: hold` keeps a task out of delivery until set ready; a milestone departs only on an explicit `depart` line; a milestone with no task and no history can be removed; a plan apply warns when two Ready tasks name one file.
accept: App tests for each. Run make check CRATE=gear-app.
estimate: 50

## T-474 [now] lane=hull milestone=M6 station=S4 required
goal: [S4] Branch hygiene and coverage: a cancelled task's branch is deleted; a merged task's branch is deleted even when its tip moved past the merged candidate (its extra commits listed to the captain first); `gear report coverage` lists accepted attempts not on the integration branch. Direction branches too (another project, 2026-10-31: stale `origin/direction/t-328` and a reviewer still on `direction/t-400`): their name comes from a conventions template like the review branch's, default `{project}/direction/{task}` (today crates/gear-engine/src/review.rs hard-codes `direction/<task>`), and a direction branch, local and on the remote, is deleted when its job closes. A task moved to another lane leaves its old lane's task branch behind (another project's T-199: done on surface, stale `api/t199` left, fully an ancestor of dev): when the task is done, every branch the task branch template gives for any lane that held the task is deleted when it is an ancestor of the integration branch, and listed to the captain otherwise.
accept: Engine and app tests. Run make check CRATE=gear-engine and CRATE=gear-app.
estimate: 40

## T-482 [now] lane=mast milestone=M6 station=S4 required
goal: [S4] A user decision the code departs from (audit of 2026-10-31): a reviewer's Herdr agent is named after its seat, `<project>-review-<k>`, with the task in its title (gear/voyages/M1/plan.md:25), not `<project>-review-t<n>` (crates/gear-engine/src/names.rs:29-34; skills/gear/references/conventions.md:19 follows); Change names.rs and conventions.md only, no other design.
accept: Engine test: a reviewer is named `p-review-1` and keeps the name across tasks. Run make check CRATE=gear-engine TESTS='test(/names/)'.
estimate: 20

## T-478 [now] lane=mast milestone=M6 station=S4 required
goal: [S4] Release v0.6.3, station S4's release. Change, no other design: (1) the version is 0.6.3 in the workspace Cargo.toml (`[workspace.package] version`), Cargo.lock (the workspace crates' entries, as `cargo build` rewrites them), herdr-plugin.toml and every version string README.md names; (2) docs/releases/v0.6.3.md, in the form of docs/releases/v0.5.6.md: one line per task merged on origin/dev since v0.6.2 when you start your final check (run git log origin/dev --grep='^merge(T-' then), its T-number and what it changed for a user, from its goal; then "Skill": one line per change to skills/ since v0.6.2 (git log origin/dev -- skills/), or "Nothing outside the tasks above."; "Upgrading" as in v0.5.6. The captain merges dev into main, tags v0.6.3 and publishes the release; this task does not. The code change is in these files only.
accept: `cargo build --release -p gear-cli` then `target/release/gear --version` prints "gear 0.6.3"; docs/releases/v0.6.3.md lists every task merged on origin/dev since v0.6.2 at the hand-in. Run only `make check CRATE=gear-cli`; the merge gate runs `make gate`.
deps: T-473, T-474, T-482
estimate: 15
