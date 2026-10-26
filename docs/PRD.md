# GEAR Product Plan: Continuous Multi-Agent Collaboration on Herdr

Version: v0.9 (2026-10-22). Status: revised after the walking skeleton ran end to end on a real Herdr; the user re-aligned positioning, kickoff, the work loop, the board and the runtime form (§0, §8a, §10, §15, §16). Previous version: [archive/PRD-v0.7.md](archive/PRD-v0.7.md).

Project name GEAR (**G**oals **E**xecuted through **A**gent **R**elay); Herdr plugin ID `herdr-gear`; CLI `gear`. The walking skeleton runs; GEAR is now developed with GEAR itself.

This document describes the **final form** and aims to be comprehensive; what lands first is in [TECH-MVP.md](TECH-MVP.md). Reference projects and trade-offs are in [research/2026-10-21/references.md](research/2026-10-21/references.md); Firstmate is the most important comparable reference and can be borrowed from heavily, but it is not the final form we want.

---

## 0. Positioning

GEAR is for work that is **all** of:

1. **big and long-running** — hours to days, more than one context window;
2. **multi-agent** — several agents of possibly different kinds (Claude, OpenCode, Codex …) in Herdr working together;
3. **re-planned while it runs** — the plan is living: the captain adjusts order, scope and staffing at any time, instead of writing everything up front and stopping for confirmation on every change (the BMAD failure mode).

For anything else, talk to an agent directly; GEAR adds nothing.

**The soul: lanes never stop — within a voyage.** A voyage is one milestone: it has a done-sentence and a planned duration, and it must land. Never-stop means the crew works without pause until this voyage lands as fast as it can; then the crew rests, the voyage is reviewed, and the next voyage is planned. It does not mean a voyage that never ends.

**Within a voyage:** Parallel work is the default, not an option. A lane hands in a result and immediately takes its next task; review and merge run beside development, never in front of it. And never-stop means valuable work — every queued task moves the goal forward, no busywork. Every wait in the flow must answer: who is idle while this waits, and does it have to be?

Why it works where a long chat does not: the plan, the task states and the messages live in GEAR's store, not in anyone's conversation. The captain reads `gear` (a bounded page) when woken, so it can re-plan without its context filling up, getting compacted and forgetting.

GEAR is not a planning method. Requirements and technical design may come from BMAD, Superpowers or plain conversation; GEAR starts at **execution** — dispatching work to other agents (not in-process subagents), keeping them working, reviewing in parallel, and merging. Methods are tools the captain may use; they never decide for it.

## 1. Problems to solve

The user runs several projects in Herdr at once; each project has a captain (the controlling Agent) leading several lanes. The current approach is a separate keeper script per project to nudge work along, which has exposed these problems:

1. **A lane stops and nobody picks it up**: it stops at the end of a turn; queue files cannot start a new turn; a human or the captain has to nudge manually.
2. **Plans get distorted**: projects keep changing, production bugs need to jump the queue, exploratory work cannot be thought through up front; plans split rigidly in advance drift away from actual execution.
3. **The captain drowns in messages and loses the main line**: reports are pushed straight into the captain's conversation, it keeps replying to messages, goes down rabbit holes, drops the main line and urgent items, and has no time to think about whether the overall plan needs adjusting.
4. **Only reports are read, not output**: some lanes go hours without committing and drifted off course in the first hour; some lanes did no work at all.
5. **Staffing changes are unmanaged**: temp workers are opened and never closed, piling up; long-lived lanes told to pause are forgotten forever; some leave without notice; the captain runs out of quota and is gone for a week.
6. **Every project rewrites its own scripts**; rules are scattered in script comments, and changing one nudge phrase amounts to changing the rules every lane has memorized.

**Success criteria**: whenever there is authorized work that can continue, someone (the program or the captain) is pushing it forward; every stopped lane has a clear reason, owner, and resume condition; the captain holds the main line and adjusts the plan regularly; the user does not need to nudge manually or step in to coordinate. Success is not measured by "all Agents show working".

## 2. Confirmed constraints

| ID | Constraint |
|---|---|
| C-01 | Herdr is the runtime foundation: terminal/session topology, Agent start, control and observation, worktrees, remote, and plugin hosting all use Herdr's public CLI/API; nothing is built separately |
| C-02 | The product is a Herdr plugin; the primary usage is a local Herdr client operating a remote Herdr via remote; disconnecting the local client does not stop remote supervision |
| C-03 | Each project has one captain who directly manages executors, with no supervisor layer; independent review is a task, not a management layer |
| C-04 | Captain, executor, and reviewer are not bound to an Agent brand; the same role behaves the same after switching brand, and capability differences are shown explicitly |
| C-05 | Parallel code writing uses an independent worktree by default; a write resource has only one writer at any time |
| C-06 | Not bound to development methodologies such as BMAD or Superpowers; they are only guidance that tasks may reference |
| C-07 | Day-to-day progress runs automatically under approved rules; dependencies, reordering within the authorized scope, exception handling, and task/milestone acceptance are organized by the captain per contract; approvals beyond authority and final project acceptance belong to the user |
| C-08 | Messages may be merged and summarized, but original facts are not lost; messages are not reduced by hiding anomalies |
| C-09 | The system does not rely on Agent self-discipline or any one CLI's proprietary hooks to keep running; unclear state is shown as such |
| C-10 | Project differences live in configuration and task contracts; no more copying a set of scripts per project |
| C-11 | The plugin adds only **one resident process** (the engine), running in the background with no pane; any `gear` call starts it when it is not running. The `gear` commands that Agents call are short-lived processes and do not count |
| C-13 | **No popups after kickoff.** Everything that needs the user's choice is asked once, at kickoff, in as few dialogs as possible. After that no agent — captain, lane or reviewer — opens a dialog: a dialog in the captain's pane freezes the doorbell; a dialog in a lane needs the captain to notice, read and answer it. Lanes ask through `gear send --kind question`; a lane caught in a native dialog is reported to the captain by the engine |
| C-12 | Lightweight: few commands, little code; complexity is controlled by "bounded scenarios", not solved by "piling up detection systems" |

## 3. Roles and objects

**Roles**

- **User**: sets goals, draws the authorization boundary, does project acceptance; can view, correct, pause, or change goals at any time.
- **Captain** (Agent): the project's sole coordinating decision-maker — decomposes work, orders queues, makes decisions, reviews and adjusts the plan, opens and closes temp workers.
- **Long-lived lane** (Agent): has fixed responsibilities and continuously takes tasks from its own queue; stands by when there is no work.
- **Temp worker** (Agent): opened against quota for one task with a deliverable (review, investigation, emergency) and released after delivery.
- **Engine** (program, the only resident process in the plugin): continuation, doorbell, review scheduling, output sampling, Herdr side effects, recording. It only executes rules and the captain's decisions; it makes no judgment decisions itself and does not approve out-of-bounds actions on the user's behalf.

**Objects** (each independent; none substitutes for another): project, milestone, task, execution attempt, lane, Agent session, worktree, message, decision, evidence. "Agent reports done", "process exited", "command returned success", "tests passed", "task complete", and "project complete" are different facts.

## 4. State and views

- **Single authoritative state**: one SQLite per execution host (under `HERDR_PLUGIN_STATE_DIR`), recording projects, roles, plan versions, task contracts, order, attempts, messages, evidence references, and operation receipts.
- **Write rules**: both the short-lived `gear` commands and the engine write to the database in short transactions (`BEGIN IMMEDIATE`), re-checking identity, generation, and version inside the transaction; the same `request_id` with the same parameters returns the original result, and with different parameters is rejected. **Pure data changes are completed on the spot by the command; all Herdr side effects are executed only by the engine.**
- **All Markdown files are generated views**:
  - `PLAN.md` (read by the captain): goal, current milestone, what is explicitly not being done, current decisions (including the reason for a queue jump and the resume condition for the original main line), a summary of all in-flight tasks, the near-term ready queue, and urgent and blocked items. The remaining backlog shows only a count, the oldest wait time, and how to query it. Length is capped both by line count and by body size.
  - Task packet (read by lanes): one per task.
  - Archive: completed/cancelled tasks are moved out of the view; records stay in the database and are queryable by milestone.
- **Changing the plan**: the captain edits a plan draft → `gear plan apply` validates it and applies it transactionally against its base version → the engine regenerates the views. Directly editing a generated view has no effect; a stale base version returns a conflict and the new facts; an invalid draft is rejected in full and the last valid version is kept.

## 5. Plan and tasks (living plan)

- **Task contract**: stable ID (issued in creation order, independent of ordering, never reused), goal, scope, acceptance criteria, expected artifacts, next evidence point, dependencies, priority, owning lane, authorization boundary; contracts are versioned.
- **Priority**: `now` (incident) / `must` (committed for this milestone) / `should` / `could` / `wont` (not in this version, not dispatched). Priority does not override dependencies, capacity, write rights, or authorization.
- **States**: to be refined, ready, in progress, waiting, awaiting verification, done, failed, cancelled; changes follow legal transitions. Waiting must state the reason, the responsible party, and the resume condition; a failed verification can return to fixing within budget.
- **Queue operations**: insert at head, reorder, change lane, pause, cancel, split, merge (old and new IDs linked, artifacts and history kept); each records a reason.
- **Detailed near, coarse far**: only tasks about to start need a full contract; refinement is one kind of captain work.
- **Plan change is normal**: queue jumps, reordering, and contract changes are ordinary operations, not exceptions; change records are traceable.
- **Completion**: milestone complete = required tasks pass verification + milestone acceptance; cancellation is not delivery; marking a task done must attach the contract version, the candidate version, and verification evidence.

## 6. Execution and continuation

- **Observation**: the engine periodically reads Herdr (`agent list` for all Agent states, `agent read` to read the screen when needed). Event subscription can be used as a latency optimization, but periodic reconciliation is always kept — events can be duplicated, lost, or late. An observation failure is shown as "observation failed this round", not treated as everyone having exited.
- **Three kinds of action kept separate**:
  1. The current attempt is unfinished and a normal end of turn is confirmed → continue **the same task, same attempt**;
  2. The current attempt has delivered → go to verification or wait for the next step; do not re-implement because of idle;
  3. The lane has no in-flight attempt → only then take the next item (subject to authorization, dependencies, capacity, resources).
- **Delivery**: dispatch, continuation, and doorbell all go through Herdr `agent prompt`; the content is a bounded task pointer, and rules are not broadcast with nudges. Each lane has at most one "delivered, not yet confirmed started" delivery at a time; when the result is unclear, check first; never resend because of a timeout. working does not mean progress; idle/done does not mean complete.
- **Handled separately**: rate limiting, quota exhaustion, authentication failure, waiting for authorization, manual pause, observation failure, and unknown state each have their own handling; not everything gets nudged with "continue".
- **CLI adaptation**: each CLI has a set of configurable observation parameters (busy signature, rate-limit/quota signature, screen lines to read, consecutive confirmation rounds) and capability declarations (whether it supports resume, etc.). Incidents from the old keeper (the nudge phrase triggering its own detection, done but still running, cooldown shorter than the poll interval, asking only once per HEAD) serve as regression cases.
- **In-flight contracts and queue jumps**: the current attempt stays bound to the contract version it adopted until, at a safe point, it is checked, the lane confirms, and the switch to the new version is registered; a successfully sent notification or having read the home page does not equal adoption. Emergency pauses are handled under approved rules. A queue jump first confirms the original task's safe state, saves its results, releases or hands over write rights, and only then activates the new task's attempt; the original task keeps its ID, results, and resume condition. Saving does not have to mean commit/push, but the new task must not mix in the original task's unisolated changes. When changes to dependency artifacts, interfaces, or authorization affect in-flight tasks, they must also be notified and checked.

## 7. Messages: send, inbox, doorbell

**Send contract (synchronous)**: `gear send` checks the sender on the spot → determines the recipient (the captain, or the **current** owner of the upstream task) → writes into **the recipient's inbox** in a single transaction → returns "delivered to so-and-so's inbox #ID". Failures report the reason on the spot (not registered, stale generation, target does not exist, lock timeout). An urgent message — and every captain instruction to a lane whose agent queues input while it works (pi, claude) — is typed into the recipient's pane before the command returns and is recorded as delivered; every other message is reported on a separate line ("will remind when the recipient is idle; engine last heartbeat …"), without pretending the recipient has received it. If the process disappears after commit and the result was not obtained, query it back with the same `request_id`; do not blindly resend.

The task, attempt, and session generation come from the registered context; `--about` only links to an upstream task and does not make the sender the upstream executor. Routing follows only approved dependencies; new dependencies are proposed to the captain and do not take effect or lift gates directly. A result registers the delivery and moves to awaiting verification; it does not directly complete the task, close the session, or release write rights.

**Message types**:

- Informational events (progress): only recorded and summarized; they do not occupy a pending slot.
- Action items (question, blocked, delivery, contract-update confirmation): have a stable ID, a handling status, and an outcome.

**The inbox is read by issue thread** (one task can have several issue threads):

- Across threads: urgent first, FIFO within the same level; whenever ordinary items are backlogged, each batch reserves at least one slot for the ordinary thread that has waited longest; a single sender occupies at most M threads per batch.
- Within a thread: show unresolved questions, existing replies and decisions, and the original text and IDs of new messages; the whole thread can be expanded if earlier context is needed.
- Acknowledgement: `gear reply` closes only the exact message IDs listed; unfinished ones can be marked "waiting on whom, re-check under what condition"; messages arriving after the fetch are not closed along with them. Replies are written into the other party's inbox.
- Urgency declared by the sender can be adjusted by the captain; messages over quota remain visible, are not deleted, and are not silently downgraded.

**Doorbell**: when an inbox has pending items, the engine sends a one-line reminder naming the message ids; a working lane is reached at once for an urgent message and for a captain instruction when its agent queues input (pi, claude), and every other message waits for idle. At most one unconfirmed doorbell at a time, and one doorbell per message — a lane's or reviewer's message is never rung twice (the captain's unanswered items are reminded once per configured period). No matter how many ordinary messages arrive while the captain is busy, it is not interrupted.

**Dependency routing**: lanes do not need to know any names. `gear send --about <upstream task>` goes to the upstream task's current owner; no owner, a new dependency, an out-of-authority request, or a dependency cycle all go to the captain. When blocked, state which interface/artifact/decision and version is being waited on; after the upstream meets its verification/delivery condition the engine re-evaluates and wakes the lane; failure, cancellation, or a mere "it's done" does not unblock.

## 8. How the captain operates

- **The main line lives in files, not in chat**: every time the captain is woken it gets a bounded briefing packet (goal, plan version, status of each lane, recent output, blockers, backlog, items due), with evidence expanded on demand. If the captain's context gets muddled it can switch to a new session; the whole situation is in the database.
- **Reviews are scheduled by the engine, not left to captain self-discipline**. Five triggers (computed by the program, no tokens spent): timer, milestone complete, not enough dispatchable work (fewer ready tasks than available lanes), a lane idle, a lane offline or lost contact.
  - Only one review item is kept per cause, updated while it persists (how long, which lanes are involved); the captain records a decision and the next checkpoint, and the item becomes due again at that point.
  - A review is a distinct kind of turn: it comes with a snapshot and asks fixed questions (does the plan need adjusting); it may read messages with global impact but does not reply to ordinary progress one by one.
  - At safe turn boundaries, a due review is given a chance to be handled; ordinary messages cannot keep crowding it out; an ongoing turn is not forcibly interrupted. Being busy for a long time cannot justify indefinite postponement; once the handling or recovery budget is exceeded, it is shown persistently and the user is notified.
  - The captain must submit a structured conclusion: keep the plan with a reason, or specific changes and the next checkpoint (via `gear plan apply`).
  - When the previous turn is confirmed to have ended normally, there is still a handleable review item, and there is no delivery that needs check, remind within budget, initially at most twice; do not force it in on schedule when working, waiting for authorization, or the send result is unclear. If there is still no progress, show it persistently and notify the user; a review must not be marked complete because the reminder count ran out.
- **The captain is also supervised by the engine**: if it has pending items but is stopped, it gets woken.
- **Holding the direction is GEAR's job, not the captain's discipline** (user, 2026-10-23). Workers and reviewers each mind their own piece — a screw minds its own tightness; the captain minds whether the ship floats. GEAR holds every captain to that: each milestone has a checkable `done:` sentence and a quality `bar:` (agreed at kickoff, required by `plan apply`); review packets carry them and reviewers fail only what breaks them; findings outside the bar are logged, not reworked; review rounds are capped and then go to the captain; drift (milestone age, rounds, size per hand-in) opens a direction review the captain must conclude. The captain's home page opens with the **voyage report** every time it is read — elapsed vs planned, scope at departure vs now (new requirements added, changes added by reviews, cancelled), done vs left to land, code added this voyage (product / tests / docs), rework rounds, crew idle time and rework rate. Past the planned duration or a scope growth threshold the engine opens a **land-or-extend** decision: cut scope to land, or extend with a reason (beyond its authority: ask the user). When the done-sentence holds the voyage lands: no new dispatch for it, a retrospective, then the next kickoff.

## 8a. Kickoff (the only alignment with the user)

Before execution starts the captain proposes an **execution plan** (distinct from the work plan) and the user corrects it in one pass:

1. **Captain**: the current agent, or a new one GEAR starts.
2. **Agents available**: the kinds installed in Herdr (detected, not typed).
3. **Lanes**: how many long-lived lanes and how many temp slots (e.g. OpenCode × 6: 2 long-lived, 4 temp); each lane's **name** — `<project>-<meaning>`, e.g. `gear-data`, `gear-runtime` — and the work items it owns.
4. **Models per role**: development on a fast/cheap model (tasks are cut small and concrete); review on a stronger model that differs from the writer's — a stronger model of the same vendor is acceptable when only one vendor is available.
5. **Quota policy** per model chain: **wait** for the reset (most vendors reset every 5 hours; GEAR parks the lane and resumes it at reset) or **fall back** to the next model in the chain (e.g. MiniMax → DeepSeek).

6. **The quality bar of this voyage — the user chooses, nobody changes it mid-voyage** (user, 2026-10-23): *fastest to the goal* (MVP/skeleton: only what breaks the chain or loses data is fixed; every other detail found on the way is cut — logged, never reworked), *deliverable* (normal-use errors fixed, edge cases logged), or *strict* (everything fixed, at a much higher cost the user accepts up front). Reviewers get the bar in every packet; the captain judges every finding and every added task by it; only the user may change it.

Kickoff ends with `gear admin init`; from then on the run needs no dialogs (C-13).

## 9. Staffing management

**Long-lived lanes**

- When there is no dispatchable task, the engine marks the lane **standby** and records the start time, **sending no message at all**; the lane also does not need to say "I'm free" itself.
- The captain learns that someone is stopped through the review snapshot ("standby: heat 35 minutes") and the "a lane idle" review trigger.
- The captain sets a park deadline or a supported resume condition through the plan draft, then submits it with `gear plan apply` (the MVP provides no separate park command or arbitrary condition expression language). When the time comes or the condition is met, the engine automatically re-evaluates: if there is work, dispatch it; if not, return to standby and enter the next review — it is never forgotten forever. If a new task becomes ready during standby, dispatch immediately without waiting for the deadline.
- **Manual hold / waiting for authorization** differs from park: new tasks cannot interrupt it; it must be lifted before resuming.
- A lane on long standby can be put to sleep by the captain's decision (Agent closed, session registration kept) and resumed when there is work.
- Each long-lived lane gets one worktree, created once at onboarding via Herdr (`herdr worktree create --branch home/<lane> --base dev --label <project>-<lane>`) and kept long-term; task branches are switched inside it and build caches persist. See [design/17-conventions.md](design/17-conventions.md).

**Temp workers**

- Project configuration states the quota, e.g. "long-lived 4, temp 6", and whether it includes the captain. Starting, working, and stop-unconfirmed all occupy quota; queued does not; confirmed stopped does not, and resuming requires reapplying.
- The captain registers temp work items in the plan draft (a task with a stable ID + a specified configuration); the engine checks the quota, creates the temp worker's own worktree and pane via Herdr (`herdr worktree create`, one per new task), starts the Agent, registers the session, and sends the task packet. When the quota is full, the request is queued and the captain is told.
- **Automatic release after normal delivery**: applies only to managed temp tasks that completed normally. The result explicitly identifies this run's artifacts and the safe end point; the engine checks that the report/candidate is really persisted and that work changes that must be kept have a persistent copy; idle/done is observed for two consecutive rounds, and there is no known unfinished activity or delivery that needs check. When satisfied, the session is closed per the approved policy, and quota is released only after the close is confirmed; the task, report, worktree, and branch are kept. If conditions are unclear, do not close automatically; move to needs check. Releasing quota does not mean acceptance passed, nor does it grant a newcomer write rights to the original resources.
- **Temp worker worktree lifecycle**: when the same work needs the temp worker back (rework, resume), its worktree is reopened with `herdr worktree open --branch <task-branch>`; after its result is merged, `herdr worktree remove` removes the checkout and workspace and keeps the branch.
- **resume rule** (user decision 2026-10-21): the mechanism must support bringing back the original temp worker. **New work opens a new temp worker; when the same piece of work needs it back midway (rework, follow-up questions, supplementary material), resume the original session.** Quota is reapplied for; when resume is unavailable, a new session reads the task packet to do the rework and is explicitly marked "new-session recovery".
- The captain is no longer asked on a timer "should I close this one" — closing is the default, and bringing back is a routine operation.
- **Quota exhausted**: the engine recognises the agent's quota marker and applies the kickoff policy — park until the reset time and resume, or switch the lane to the next model in its chain. Neither needs the captain or the user.

**Leaving without notice and handover**

- Handover **does not depend on cooperation from whoever left**: the successor rebuilds from durable records. Day to day, progress is required to land somewhere durable (commits, task notes, `gear send`, and captain decisions all go into the database via commands).
- The engine identifies unavailability (Herdr reports exit/close; the screen matches quota/rate-limit/authentication signatures; pending work but persistently no verifiable progress) and distinguishes temporary from long-term.
- **Captain unavailable**: freeze new captain decisions for that project; invalidate the old generation (confirmations and decisions it sends afterward are rejected); classify in-progress items one by one (not executed can be re-taken, with a receipt is confirmed, result unclear keeps being checked, waiting on external keeps waiting). Actions the old captain took outside GEAR are marked "unchecked"; the possibly affected parts are frozen and released after manual checking. **Replacing the captain is initiated only manually by the user** (user decision 2026-10-21): the engine shows it persistently and notifies the user, the user designates the new captain, and the handover flow is under "Replacing the captain" below. No automatic switch or automatic activation of a backup captain. The old captain is not automatically reinstated when it recovers. The freeze affects only that project's related decisions and affected operations; other projects are unaffected; authorized execution tasks that do not depend on the handover result can continue. Revoking a generation cannot cancel Herdr/Git actions already issued, nor can it stop an old captain that is still operating external resources directly. If the new captain fails to take over, the handover state stays visible and there are never two valid decision-makers.
- **Lane unavailable**: in-flight tasks are frozen; work not yet started can be reassigned. Write rights to the original worktree are **not automatically given to someone else** — closing the pane does not prove that background jobs it started have stopped — and are transferred only after manual checking confirms it is safe.
- **Retirement** (going offline voluntarily): stop dispatching new work; unstarted items go back to unassigned; in-flight ones save results and hand over write rights; delivered ones continue through acceptance. The roster shows the planned count, available count, retiring/lost-contact count, and their un-handed-over tasks; planned capacity changes only after retirement and handover complete. Late messages after retirement are kept and forwarded to the project's responsible party. The captain receives one merged review item.
  While retiring, results, blockers, and handover communications for the currently valid attempt are still accepted; late reports after retirement or from an old generation keep only the original text and links and are forwarded to the project's responsible party for checking; decisions, acceptance, or write-right changes are not applied under the old identity.
- **Replacing the captain (normal brand change)**: not by same name. "Captain" is a project role bound to the execution host, session, pane, session reference, and generation. Handover: freeze → invalidate old generation → classify item by item → handover packet → activate after the new captain confirms.

## 10. Output supervision and acceptance

**The work loop runs in parallel, not in series.**

1. A lane works through its queue: finishes a task, pushes its branch, hands in a result, and **immediately starts its next task** — it does not wait for review.
2. The engine starts a temp reviewer on the pushed branch (review model ≠ writer model); the reviewer pushes its findings to a review branch.
3. Verdict: pass → merged into `dev`; fail → the task goes back to the same lane as rework, ahead of its next item.

The captain decides only what the loop cannot: plan changes, questions, disputes, and acceptance of milestones.

**What the engine decides alone, and what it never does** (2026-10-23). Mechanical and reversible steps belong to the engine; judgement that changes scope belongs to the captain.
- Pass → the engine merges the exact reviewed commit behind a hard gate (build + tests); a wrong merge can be reverted. The captain samples passed tasks during direction reviews to catch reviewer misses.
- Objective failure (build/tests fail, does not merge, a P0) → automatic rework on the same lane, first in its queue.
- Other findings: each is marked blocking (it must name the part of the milestone's done-sentence or bar it breaks) or logged. Blocking findings → automatic rework with only those; logged findings → the milestone backlog. After the round cap the task goes to the captain.
Automatic settlement is on by default; it may only be switched off while these safeguards are missing.


- **Evidence points**: when dispatching, agree on the next evidence point (a small diff + test results, a reproduction record, investigation notes, experiment results …). Small commits are a means, not the only legitimate output.
- **Output sampling** (program, no tokens spent): periodically compare each lane's worktree changes and evidence files. If expected evidence is still missing after the evidence point → create a "needs check" item, without directly ruling that no work was done; changes also do not mean the direction is right. Set a cap on "longest time without anyone looking", so tasks that are constantly changing also get checked.
- **Drift check**: when due or triggered by risk, the captain (or a temp reviewer in a fresh context) compares the diff against the task contract and records the judgment, corrective action, and next checkpoint. Process re-checks record the observation time, baseline, and an identifiable snapshot; when concurrent changes make evidence inconsistent, mark it as needing resampling, and do not treat an unstable diff as the final review subject.
- **Acceptance**: the result enters awaiting verification → pin a candidate commit or artifact version → run the verification in the contract → independent review as needed (fresh context; an independent checkout when it may run tools; no forced brand change) → done only if it passes. Approval is bound to the contract version and candidate version; if either changes, re-check. Tasks requiring integration acceptance are not marked finally done until integration evidence arrives.

## 11. Code and resources

- Each writer has exclusive use of a worktree; when registering ownership, check the actual cwd, the normalized repository/worktree identity, and the active writer, not path aliases or workspace names.
- All worktrees are created through Herdr's worktree API; configuration contains no hand-written worktree paths. Default branch model: `main` (release), optional `staging`, `dev` (integration), task branches `<lane>/<task-id>-<slug>` from `dev`. Long-lived lanes keep one worktree (`home/<lane>`) and switch task branches inside it; temp workers get one worktree per new task, removed after merge with the branch kept. See [design/17-conventions.md](design/17-conventions.md).
- Changing people requires handing over write rights; closing a session, removing a worktree, and deleting a branch are three separate things; uncommitted changes, unmerged commits, and evidence must be kept.
- Shared resources such as ports, databases, and external accounts are registered separately; if unavailable or unknown, queue/block rather than share silently.

## 12. Multiple projects and identity

- One engine manages multiple projects within the same execution host/session; each project has its own captain, plan, queue, inbox, capacity, and authorization. Tasks, threads, and operations all carry the project ID; cross-project operations are rejected by default. A lane belongs to only one project.
- A project is not a workspace: one project can span several workspaces (each lane's worktree); ownership is determined by GEAR registration; workspace/pane metadata is display only.
- Identity is determined by the registered execution host, session, pane, and GEAR generation; `HERDR_PANE_ID` and similar values injected by Herdr are only used for locating. The generation is fixed to the session at onboarding and requests carry the original generation; the latest generation must not be taken automatically from the pane on each request; when binding is unclear or a pane move causes inconsistency, pause that lane and re-register manually. No protection against malicious Agents on the same account.
- Fairness between projects: a message flood or slow Herdr calls in one project do not starve the others.

## 13. Herdr's role in GEAR

Herdr is the foundation; GEAR only adds the "work layer" Herdr lacks (tasks, plans, message handling state, roles, reviews, quota). Verified against the local Herdr 0.9.1 source and docs (see references.md):

| GEAR needs | Herdr capability |
|---|---|
| Host the board | Plugin `[[panes]]` opened by a plugin action bound to a key (`[[keys.command]]`); the engine is a background process, not a pane |
| User shortcuts | Plugin `[[actions]]` + Herdr key bindings (actions take no parameters and are async; only fixed entry points, not an Agent business interface) |
| Open lanes, change people | `pane split` / `tab create`, `agent start --kind`, `worktree create/open/remove` |
| Dispatch, continuation, doorbell | `agent prompt` (types text + Enter into the pane terminal) |
| Observation | `agent list/get/read`, `pane process-info`; optional event subscription / `[[events]]` hooks |
| Locate the caller | Herdr-injected `HERDR_WORKSPACE_ID / HERDR_PANE_ID / HERDR_SOCKET_PATH` |
| Visible in Herdr | `pane/workspace report-metadata` (key names only `[A-Za-z0-9_-]`, e.g. `gear_project`), `notification show` |
| remote | Plugin and GEAR installed on the target execution host; the user views and operates via Herdr remote |
| Persistent state | Herdr provides no plugin storage; GEAR keeps its own SQLite in `HERDR_PLUGIN_STATE_DIR` |

The Herdr plugin manifest/API we verified has no persistent scheduled tasks that a plugin can declare, so timing work (park expiry, review timers, output sampling) is handled by the engine loop.

## 14. Command surface

Day-to-day business has only 4 entry points (lanes only need to remember the first 2):

| Command | Who uses it | What it does |
|---|---|---|
| `gear` | All roles | Home page: returns what each role should look at. Lane: current task packet, replies to me, suggested next step; captain: briefing snapshot, this batch of inbox threads (with prior context), due reviews. Details can be expanded by ID; `--all` only widens the viewable scope |
| `gear send` | Lanes, captain | All reporting: `--kind progress/question/blocked/result`, `--about <upstream task>`; also used to answer questions received and to confirm adopting a new task version (citing the original message ID and version) |
| `gear reply` | Captain | Handle exact message IDs: record the decision and reply; mark unfinished ones with whom they wait on and the re-check condition |
| `gear plan apply` | Captain | All plan and staffing changes: queue jumps, reordering, refinement, park/retire, temp workers, review conclusions (`--keep "reason"` when the plan is unchanged). Returns "plan version in effect" plus pending execution/handover items; actual completion of staffing changes is shown on the home page after the engine executes them |

Reading the home page or details with `gear` does not close items. `plan apply --keep` must still link the plan version it is based on, the review item, and the next checkpoint, and does not close unlisted messages or items that arrived later.

There are also install/ops entry points (`gear admin init`, `gear admin stop`; `gear run` only for debugging) that Agents do not need to learn. The user mainly looks at the board, opened by a hotkey.

## 15. Visibility and human intervention

- **Board** — what the user and the captain look at. Opened and closed by a Herdr hotkey (plugin action, like the file viewer), never a pane that must stay open. It shows the **plan**, not agent liveness (Herdr's own agent panel shows working/idle):
  - totals: how many tasks/stories, how many done;
  - per lane: its tasks and their state (e.g. `gear-data`: T-1 done, T-2 in review, T-3 queued);
  - items waiting for the captain or the user;
  - temp slots in use / quota;
  - later: the task dependency graph.
  - A single warning line only when the engine is down or observation fails.
- Home page (`gear`): the role-specific view for agents.
- **Engine log**: `engine.log` next to the database, timestamped, verbose while GEAR is in development — the first place to look when something goes wrong.
- By default the user is sought only in these cases: out-of-authority requests, budget exhausted, captain unavailable, supervision anomalies, resource handovers needing manual checking, project acceptance. The user can step in on their own at any time.
- All decisions and operations have audit records.

## 16. Runtime form

- The engine is a **background process with no pane**: any `gear` call starts it when its lock is free; tabs, panes and clients do not affect it; `gear admin stop` stops it (upgrade, uninstall). If it dies, the next `gear` call restarts it and the recovery checks run. While it is down, `gear send` still writes to inboxes and doorbells follow after recovery. The user never runs or watches it. The one exception is `gear board`: it is read-only and does not start the engine, so it can show that the engine is down.
- A project is bound to an execution host and session; running code and configuration versions are queryable, and changes are switched explicitly; changing a file does not mean it is in effect.
- Initialization: the kickoff of §8a.

## 17. Invariants (enforced by the engine, not relying on Agents having read instructions)

1. A single authoritative state and queue source; one continuation controller per lane.
2. A write resource has one writer at any time; if uncertain, do not transfer.
3. Each lane has at most one delivery that needs check; if the result is unclear, do not resend.
4. Messages are acknowledged item by item; being fetched, read, or doorbell-delivered does not count as handled.
5. Unknown is not treated as success; observation failure is not treated as all-clear.
6. Old generations (captain, attempt) cannot overwrite new state.
7. Completion is bound to the contract and artifact versions; cancellation is not delivery.
8. Results are not lost: closing sessions, removing worktrees, and deleting branches are separate, and undelivered results are kept.

## 18. Not doing

Multiple supervisor layers; the core choosing technologies on its own or replacing Agents beyond its authority; Web/mobile; joint scheduling across multiple execution hosts (first version); rewriting Git/worktree/terminal management; built-in development methodology; binding to GitHub/BMAD; a general-purpose workflow engine; promising that any model can complete any task.

## 19. Acceptance scenarios

| ID | Scenario | Must be observed |
|---|---|---|
| AC-01 | A lane writes two lines and then ends its turn normally | Continues the same task with nobody nudging; or a clear reason why it cannot continue is formed |
| AC-02 | A prompt was submitted but no start was observed | No duplicate paste, no double start; recovers after checking |
| AC-03 | Rate limiting, quota exhaustion, waiting for authorization, manual pause | Each has its corresponding state and handling; no endless "continue" |
| AC-04 | An urgent bug is inserted midway | There is an actual responsible lane and a safe takeover point; the original task's ID, results, and resume condition are kept; a review confirms ordering and milestone impact |
| AC-05 | Ten action items, the captain handles only one | Only one is closed; the rest remain fetchable; different questions on the same task are not closed by mistake |
| AC-06 | A lane keeps spamming messages; urgent messages keep arriving | Old ordinary items still get a chance to be handled; urgent ones are not swallowed; prior context is available |
| AC-07 | A review comes due while messages keep arriving | The review is still handled at a turn boundary and produces a structured conclusion |
| AC-08 | `gear send` succeeds, fails, loses its receipt after commit, engine offline | Success means it is in the inbox with an ID; failure gives a reason on the spot; a lost receipt can be queried back by request_id; messages are not lost while the engine is offline |
| AC-09 | A trustworthy long test is running but there is no commit; there is a diff but it deviates from the contract | The former is not misjudged as stopped work; the latter is re-checked and produces a corrective action |
| AC-10 | A long-lived lane has no work | Quiet standby, no messages; appears in the review snapshot; park expiry automatically re-evaluates; new tasks are dispatched immediately |
| AC-11 | A temp worker is still writing files after delivery | Not closed early, no artifacts lost; quota is released automatically once conditions are met |
| AC-12 | Temp workers over quota | Explicitly queued; quota is not bypassed |
| AC-13 | 2 of 10 lanes retire | Dispatch stops; in-flight results are kept and handed over; the roster is accurate; the captain receives a merged item; capacity changes only after retirement completes |
| AC-14 | A lane's process exits, the pane closes | The original worktree is not automatically given to a newcomer; transferred after manual checking |
| AC-15 | Captain quota exhausted | Captain-side changes frozen, old generation invalidated and checked item by item; shown persistently and the user notified; re-onboarding/replacing the captain only initiated manually by the user, no backup activated automatically; the old session cannot exceed its authority |
| AC-16 | Normal captain brand change | Not by same name; handover packet complete; activated after the new captain confirms; decisions already in effect are not redone |
| AC-17 | Two projects in the same session, same task numbers | No cross-dispatch, no cross-reply; a flood in one project does not slow the other |
| AC-18 | Engine reopened, client disconnected, Herdr briefly unreachable | No double start, no duplicate nudges; unreachable shown as unknown, checked on reconnect |
| AC-19 | Contract update interleaved with a lane's late result | An old version cannot complete a new contract; notification delivered does not mean adopted |
| AC-20 | Candidate or acceptance conditions change | Old approvals need re-checking; cancellations do not count toward completion statistics |
| AC-21 | No development framework installed, only a goal and acceptance conditions given | Can complete the loop of dispatch, continuation, collaboration, verification, delivery, and reclamation |
| AC-22 | Different CLIs serve as captain/executor/reviewer | Role behavior is consistent; capability differences are shown explicitly |

**Metrics**: number of manual nudges, number of manual coordination interventions, number of unsupervised unfinished tasks, duplicate dispatches, mistaken reclamations, wrong unblocks, drift detection latency, urgent task start latency, captain message backlog and oldest wait. These must be judged together with false nudges, missed items, and wrong completions.

## 20. Change log

- v0.9 (2026-10-22): after the walking skeleton ran on a real Herdr. Added positioning (§0), kickoff and the no-popup rule (§8a, C-13), the quota policy (wait for reset / fall back), the parallel develop–review–merge loop (§10), the plan-centred hotkey board and the engine log (§15), the background engine (§16, C-11).

- v0.8 (2026-10-21): rewritten after 10+ rounds of Claude × Codex discussion. Added plan distortion and the living plan, keeping the captain from drowning (synchronous send + threaded inbox + doorbell), engine-scheduled reviews, watching output, long-lived lane standby and park, temp worker release on delivery, leaving without notice and quota exhaustion, multiple projects and identity generations, Herdr role verification, 4 daily commands; SQLite as the single authoritative state, CLI writes directly in transactions, engine owns Herdr side effects exclusively; trade-offs for Firstmate and other reference projects listed separately.
- v0.7 and earlier: see [archive/PRD-v0.7.md](archive/PRD-v0.7.md).
