# GEAR Product Requirements: Continuous Multi-Agent Collaboration on Herdr

Version: v0.7; status: requirements baseline, to be revised during implementation based on real runtime evidence.

Project name: GEAR (tentative); Herdr plugin ID: `herdr-gear`; proposed CLI: `gear`. Acronym: **G**oals **E**xecuted through **A**gent **R**elay (completing goals through an agent relay).

This document is the primary document for the current product requirements. The Firstmate, Herdr Board, and other selections in prior research are candidate implementations, not product constraints the user has confirmed. An implementation plan must not substitute the limitations of some library for the requirements here. Orca serves as a reference of mature practice: we borrow its mechanisms and the failures it has already handled, without requiring the user to migrate to Orca.

**1. Problem to solve and success criteria**

The user is a heavy Herdr user running multiple heterogeneous Agents and needs them to collaborate continuously and in an orderly way to complete tasks. Development methods and skills are chosen per project: BMAD, Superpowers, custom skills, or no development framework at all. The main usage mode is a local Herdr client connecting in remote mode to a remote Herdr, with the remote side running the project and multiple Agents. Today, project-specific scripts detect when an Agent stops its turn and nudge it, which suffers from work stopping midway, the queue drifting out of sync with actual execution, dependency blocking, message overload on the master controller, and having to rewrite the scripts for every project.

The product must guarantee: once a task is dispatched, a person or program is continuously responsible for driving it forward; normal turn stops do not require the user to nudge manually; when work cannot continue there is a clear reason, owner, and next step; the user can see the actual work and the upcoming queue at any time.

Success is not measured by "all Agents show working". An empty queue, waiting for authorization, or an explicit pause may stop quietly; continuous output does not prove the task is progressing either. The goal is to continuously complete authorized work, keep records trustworthy, and reduce manual nudging and repeated judgment by the lead.

**2. Confirmed constraints**

| ID | Requirement |
|---|---|
| C-01 | Continue to use Herdr as the runtime and terminal interaction environment; prefer reusing its pane/workspace, Agent control, communication, and worktree integration |
| C-02 | CLI/TUI first, supporting the remote usage mode where a local Herdr client connects to a remote Herdr; hiding/leaving the board view or disconnecting the client must not leave remote tasks unsupervised |
| C-03 | One lead manages executors directly, with no supervisor reporting layer; independent review is a task, not an added management layer |
| C-04 | The lead, executors, and reviewers are not bound to any Agent brand or model; Codex, Claude Code, OpenCode, Grok, and Pi are the first CLIs to be validated |
| C-05 | Git worktrees are the foundation of parallel development: parallel code writing uses separate worktrees by default; multiple writers are not put in the same project directory |
| C-06 | Orchestration is independent of development methods and skills; the core loop can be completed without installing BMAD, Superpowers, or other frameworks, and switching methods does not require changing the orchestration core |
| C-07 | Routine work advances automatically per approved rules; dependencies, authorization, re-decomposition, priority, exceptions, and project acceptance are held by the lead |
| C-08 | Notifications may be deduplicated, grouped, and summarized, but raw facts and evidence must not be lost; message volume is not reduced by hiding anomalies |
| C-09 | Do not rely on any Agent's self-discipline or a vendor-specific Stop hook to keep the whole system running; unknown state must be shown truthfully |
| C-10 | Put project differences in configuration, a generic task contract, and project-provided context, instead of copying and modifying a set of keeper scripts for every project |
| C-11 | The plugin adds only one resident orchestration process, which internally provides the board, queue, nudging, mailbox, and persistence; do not stitch together multiple plugin backends. Agents, build/test, and short-lived CLI request processes are not additional resident orchestration services |

**3. User flow**

On first start, a project completes the FR-13 initialization confirmation: determine the number of long-lived lanes, the temporary concurrency quota, the master controller and executor configuration, and the authorization scope, and then enter the execution flow below. When an existing project resumes, it reads the confirmed configuration and confirms only actual changes, without re-asking about already approved items.

1. The user describes the goal to the lead and may provide existing documents or specify a development skill; tasks can also be created directly without standard documents.
2. The lead decomposes the work, specifying scope, dependencies, acceptance method, priority, Agent choice, and authorization boundaries.
3. The system creates or confirms separate worktrees, prepares the runtime environment, and dispatches ready tasks.
4. Executors work; the program handles normal turn stops, message delivery, verification, queue continuation, and state recording.
5. Within their authorization boundaries, executors may laterally clarify interfaces, request artifacts, and verify deliveries; new dependencies or global conflicts go to the lead.
6. The lead handles must-decide items and proactively checks decomposition, resource utilization, and follow-up work.
7. A completion claim enters acceptance; when needed, an independent review is done in a fresh session; candidate code is considered delivered only after passing integration checks.
8. Task state in the orchestration system is updated and evidence retained. A long-lived lane picks up the next item or sleeps; temporary sessions are reclaimed in a controlled way.

**4. Core objects and states must be kept separate**

| Object | Fact it expresses |
|---|---|
| Project/goal | What the user wants, and what the constraints and final acceptance are |
| Task | A concrete piece of work; includes inputs, dependencies, scope, acceptance, and result |
| Execution attempt | One actual execution of a task by some Agent; retry/reassignment creates a new attempt or an explicit handoff generation |
| Work lane | A logical execution slot with a scope of responsibility, current task, follow-up queue, and long-lived/temporary attribute |
| Agent session | The actual runtime state, context, and process of some CLI; not equivalent to a task |
| Worktree/branch | Code write space, baseline, output, and resource ownership; not equivalent to a pane/workspace |
| Message/decision | Progress, collaboration requests, blockers, authorization, results, and handling receipts |

Tasks must distinguish at least: to be prepared, ready, executing, waiting on dependency/decision, awaiting verification/review, done, failed, cancelled. Agent state must distinguish at least: executing, can accept input, waiting for interaction, exited, unknown. Work lane paused/sleeping is a separate state.

"Agent done", "process exited", "CLI returned success", "workspace board moved to Done", "tests passed", and "project complete" cannot substitute for one another. Names in the implementation may differ, but these semantics must be preserved.

**5. Functional requirements**

**FR-01 Task decomposition and execution contract (P0)**

Every dispatchable task must have a stable ID, goal, scope, relevant files/modules, necessary context, dependencies, acceptance criteria, expected artifacts, authorization boundaries, and a responsible work lane. Task inputs are versioned; changes to goals or acceptance conditions must be traceable. Executors do not need to understand the whole project, but they must receive a sufficient local contract.

One goal or parent task may correspond to multiple execution/review tasks; external documents are referenced only as optional sources. A parent item is complete only when all required child items pass their respective acceptance; Agent completion replies alone must not be counted.

**FR-02 Worktree isolation and lifecycle (P0, foundational capability)**

- Parallel code-writing tasks each use a separate worktree/branch. Multiple terminals, tabs, or workspaces do not by themselves provide file isolation.
- Prefer Herdr's existing worktree create, list, open, and remove capabilities; do not build another terminal/workspace manager.
- Record the execution host, repository identity, actual worktree path, branch, base commit, current write attempt, and owner. Path aliases or different workspaces pointing to the same checkout are still treated as the same write resource.
- Before dispatch, confirm that the Agent's actual cwd matches the registered worktree; setup success/failure must be visible, and a task that requires setup must not silently start after setup failure.
- When a task switches Agents, the original worktree may be reused, but it must be confirmed that the old writer has ended and ownership handoff is complete. Read-only viewing may be shared, but a reviewer with executable tools must not be assumed to be truly read-only.
- Review must pin a candidate commit; reviews that may run tests, generate files, or patch code use a separate checkout/worktree. Changes made after review trigger the corresponding re-verification.
- Make explicit the distinction between independent baselines and dependent-branch baselines. How upstream changes enter downstream, and when to rebase/merge/cherry-pick, is decided by the established integration strategy; do not assume files from another worktree will appear automatically.
- Merging uses a controlled order, and the merged result is verified. Mechanical merge conflicts may be handled within authorized scope; semantic conflicts go to the lead.
- Closing an Agent session, removing a worktree, and deleting a branch are three different operations. Uncommitted changes, undelivered commits, and review evidence must be preserved; branches or directories must not be force-deleted just because the task window disappeared.
- Worktrees do not isolate ports, databases, external accounts, or shared caches; these are listed separately as resource dependencies.

**FR-03 Heterogeneous Agents and role switching (P0)**

Start, submit input, observe state, retrieve results, and stop/replace form a unified integration contract. All five initial CLIs must be validated in the lead, execution, and review roles; model/effort is specified by configuration, and the actually effective values are displayed.

A CLI without native resume may continue with a fresh session that reads the persistent task packet and existing output, but the interface must clearly state that it is a new session. Capabilities the adapter does not support are explicitly refused; it must not silently switch models, switch hosts, or degrade into a different behavior.

**FR-04 Task queue and dynamic dispatch (P0)**

Support a project backlog and an ordered queue per work lane; the user/lead can add, remove, pin to top, reorder, transfer not-yet-started tasks, and pause/resume. The next item must simultaneously satisfy authorization, dependency, capacity, and resource constraints.

An approved next item may be dispatched automatically; when there is no suitable work, notify the lead to arrange it, and do not invent new requirements. The lead should get a prompt when the queue is about to run out.

Reassignment while running must preserve progress and hand over write ownership; dragging a card must not silently let a second Agent take over a worktree that is still being written. Display order adjustments must match the real dispatch order, or be explicitly labeled as display-only ordering.

**FR-05 Preventing work from stalling midway with no one continuing (P0, core loop)**

- An external resident execution mechanism checks for "task incomplete and continuable" state; it does not require the model to generate all the way to project completion in one go.
- When an Agent stops its turn normally while unfinished, the system continues per the task contract and checks whether the new turn actually started.
- Input accepted, execution started, results submitted, and acceptance completed are recorded separately; one successful prompt call cannot substitute for subsequent evidence.
- When the outcome of a send is unclear, verify first; do not re-paste long instructions or create a second copy of the task.
- Test failures may be fed back to the executor for retry within the authorized fix scope; persistent lack of progress has a count/time/cost budget, and on reaching the boundary the output is preserved and escalated.
- When working but output is silent, check real tool/test activity; silence duration alone does not determine death; a still-alive terminal does not prove the Agent is working either.
- Permission prompts, authentication failures, rate limiting, real dependencies, manual pauses, and normal turn stops are handled differently; do not repeatedly send "continue" for every pause.
- When the lead itself stops its turn but there are still pending decisions, the system must also wake/resume it; when the lead cannot be resumed, show the unhandled items to the user.
- When local/remote state is unknown, keep the original execution ownership and verify; do not mask the unknown by launching again.

Observation sources, stability windows, timeouts, and recovery budgets are configured per CLI and work type. Concrete values are determined by measurement; a fixed polling interval cannot be taken as proof of reliability.

**FR-06 Task board and details (P0)**

Provide two terminal-usable views: a task stage view and a work lane/Agent view. It must be possible to see what is being done now, what comes next, which items are unfinished/finished/blocked, who is waiting on whom, the last evidenced progress, and whether the lead needs to act.

Task details show: acceptance conditions, dependencies, current attempt and history, Agent/model, host/worktree/branch, result and test evidence, unhandled messages, and next step. It can jump to the actual Herdr pane; even after the session is released, archived results remain readable.

Prioritize keyboard selection, entering details, sorting, reassignment, and pause/resume; no web service or desktop client is required. Overall progress is expressed by task and acceptance milestones; when it cannot be inferred, do not fabricate a percentage.

**FR-07 Reminders, reporting, and persistent inbox (P0)**

A message contains task/attempt association, sender, recipient, type, time, raw content/evidence, and handling status. Persistently recorded does not mean read, and read does not mean handled; unacknowledged items can be recovered after handling is interrupted.

Normal progress/completion is written to the board and summaries, without requiring the lead to reply item by item. New blockers, authorization, budget exhaustion, low queue watermark, supervision failure, and contradictory results get explicit reminders. Items with the same confirmed root cause may be merged, listing all affected tasks; unknown problems must not be silently classified as duplicates.

A negotiation/authorization question that times out remains the same pending question; do not create a string of duplicate questions. Notifications are a wake-up channel; the unhandled records in the database are the ongoing responsibility; notification failures must be visible and retried.

The master controller uses a project-level persistent mailbox, bound to the logical role rather than to a particular terminal or conversation; after replacing the master controller session, the same set of items can still be handled. The concrete handling contract is as follows:

- Persist to the inbox first, then send the wake-up notification. Each message has a stable ID; a notification or prompt is a reminder, not the only message store. The master controller pulls pending items via a structured CLI/API, not relying on the last chat segment in the terminal.
- Record informational events and items that need handling separately. Ordinary progress/completion events are retained, summarized, and update state per approved rules, without requiring the master controller to decide on each one; items that need a decision must not be auto-closed just because a summary was shown. When the system cannot classify reliably, keep the item pending.
- Pending items distinguish at least: awaiting claim, in progress, waiting on external result, handled. Claimed/read cannot be treated as handled; only after recording a decision, a reply, or an accepted operation receipt can items be acknowledged as done one by one. A batch may acknowledge only the IDs actually handled in it; it must not be bulk-cleared with "looked at this batch".
- Bounded batch pulls return the IDs in this batch, the remaining count, and the oldest wait time; items of equal priority are handled in inbox order, urgent items may go first, but there must be wait-aging/quota rules to prevent old items from starving forever. Concrete thresholds are configurable.
- Messages arriving during handling are first put into the inbox; do not stuff ten new prompts in a row into a master controller that is executing. Each master controller allows only one active handler, and the external program coalesces wake-ups; if there are still handleable items after the current turn ends, schedule the next batch. Urgent items use controlled reminders and must not by default interrupt a decision being carried out.
- An in-progress claim has recoverable ownership and a deadline; after the master controller exits or is replaced, verify before resuming, so that no message stays in progress forever. An old generation cannot acknowledge items of the new master controller. When an operation has executed but the acknowledgment was lost, reconcile via stable operation IDs and receipts; do not blindly redo it.
- Items waiting on a response from the user/upstream record the responsible party, the associated request, and the condition for re-checking, and the same question is not repeatedly fed in the meantime; other handleable items continue. Message handling completion and corresponding task completion are recorded separately.
- The board shows the counts of pending/in-progress/waiting-external items, the oldest wait time, overdue items, and recent handling progress. When the backlog keeps growing, critical items are overdue, or the master controller makes no handling progress for a sustained period, remind the user; the program must not eliminate the backlog by dropping messages or having another Agent decide on the master controller's behalf.

For example, ten executors simultaneously submit ten questions needing decisions, and the master controller handles only the tenth this turn: that item can be closed after its handling result is recorded, and the other nine remain in the mailbox and are delivered in subsequent batches. The acceptance requirement is not "ten notifications were sent", but that all ten items eventually have a traceable handling result or an explicit reason for waiting.

**FR-08 Must-handle items for the lead (P0)**

1. Resource and task dependencies: additions/changes, resource contention, circular waits, a depended-on party unable to deliver.
2. Authorization: scope/budget/permissions beyond the task; items requiring user approval must not be signed off by an Agent on the user's behalf.
3. Proactive evaluation of decomposition: inappropriate granularity, duplicated work, persistent rework, contract mismatch; re-split/merge/re-orchestrate.
4. New work and capacity: queue about to empty, work lanes idle, no suitable ready items; reallocation or preparation of new tasks is needed.
5. Goal/scope/priority: requirement changes, resource preemption, acceptance criteria changes, global architectural trade-offs.
6. Exceptions that cannot be recovered by rules: budget exhaustion, persistent non-convergence, contradictory evidence, and recovery failure.
7. Project-level acceptance: integration results, outstanding issues, whether the user's goal is met, and delivery/release decisions.

The lead may dispatch investigation or review tasks to obtain evidence; decision authority does not transfer with the investigation task. Proactive re-checks are triggered by milestones, requirement changes, accumulated blockers/rework, low queue watermark, and a configurable period; the lead must not only wait for executor reports.

**FR-09 Lateral collaboration among executors (P0)**

Executors may directly, with the owners of related tasks, clarify approved interfaces, request schemas/fixtures/test results, hand off artifacts, and fix inconsistencies within both parties' authorized scope. The program provides persistent delivery, wake-up, timeouts, and records, without requiring the lead to act as a messenger.

New dependency/resource needs, priority adjustments, changes to public contracts, impact on third parties, exceeding budget/permissions, and non-converging negotiation must go to the lead. Mutual agreement cannot replace acceptance, nor expand either party's permissions.

Messages are preferably sent along related task relationships, avoiding whole-team broadcast; the protocol records requests, responses, commitments, deliveries, and verification; a commitment is not completion. Whether lateral communication reduces blocking or increases message storms must be validated in real runs; do not assume Agents have a human team's tacit understanding.

**FR-10 Long-lived work lanes and temporary sessions (P0)**

A long-lived lane retains its responsibilities, queue, and necessary context, and may sleep when there is no work; support a specified pause duration, e.g. 20–30 minutes, re-evaluating on expiry, and make explicit whether a new task wakes it early.

After a temporary execution/review completes, choose to reuse, retain, or release it; there must be an explicit next owner. Independent review uses a fresh context and does not inherit the implementer's conversation. Releasing a temporary session is separate from reclaiming its output/worktree/branch; failed output should also be retained.

**FR-11 Replaceable task guidance and context (P0, minimal boundary)**

A task may carry document/artifact references, execution instructions, and an optional skill name or path. These are passed to the Agent as task input; core scheduling depends only on the generic task contract, dependencies, authorization, and acceptance conditions, and does not parse the phases, directories, or state files of any specific development framework. Whether a specific skill can be used is confirmed by the chosen Agent environment; when missing, report it explicitly and do not pretend it was loaded.

No built-in development methodology is provided, and configuring a process engine is not required. Projects may change task guidance; a change during a run records the input version and makes explicit whether re-verification is needed; switching skills must not be used to bypass authorization, isolation, or acceptance requirements.

The first version does not develop BMAD/Superpowers-specific importers, version adapters, phase mapping, bidirectional state sync, or write-back to shared sprint files. These may be extended independently in the future if needed, but must not become runtime dependencies of core orchestration. Independent review remains a generic task capability, and the task's acceptance requirements decide when to use it.

**FR-12 Interruption recovery and remote operation (P0)**

The board and scheduler coexist in a single process: leaving/hiding the view does not exit the run loop. Actually closing the only tab hosting the plugin terminates the process, and must be distinguished from hiding the board; reopening the entry point restores persistent state. Without an external observer, we cannot promise immediate notification or automatic revival after the plugin exits unexpectedly.

The local client disconnecting from the remote Herdr, hiding/leaving the board view, explicitly terminating the plugin process, the lead stopping its turn, a worker exiting, and the runtime service restarting are distinct events. After restart, reconcile persistent records against actual processes/worktrees, recover unhandled messages and tasks, and do not create duplicate executors.

The remote host owns the actual process and file state; loss of contact shows as unknown, which is not the same as exited. The same pane ID/path on different hosts must not be confused. When a required capability does not exist, report an error explicitly; do not fall back to local execution.

Phase one must support a local Herdr client operating a remote Herdr in remote mode: the user views and interacts locally, while project files, worktrees, Agent sessions, and this project's continuous execution service and persistent state reside on the target execution side. The user does not need to separately log into the server to start an independent interaction flow; disconnecting the local client must not terminate remote supervision. Local Herdr execution is also supported. Joint scheduling across multiple execution sides may be extended later; this does not affect first-version support for Herdr remote.

A project is bound to an explicit Herdr execution side and session; initialization discovery, task dispatch, state queries, messages, and worktree operations must always be routed to that binding. The remote machine selected in the interface must not be assumed to be the implicit target of tool commands: a CLI inside a Herdr pane usually inherits the session context, and locally forwarded calls must explicitly select the corresponding machine. Switching the interface connection does not migrate the execution location of existing projects. Display names and pane IDs alone cannot serve as resource identity across execution sides.

The implementation must distinguish Herdr's interactive remote connection from machine API forwarding; only capabilities actually supported by the target version may be used. If there is no existing query capability for detecting CLI installation/availability on the execution side, a read-only probe must be added on that execution side; a local probe must not be dressed up as a remote result. When the remote side lacks the plugin or orchestration components, report it explicitly; do not assume a local install is automatically copied to the remote side; the concrete deployment and how the remote plugin is displayed still need validation.

**FR-13 Project initialization and capacity confirmation (P0)**

On first start, the system lets the lead present and confirm a project runtime configuration with the user through conversation or CLI/TUI. The lead may suggest a configuration based on the initial goal, but must not treat the suggestion as already approved by the user. A configuration or template the user has already given may be adopted directly, to avoid repeated questions.

Initialization is a conversational flow that can go back and forth, not requiring the user to fill out a configuration form first:

1. **Discover available Agents.** First identify and show the Herdr execution side the project is bound to (Local or a remote machine/session), query Herdr's Agent support information on that execution side, and check installed CLIs and existing configuration profiles. List for the user the discoverable types, availability, and not-yet-validated capabilities; clearly distinguish "supported by Herdr", "installed on the target execution side", and "validated to start/integrate". Not detected does not mean definitely absent; allow the user to supply custom paths/configuration. When the user connects to a remote side through local Herdr remote, discovery and subsequent launches all target the same remote Herdr; the user is not required to switch to a separate SSH interaction, and the client's installation state must not be used as a substitute. Basic discovery does only the necessary read-only checks; validation that requires starting sessions or model calls should be performed within authorized scope.
2. **Propose an adjustable plan.** Combining the project goal, task scale, user preferences, and available Agents, the lead gives a concrete team draft: master controller configuration, long-lived lane count/responsibilities/type mix, temporary quota/purpose/type rules, and a brief rationale for the mix. Unknown parameters are marked with defaults or as to-be-confirmed; do not ask repeated questions just to complete the configuration, and do not assume a tool suits all roles just because it is installed.
3. **Modify through conversation.** The user may say directly "switch all development to Pi", "keep one Codex", "use OpenCode for temporary review", "start two fewer for now". The system updates the same draft and shows the changes and the final mix; adjusting count, type, or responsibilities does not require re-initializing from scratch. At this stage the whole team is not launched, and resources are not repeatedly created on each revision.
4. **Confirm and land.** After the user confirms the current version of the plan, save it as the project runtime configuration, prepare worktrees per the configuration, start the long-lived lanes that need to work immediately and register the rest; the temporary quota is registered as an on-demand launch rule. Show the actual landed result, items not started, and failure reasons. A complete plan the user has already explicitly approved can be landed directly without mechanically asking again. If the environment changes during landing, keep the successful parts and handle the failed items; do not switch Agents or expand quota on your own.

The CLI/TUI may show discovery results and the current draft; conversation is responsible for proposing and modifying the plan, and the program for validating and executing it. The draft has a stable identity and version and is recoverable; confirmation applies only to the corresponding version, and post-confirmation changes must not be mixed into previously approved launch operations. When initialization exits, retries, or is re-entered, it continues the existing draft/configuration and does not create the team again.

| Item to confirm | What must be made explicit |
|---|---|
| Project and environment | Herdr execution side and session, repository, execution host, working directory, baseline, worktree/setup and output integration strategy |
| Master controller | Agent configuration serving as the lead, recovery method; brand not fixed |
| Long-lived development lanes | Initial count, each lane's responsibilities and Agent configuration, the range allowed for automatic scaling up/down; sleep policy when idle |
| Temporary support | Maximum number of temporary Agents allowed to run concurrently, purposes (e.g. independent review/investigation), selectable Agent configurations, reclamation rules |
| Total resources | Total concurrency limit and whether it includes the master controller, separate long-lived/temporary quotas and reserved capacity, necessary cost and time budgets |
| Automation authorization | Which continuations, dispatches, retries, scaling, and integration actions are approved, and which require the master controller or user to decide |

Initialization must confirm "count + Agent type mix", not just ask how many lanes to open. Agent type here means an execution CLI such as Pi, Codex, or OpenCode; responsibilities (development/review/investigation), model, and development skill are configured separately and must not be mashed into one fixed role template.

- First confirm this project's default Agent type for lanes, then confirm whether each long-lived lane overrides the default. Support all-same-type as well as mixed combinations, e.g. four Pi plus one Codex; executor type must not be inferred from the master controller's type.
- Resolve and show the final configuration for each lane: lane ID/responsibility, Agent CLI, model and applicable runtime parameters, optional skill. The user may reference an existing configuration profile or accept CLI defaults without re-entering all parameters; the source of defaults must be explicit, and the actually effective values must be verifiable after launch.
- Temporary support separately confirms its default type, per-purpose selection rules, and the range of configurations allowed. For example, development lanes mainly use Pi, while independent review may be configured with another Agent or a fresh session of the same type; independence is not forced to mean switching brands. Mixed temporary types share the confirmed total temporary quota, with per-type caps set if needed.
- Configuration is project-scoped. Templates may be reused, but when a new project adopts a template, show and confirm this run's mix; a preference for mainly Pi in the last project must not be silently carried into a project that mainly uses Codex or OpenCode.
- Before launch, show the final orchestration manifest, including the master controller, each long-lived lane, temporary slots, and type selection rules. Combinations the user has already specified go directly into the manifest, asking only about missing items; unspecified types must not be secretly decided by the system.
- When a specified CLI is unavailable, a model is unavailable, or integration capability is insufficient, explicitly block that configuration. Switching per rules is allowed only when the user has pre-approved a substitution range, and the actual substitution is recorded; otherwise request confirmation. New lanes inherit this project's approved default configuration; type changes beyond the approved range require confirmation; changing the type for an in-flight task still requires a safe handoff.

For example, initialization might be confirmed as: "the master controller keeps its current configuration; four long-lived development lanes, three Pi and one Codex; temporary concurrency cap of two, review uses OpenCode, investigation keeps Pi." This is an example of an optional combination, not a product default. The final object of confirmation is the complete execution team configuration, not just a total Agent count.

The temporary count expresses an on-demand concurrency quota; it does not require launching idle review Agents at kickoff, nor does it mean only that many may be created over the whole project. The long-lived lane count expresses continuously retained responsibilities and queues; it does not require the model to keep generating when there is no work. If independent review is set as a completion condition while the temporary quota is zero, initialization must make explicit which suitable work lane's fresh session will take it on, or adjust capacity; review must not be secretly skipped.

The confirmed configuration is versioned and shows the planned count, actually launched count, idle/queued count, and launch failure reasons. Before launch, check the selected CLIs, repository/worktree capabilities, and necessary environment; partial failure must not be disguised as all ready. Repeated initialization/recovery must not create duplicate work lanes or Agents.

The lead may adjust work lanes and the temporary quota within the approved range; beyond that range, it goes to the user for confirmation. Scaling down first stops taking new tasks, and releases after in-flight work is completed or handed off; sessions with unsaved output must not be killed directly. When capacity is insufficient, queue explicitly; do not bypass the limit by spinning up additional temporary Agents.

**FR-14 Master controller's project state entry point and adjustment loop (P0)**

The TUI for the user and the structured CLI/API for the master controller must read the same authoritative task records. The master controller must not be required to "look at the screen and guess the global picture", nor rely solely on its memory of chat history. State queries provide at least:

- Project goal, current plan version, task stage distribution, items awaiting acceptance, and integration results.
- For each long-lived/temporary lane: current task, next-item queue, Agent state, worktree, capacity usage, and last verifiable progress.
- Dependency relationships, blocking reasons, wait durations, idle lanes, lack of work, or resource contention.
- Mailbox unhandled count, oldest item, urgent decisions, supervision/communication anomalies.

Each snapshot is labeled with its version, capture time, and the freshness of each observation source; unknown/stale state is explicitly marked. Support reading a full snapshot, as well as the changes since the last version; when change records are missing, require re-fetching the full snapshot. When the master controller starts, resumes, or switches sessions, it first fetches the snapshot and unhandled items.

Key state changes trigger coalesced reminders via persistent events; in addition, a global re-check is triggered on a configured period, so that things are not left unchecked just because no one reported. While the master controller is busy, changes accumulate and are pulled at the next checkpoint; "queryable at any time" does not mean the model is continuously thinking or can be interrupted arbitrarily and instantly.

Through the same control entry point, the master controller can add/remove/split tasks, change dependencies, reorder queues, reassign, pause/resume, and request scaling up/down. Change requests carry the plan/task version they are based on; before landing, permissions, resources, and current state are validated; version conflicts return the diff for re-judgment, and a stale snapshot must not overwrite new results. Running tasks must be operated on per handoff rules.

Each adjustment returns applied, waiting for safe handoff, or rejected with reason, associated with an operation ID; both the board and the master controller can verify the actual result. Issuing a reassignment instruction must not be treated as having reassigned. This entry point executes the master controller's decisions and established rules; it does not add an Agent that makes decisions on the master controller's behalf.

**6. Concrete lessons borrowed from Orca**

This round reviewed Orca's orchestration guide, key source code, and corresponding test assertions; see [Orca capabilities and failure lessons](research/2026-10-18/orca-lessons.md). Orca was not installed or run, and reading the tests is not claimed as the tests having passed.

| Design confirmed in Orca | Product requirement we adopt |
|---|---|
| Task separated from Dispatch; execution results bound to the current attempt | FR-01/03/05: tasks are not lost when sessions change; old results cannot overwrite a new attempt |
| ready queries and dependency completion conditions | FR-04: only dispatch truly ready work; a failed dependency must not mistakenly unlock |
| Agent status board and workspace board are different projections | FR-06: distinguish display state, Agent state, and task acceptance state |
| Persistent delivery, Delivery replay, and ack | FR-07: received does not mean handled; interrupted consumption must not lose messages |
| Prompt sent but start not observed: do not re-paste | FR-05: uncertain must be verified; do not run two executors |
| After takeover, the old consumer's authority is revoked | FR-03/12: an old session cannot acknowledge the new session's mail or settle a new task |
| Launch failure counting and circuit breaking | FR-05: persistent launch failures do not create terminals endlessly; business failures and launch failures are counted separately |
| Remote live/unverifiable/exited distinction | FR-12: disconnection cannot be taken as proof that something can be reclaimed/retried |
| release, retain, reuse, and stop separated | FR-10: resources have an owner after completion; cancellation and reclamation semantics are separate |
| Unmerged branches retained after removing a worktree | FR-02: directory reclamation does not mean discarding code output |

Two points are not copied mechanically: Orca's guide allows using the existing workspace by default, whereas this product, per the user's requirement, uses **separate worktrees by default for parallel write tasks**; Orca's successful worker_done advances the task record, whereas this product additionally requires a match with acceptance/review evidence and does not trust a success claim alone.

Orca's coordination loop still includes the coordinator actively waiting, checking, and choosing recovery; its keepalive output is not by itself automatic nudging of Agents. This round's evidence cannot support the conclusion that "installing Orca guarantees any Agent will keep working to the end". We need to validate and fill in our own continuous execution goals.

**7. First usable version and out-of-scope items for now**

The product provides only the six groups of capabilities needed for multi-Agent orchestration and collaboration:

1. Generic tasks and queues: record goals, local context, dependencies, acceptance; support dynamic adjustment.
2. Execution environment and isolation: call Herdr; manage heterogeneous sessions and worktree write ownership.
3. Continuous execution and recovery: continuation after turn stops, bounded retry on failure, verification of unknown state, master controller self-supervision.
4. Collaboration and decisions: lateral messages, artifact handoff, escalation of blockers and authorization, reducing ineffective reporting.
5. State and evidence: terminal board, current/upcoming work, acceptance results, and traceable records.
6. Lifecycle: long-lived lane sleep and refill, temporary task reclamation, restart recovery, and output retention.

How requirements are written, how design is done, and what development steps are used are decided by the Agent guidance or skill the user chooses; these are not functions the orchestration system must build in.

The first version delivers the basic loop of FR-01 through FR-14: single project, one lead, multiple heterogeneous executors, separate worktrees, persistent tasks/messages, CLI/TUI, automatic continuation after turn stops, exception escalation, queue adjustment, independent review, and recovery. First prove the mechanisms at small scale, then validate ten lanes; a pretty interface first is no substitute for execution reliability.

Not required for now: multi-level supervisors, automatic model selection, a mandatory Jev judgment layer, web/mobile clients, unified resource scheduling across multiple hosts, replacing Herdr, reimplementing Git/worktree internals, integration with specific development frameworks or a generic workflow engine, or promising that any model can complete any task.

How plugins, skills, and background services are combined, and whether to extend Herdr Board, belong to the next phase of implementation design. Only capabilities that satisfy the product contract and have been actually validated are adopted.

**8. Acceptance scenarios**

| ID | Scenario | Result that must be observed |
|---|---|---|
| AC-01 | Two Agents simultaneously modify a same-named file in the same repository | Actual cwd is in two separate worktrees; changes do not overwrite each other; subsequent integration is verified independently |
| AC-02 | Different workspaces point to the same checkout | The system recognizes the same write resource and rejects two parallel writers |
| AC-03 | A worker writes two lines and then ends a turn | The task is still incomplete; it continues without the user nudging, or a clear reason it cannot continue is produced |
| AC-04 | Prompt accepted but response/start observation is lost | No re-paste, no duplicate task/worker creation; recover after verification |
| AC-05 | An Agent keeps producing output but tests keep failing | Not treated as valid completion; escalate after bounded fixing, with evidence retained |
| AC-06 | A long test has no terminal output | Judged by actual activity; active tools are not mistakenly killed |
| AC-07 | Permission/authentication/rate-limit blocking | Targeted state and reminders; no endless "continue" |
| AC-08 | Upstream completes/fails/is cancelled | Downstream is unlocked only when dependency conditions are met; failure is not disguised as completion |
| AC-09 | Executors negotiate artifacts laterally | Requests, responses, and deliveries are traceable; out-of-bounds conflicts go to the lead |
| AC-10 | Change queue order, pause/resume, reassign | The actual next execution follows the new arrangement; no double holding of write ownership while running |
| AC-11 | The lead stops its turn or exits | Pending decisions are not lost; the system wakes/resumes it, and reminds the user on failure |
| AC-12 | Notification lost; crash after handling but before ack | Unacknowledged messages can be recovered/replayed; completed actions do not take effect twice |
| AC-13 | Local Herdr remote client disconnects or the remote service is briefly unreachable | Client disconnect does not stop remote supervision; unreachable state shows as unknown, verified on reconnect; no double launch or reclamation due to lost contact |
| AC-14 | A late reply from the old session after the worker is replaced | The old generation has no authority to change the new attempt's result or acknowledge its messages |
| AC-15 | Temporary review completes and the session is released | Report, candidate commit, and verification results remain queryable; unlanded output is not deleted |
| AC-16 | A worktree has dirty files or unmerged commits | Cleanup explicitly retains/refuses and cannot force-delete; terminal release is separate from directory/branch handling |
| AC-17 | Worktree setup fails or the baseline drifts | Work does not start silently; the failure reason and available recovery actions are shown |
| AC-18 | Long-lived lane queue is empty, a pause expires, a new task arrives | No meaningless model turns; re-woken per configuration |
| AC-19 | Ten lanes with a mix of completion, blocking, and failure | Board matches real state; ordinary events are summarized; must-decide items are fully visible |
| AC-20 | Swap different CLIs into the lead/execution/review roles | Supported role behavior is consistent; capability differences are explicitly presented; no fixed brand |
| AC-21 | All subtasks show complete but integration fails | The project is still incomplete; an integration fix/decision item is produced |
| AC-22 | The same task requests a retried submission, or requirements are revised midway | No duplicate dispatch; versions and re-acceptance requirements are traceable |
| AC-23 | No development framework installed; only goals and acceptance conditions provided | The complete loop of dispatch, continuation, collaboration, verification, delivery, and reclamation can be completed |
| AC-24 | Different tasks use different available skills, or the project changes development method | No change to the orchestration core is needed; existing queues, dependencies, output, and history are retained; input changes are traceable |
| AC-25 | First confirm three long-lived lanes and a temporary concurrency cap of two, then re-run initialization | Three long-lived lanes are created/registered; temporary tasks launch on demand with no more than two concurrently; re-initialization does not double-launch |
| AC-26 | Temporary tasks exceed the quota, or scale-down during a run | Over-quota tasks are explicitly queued; scale-down releases after in-flight work completes or is handed off, without losing output; unauthorized scale-up requires confirmation |
| AC-27 | Master controller resumes or switches sessions, and project state has changed | Recovers from snapshot and mailbox; queues, dependencies, capacity, and unknown states are visible; no reliance on old chat |
| AC-28 | Master controller adjusts a task based on a stale snapshot, while that task has just completed/been reassigned | Returns a version conflict and the new facts; does not overwrite the completion result or produce double writes; legitimate adjustments get an actually-applied receipt |
| AC-29 | Ten Agents simultaneously submit ten items needing decisions, and the master controller handles only one | Only the handled item is closed; the other nine remain pending and can be fetched in later batches, not overwritten by the last message |
| AC-30 | Master controller exits midway through handling a batch, or after an action completes but before acknowledgment | Unhandled items become claimable again; applied operations are acknowledged after verifying receipts, with no duplicate reassignment/scaling; the old master controller cannot acknowledge beyond its authority |
| AC-31 | Master controller receives messages continuously while busy, and one item is waiting for a user reply | Prompts are not force-fed one by one; the waiting item keeps its responsible party and wake condition; other items continue; old items are not permanently crowded out by new messages |
| AC-32 | No new reports but the queue is about to empty, or the mailbox has a long-standing backlog | Periodic re-check/low-watermark reminder triggers a global evaluation; backlog and oldest wait are visible; overdue items remind the user; no silent stall |
| AC-33 | Project confirms three Pi and one Codex long-lived lanes, with OpenCode for temporary review | The initialization manifest lists type and configuration per lane; launches per the confirmed mix; temporary tasks follow purpose rules and the total quota, not changing with the master controller's type |
| AC-34 | A new project switches to a different default type, or a specified CLI is unavailable | Does not inherit other projects' implicit preferences; explicitly blocks when unavailable, no substitution without authorization; authorized substitutions are recorded |
| AC-35 | Local Herdr connects to a remote via remote, and the two sides have different Agents installed | Within the existing Herdr usage flow, shows discovery results for the target remote and dispatches to the same side; distinguishes supported/installed/validated status; does not misuse local installs or an inherited other session |
| AC-36 | The user adjusts count and type repeatedly, then confirms, and retries after partial launch failure | Modifies the same versioned draft; the team is not launched before confirmation; after confirmation lands per the final plan; retries do not re-create successful items and do not silently substitute failed types |
| AC-37 | The interface switches machines, both sides have the same pane ID; the remote lacks a required capability | Project operations still use the bound execution side and session, not misdelivered due to the interface switch; missing capability is explicitly reported, with no fallback to local |
| AC-38 | Repeatedly open the plugin, stay in details, hide the view, and simulate slow task calls | Only one resident plugin process; view and scheduler do not block each other; Agents/short-lived commands are listed separately; a hidden child daemon cannot be used to satisfy acceptance |

Metrics: number of manual "continue" nudges, number of unsupervised incomplete tasks, number of duplicate dispatches, number of erroneous reclamations, number of erroneous dependency unlocks, recovery latency, number of repeated decisions by the lead, total tokens/time per unit of delivery. Target values and scenario baselines are determined after small-scale validation; obvious invariants such as no double writes, no lost output, and never treating unknown as success must always hold.

**9. Items that must be settled before entering implementation discussion**

| Question | Current requirement/boundary |
|---|---|
| Which capabilities reuse Herdr directly | First check the actual CLI/protocol on the target server; do not rebuild worktree, pane, or Agent control |
| Which task/board code to adopt | Herdr Board is a candidate, and Orca is a design reference already reviewed; confirm after an interface and cost comparison against this PRD |
| How to obtain Agent state evidence | Validate native events, Herdr state, process, and artifact signals per CLI; do not assume a unified Stop hook |
| What counts as task completion | Each contract specifies tests/artifacts/review; project completion additionally requires integration and goal acceptance |
| Who maintains exclusive write ownership of a worktree | Specify a single lifecycle owner; multiple backends must not each create and clean up |
| Merge strategy and branch baselines | Confirm per project configuration; do not unilaterally change the existing way of working into fixed PRs or fixed direct merges |
| Continuation, retry, and escalation budgets | Set separately for launch failures, no progress, business fixes, and notification retries; calibrate by measurement |
| Message and model cost | First handle clear events with rules; avoid whole-team broadcast, repeated long context, and multi-level reporting |

The above are implementation design topics and do not prevent this requirements document from serving first as a shared baseline. When implementation finds a mechanism does not hold, add the actual reproduction, the affected requirement IDs, the proposed change, and the acceptance changes, then update the document; do not change only the implementation while keeping a product promise that does not hold.

**10. Documents and evidence**

- [Herdr plugin mechanism review](research/2026-10-18/plugin-feasibility.md): native interfaces, existing plugin precedents, and MVP lifecycle boundaries.

- [Minimal implementation technical plan](TECH-MVP.md): first runnable slice, implementation choices, and three-step on-machine acceptance.

- [Orca implementation and failure lessons](research/2026-10-18/orca-lessons.md): pinned source, concrete capabilities, test assertions, and which lessons are adopted/not copied.
- [Prior technical candidate research](research/2026-10-18/decision.md): for implementation discussion only; does not override this PRD.
- [Existing validation results](research/2026-10-18/validation.json): notes passed, failed, aborted, and unvalidated scope.
- v0.7: confirmed a single resident orchestration process, with board and nudging in the same process; Board/Dispatch are reference only; clarified the difference between hiding the view and terminating the plugin tab.
- v0.6: corrected the remote usage model, making a local Herdr remote client connecting to a remote Herdr the first-version scenario; added execution side/session binding, explicit routing, and capability boundaries on both sides.
- v0.5: clarified the four-step initialization flow of discovery, proposal, conversational modification, and confirm-and-land; distinguished execution host from client; added acceptance for draft recovery and partial launch failure.
- v0.4: refined FR-13 with project default Agent type, per-lane overrides, temporary purpose selection, final configuration confirmation, and prohibition of silent substitution; added type-mix acceptance.
- v0.3: added initialization capacity confirmation (FR-13) and the master controller state and adjustment entry point (FR-14); refined the mailbox per-item handling and recovery contract (FR-07); added eight acceptance scenarios.
- v0.2: removed the hard binding to BMAD and the dedicated integration work; clarified six groups of generic orchestration capabilities, treating development skills as replaceable task guidance; added acceptance scenarios for framework-free runs and switching skills.
- v0.1: worktrees as a P0 foundation; added the Orca reference; clarified continuous execution and acceptance requirements; separated technology selection from product requirements.
