# Herdr Any-Agent Orchestration and Continuous Execution: Candidate Technical Approach

**Status: historical candidate approach. The first implementation slice adopts the [minimal landing technical approach](../../TECH-MVP.md) and does not fork Herdr Board for now; this document is kept as a reference for later extension. Product requirements are governed by the [PRD](../../PRD.md); supplementary investigation is in [Orca implementation and failure lessons](orca-lessons.md). The earlier conclusion that directly selected Herdr Board was premature. What follows is kept as an extension route that can be evaluated; it does not mean the comparison against Orca's design in terms of interfaces, runtime dependencies, and modification cost has been completed.**

The candidate route is to extend Herdr Board 0.19.1, adding a role-agnostic continuous-execution controller, a persistent decision inbox, lane and dependency scheduling, and generic task context passing. The settled organizational requirement is: one lead directly manages the executors, with no supervisor layer added; the lead, executors, and independent review tasks can each be configured as any Agent CLI that has passed the unified integration acceptance.

This is a code-reuse-and-extension route, not "install the existing Herdr Board and the requirements are met". The continuous-execution controller is substantive development work and cannot be packaged as a skill or a few lines of glue. That implementation has not been completed, and it has not been proven that a real ten-lane scenario passes. The earlier recommendation of fixed Codex/Claude roles has been withdrawn; historical source observations and tests are kept in [prior-research.md](prior-research.md), and versions and experiments are in [sources.lock.json](sources.lock.json) and [validation.json](validation.json) respectively.

**1. Non-negotiable requirements**

1. Agent brand, CLI, model, and organizational role are fully separated; any integrated Agent can serve as lead, executor, or reviewer.
2. The end of a generation turn does not leave unfinished tasks sitting there; when the lead itself stops, it is governed by the same mechanism.
3. No supervisor layer is added. Questions requiring judgment go directly to the lead; routine continuation, message redelivery, verification, and queuing are done by the program under authorized rules.
4. Tasks, lanes, and run records are independent of panes and model conversations; switching Agents loses no queue, artifacts, decisions, or acceptance criteria.
5. There is a terminal board, task details, lane queues, precise ordering and reassignment; closing the TUI does not stop background execution.
6. Long-lived duties may keep their sessions; temporary reviews use new sessions, release resources when done, and keep their reports.
7. Development methods and skills are replaceable; core orchestration requires installing no development framework; it runs within the existing Herdr and SSH-server way of working.

**2. Why replace the Firstmate base**

Firstmate is a third-party Agent orchestration distribution: prompts, skills, scripts, task records, and a set of companion tools. It already implements many supervision and recovery flows, but background supervision is deeply coupled to the primary agent CLI's hooks/extensions. The pinned version's supervision-host states explicitly: independent supervision in attended mode applies to Claude/Cursor; Codex and others are pass-through, and Pi uses its own in-process branch.

This does not mean it only allows Claude as supervisor. It supports multiple supervisors, but the supervision behavior is not equivalent; fixing on Claude earlier in order to get a more complete path directly contradicted the user's requirement. Leveling out that difference would require deep changes to its supervision architecture, while it also brings in a large toolchain and delivery conventions. By comparison, Herdr Board already places the database, dispatch, timing, concurrency, and run recovery in an independent daemon, which is better suited to extension as an external controller shared by all roles.

[Supervision scope of the pinned Firstmate version](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/docs/supervision-host.md)

**3. Why Herdr Board is listed as a candidate, and why it is not an out-of-the-box solution**

The candidate version inspected this time is `nelsonPires5/herdr-board@7a215a179acb4e098dd1c2971332497806309360`, Cargo workspace version 0.19.1.

| Capability | Existing implementation | Handling in this approach |
|---|---|---|
| Terminal board, cards, details, run history, column and card ordering | board-tui, board-core | Reuse; add role and lane views |
| Persistent data and migrations | SQLite, board-core/db | Reuse the same database and writer; do not layer tasks-axi on top |
| Launch, queuing, concurrency limits, session/workspace routing | board-daemon/dispatch, spawner | Reuse; add lane and actual worktree mutual exclusion |
| Run lifecycle, results, and stale-run checks | the three transactional units enqueue/promote/finalize | Reuse; add submission acceptance and recovery actions |
| Herdr event subscription, disconnect and restart reconciliation | watchers, supervisor, recovery | Reuse the observation mechanism; business recovery policy needs extension |
| Agent configuration and model parameters | harness adapters | Keep verified adapters; add a unified capability interface and integrations such as Grok |
| Automatic continuation of unfinished tasks | Currently idle/done enters awaiting | Must be added |
| Any Agent responsible for management decisions | No complete role layer | Must be added; the lead is also a supervised role session |
| Task dependencies, long-lived lanes, continuously adjustable queues | Currently mainly a run queue serialized per space | Must be added/modified |
| Completion verification, independent review, task artifact linkage | Cannot be replaced by board done self-reporting | Must be added |

[Base design](https://github.com/nelsonPires5/herdr-board/blob/7a215a179acb4e098dd1c2971332497806309360/docs/design.md), [configuration and adapters](https://github.com/nelsonPires5/herdr-board/blob/7a215a179acb4e098dd1c2971332497806309360/docs/configuration.md)

Herdr Board was previously excluded because it lacks complete continuous supervision; that gap still exists. It is listed as a candidate because the architectural boundary of an external daemon better fits the hard requirement of "any role, any Agent", not because it has suddenly been deemed to have automatic continuation.

The other candidates have these known limitations: bmad-loop's max_parallel is currently forced to 1; Herdr Dispatch has gaps in doing/done coverage and retry budgets and is provider-specialized; Firstmate's independent supervision depends on the primary agent CLI; Captain’s Deck depends deeply on Firstmate's data model. Their experience can be referenced, but four independent state stores and control loops cannot be stacked on the same task.

**4. Runtime structure**

```text
User / terminal board / management CLI
             │
      Independent resident controller
      ├─ Database of tasks, queues, dependencies, acceptance, and messages
      ├─ Observe, dispatch, continue, verify, recover, alert
      ├─ Lead role       ── any Agent configuration
      ├─ Executor role   ── any Agent configuration
      └─ Reviewer role   ── any Agent configuration, new session
                       │
                 Native Herdr panes
```

The program handles deterministic actions: the next ready task, recovery timing, retry counts, message deduplication, verification commands, persistence, and resource ownership. Models handle actions requiring judgment: requirement decomposition, dependency restructuring, interface decisions, repair strategy, and semantic judgment during acceptance.

The lead is not the only process keeping all loops running. It runs normally while handling decisions; after it ends a turn, the program checks whether unhandled decisions remain; if so it wakes the lead again, otherwise it waits quietly. The program itself does not invoke its own shell to masquerade as an Agent that is thinking.

**5. Integration contract for any Agent**

"Any" means roles have no CLI allowlist; each CLI's launch/status/input methods still require a verifiable adapter implementation. A CLI cannot be denied the ability to serve as lead just because it has no Stop hook; external observation, result submission, and recovery mechanisms make up for it. Nor can support for arbitrary argv be used to claim that any CLI can already be interacted with reliably.

Unified adapter surface:

- Start a new session, passing the working directory and context bundle.
- Submit input, distinguishing accepted, started, uncertain.
- Observe working, ready, blocked, exited, unknown, keeping the evidence source.
- Collect task output and result records.
- Stop/replace a session, confirming the old execution right has ended.
- Optional native resume; when unsupported, explicitly use a new session and continue from task records and artifacts.

The existing Board's built-in managed adapters are Pi, Claude, Codex, OpenCode, Antigravity; custom harnesses are currently an unmanaged path of argv + BOARD_PROMPT. **Full interactive integration of Grok must be listed as a development and acceptance item; the existence of a configured argv must not be used to mask this.** Later new CLIs can likewise take on all roles after passing the integration tests.

Role binding uses role → AgentProfile → adapter, and a profile snapshot is stored at runtime. Changing the lead does not change task policy: first confirm the old holder has stopped, increment the assignment generation, then hand over the current plan, unhandled decisions, and artifact index. Late results from the old session can only enter history; they cannot advance the new round's state.

Equal model capability is not promised. Configuration is equally free, and whether a task is done well is proven by acceptance; a weaker model's capability problem must not be miswritten as a requirement that some role use a particular brand.

**6. Continuous progress policy**

Task state and session state are separate. Session ready/done cannot be the basis for task completed.

| Observed fact | Controller action |
|---|---|
| Authorized task unfinished, Agent stably ready, no pending permission prompt | Send the remaining goal and the latest check results; confirm execution started |
| Agent has a completion claim | Enter verification; check acceptance commands/artifacts; dispatch an independent review when necessary |
| Verification failed and falls within authorized repair scope | Return the failure evidence to the executor, counted in a bounded repair loop |
| Submission result unconfirmed, transport timeout | Query the run and receipt; keep uncertain; must not unconditionally re-dispatch |
| Consecutive turn stops with no substantive progress | Back off; hand to the semantic-decision role for diagnosis; replace the session once confirmed stuck |
| blocked | Record the block type; permission questions requiring the user are not auto-answered; technical problems the supervisor can resolve go to the lead |
| Task dependencies unmet | Keep the block reason; pick other ready tasks when resources and authorization permit |
| Lead stopped its turn and has unhandled decisions | Wake the lead using the same adapter contract, without relying on its specific hooks |
| Herdr temporarily unreachable | Record unknown, keep ownership, reconcile on reconnect; must not treat it as exited and double-launch |
| Queue empty | Consume no model turns; wake again on new tasks, dependency release, or a specified wake_at |

Progress cannot be judged from terminal scrolling or commit count alone. Combine task phase, artifact changes, running tools/tests, execution receipts, and acceptance evidence. A long test with no output does not mean stuck; repeatedly replying "working on it" does not mean progress.

The recovery budget includes continuation count, no-progress time, replacement count, and cost/time limits. When a boundary is reached, a visible failure/decision is produced, rather than burning quota indefinitely. The overall task remains tracked, and other ready tasks continue.

**7. How to reduce primary-agent message volume**

All workers write structured events, which the program classifies first:

1. Routine progress, normal completion, checks passed: written to the task record and board, merged into summaries.
2. Mechanically recoverable stalls, authorized test fixes: the controller advances directly without asking the primary agent item by item.
3. New dependencies/resource conflicts, authorization requests, unclear requirements, out-of-bounds interface conflicts, exhausted budgets, decompositions needing changes: create a decision item and send it directly to the lead. Interface clarifications within authorized scope may be handled laterally first, with records not lost.

Repeated events for the same blocking object are merged; decisions have stable IDs, versions, handling receipts, and timeout escalation records. When the lead has ten unhandled items, necessary context is provided ordered by predetermined priority and dependencies, without stuffing in the full chats of ten workers. Raw events, artifacts, and logs remain traceable at all times; unknown or not-reliably-classified issues must not be silently filtered.

The lead handles category 3 events; configuring the lead as Codex, OpenCode, Grok, or Pi does not change the notification/retry mechanism. When the lead fails, events are kept and recovery is executed; if it cannot be recovered, it is presented to the user. Independent review is a task that provides evidence and conclusions; it does not add a supervisor layer with dispatch authority.

**7.1 Concrete conventions for reducing reporting and reducing decisions**

- All events are kept in the task record; routine progress, successful checks, and regular completions do not require item-by-item replies from the lead, and are displayed in batches.
- Automatic actions must come from the task contract or project rules: what counts as done, the modifiable scope, executable verification commands, allowed repair/retry budgets, selection rules for the next task, and situations where it must stop. When the rules are insufficient, the program must not be left to guess.
- Ordinary turn stops on approved tasks are auto-continued; verification failures within approved repair scope are auto-fed back; queue items with satisfied dependencies are dispatched in the existing order; none of these generate new approval requests.
- Repeated alerts with the same root cause are merged by a stable key into one pending issue, keeping the list of affected tasks and the raw evidence. Events whose shared root cause cannot be confirmed are not forcibly merged.
- Raw block descriptions and failure evidence go directly to the lead. The program may sort, group, and deduplicate, but must not rely on model summaries to hide uncertainty or rewrite facts.
- Only missing input, interface semantic conflicts, acceptance failures beyond budget, task scope changes, and user permission questions enter the decision inbox; unconfirmed issues stay visible.
- The first version does not add Jev or another generic Agent as a mandatory message-filtering/decision layer. Jev's narrow semantic classification capability can be evaluated separately, but models are not called where program rules suffice, and its output alone cannot approve completion, discard alerts, or change requirements.

The optimization goal confirmed by the user is to reduce the number of judgments needed, not to shift the same number of judgments to another supervisor. Measure the number of human/primary-agent interventions after dispatch for normal tasks, the number of duplicate alerts, and time spent waiting for the primary agent; metrics must not be lowered by deleting records.

**7.2 The lead's mandatory duties and the boundary of lateral collaboration**

The organization uses a single lead directly managing executors. The lead bears global decision responsibility, the background program executes approved rules, and executors may carry out recorded local collaboration. A task's current completion and authorization for new work are different facts.

| Lead's mandatory items | Triggers that must be delivered | Can be handled by program/executors once approved |
|---|---|---|
| Resource and task dependencies | Newly discovered or changed dependencies, circular waits, resource contention, a depended-on party unable to deliver | Wait/unlock per approved dependencies, verify artifacts, communicate delivery details |
| Authorization and permissions | Beyond task scope/budget/permissions, executor requests extra authorization | Run already-allowed commands and fixes; approvals truly reserved for the user are forwarded to the user, the lead cannot sign on their behalf |
| Proactively evaluating task decomposition | Persistent blocks, rework, upstream/downstream contract mismatch, unbalanced granularity, a milestone finding the plan unfit | Collect progress and failure evidence, form snapshots at checkpoints |
| Scheduling new work and capacity | Queue about to run out/run out, no suitable ready item, new goals or reallocation of capacity needed | Dispatch the next already-authorized and ready item, without re-asking for the next step after each completion |
| Goal, scope, and priority rulings | Requirement revisions, changed acceptance criteria, preempting resources/queue-jumping, local solutions with global impact | Make local implementation choices within the established contract and priorities |
| Exception and failure handling | Recovery budget exhausted, persistent non-convergence, contradictory evidence, lead/worker recovery failure | Bounded continuation, retries, saving work, executing approved recovery paths |
| Project-level acceptance and wrap-up | All sub-items done but the overall goal unproven, integration regressions, release/merge decisions, leftover issues needing acceptance | Verify individual items against preset criteria, collect independent review evidence; the project must not be ended merely because all cards are done |

The lead need not personally do every technical investigation; it may dispatch investigation/review tasks to obtain evidence, but the decision rights above do not transfer with the investigation tasks. Executors cannot, by mutual agreement, change third-party interfaces, global priorities, or original acceptance requirements.

Lateral collaboration allows directly requesting already-agreed schemas, fixtures, test results, artifact locations, interface descriptions, and correcting implementation inconsistencies within scope both sides are authorized for; all new dependencies and dependency changes still go into the lead's record/decision entry point. Once a dependency has been confirmed by the lead, there is no need to request approval for each status query.

Collaboration is done through request/response/delivery/verification records linked to tasks, with versions and artifact references written in. Both parties saying "agreed" is not verification evidence, and a promise to deliver does not mean the dependency is satisfied. The program is responsible for waking the recipient, deduplicating, and checking for timeouts and circular waits. Requests are sent only to the owners of related tasks, never broadcast to the whole team. When the agreed negotiation time limit is exceeded, permission boundaries cannot be confirmed, or priority/scope/resource conflicts occur, escalate to the lead with both sides' proposals and the raw evidence.

Proactive re-examination is triggered by accumulated blocks, recovery failures, empty lanes/low queue watermark, requirement changes, milestones, and a configurable period; it must not merely react passively when executors report. A re-examination may conclude "no plan change", and must not manufacture tasks just to stay busy.

Evaluation goal: reduce repeated judgments, not hide dependencies, permissions, and project outcomes that should be in the lead's hands inside local negotiations.

**8. Task queue and resource isolation**

Reuse the same SQLite, adding or extending task dependencies, lane, queue position, role binding, decision, delivery receipt, and recovery record. Task input version, attempt, assignment generation, and acceptance evidence must be linkable.

The board has two projections: by task phase showing todo/executing/blocked/review/done; by lane showing the current item and subsequent order. They read the same set of records.

Reassigning while running first completes a checkpoint and the old owner's release; adjusting pending items immediately affects the next selection. The paused state is persistent with an optional expiry time, no longer using multiple incompatible mute/hold files.

The existing Board's serialization key is session + space, which is not equivalent to actual worktree mutual exclusion; different workspaces can point to the same directory. This approach adds worktree ownership/lock, guaranteeing the same worktree cannot be occupied by two writing attempts. Parallel workers use isolated worktrees, and merging goes through separate verification and serialization. Conflicts such as dependencies, migrations, and shared services are described by task contracts/resource constraints and cannot be solved by Git branches alone.

**9. Long-lived and temporary sessions**

A long-lived lane is an object with responsibilities and a queue, and can be bound to a resident session; changing the session does not lose the lane. Long-lived sessions sleep when idle; if the user needs a re-check after 20–30 minutes, wake_at is recorded. Whether a new task wakes it early is decided by configuration.

Temporary workers/reviewers release their sessions upon completion, and results and review records are kept. Reviewers default to a fresh context, receiving requirements, design, candidate commit, and verification evidence, without inheriting the implementer's chat. Temporary resources are reclaimed per the creator's ownership records; uncommitted changes or unarchived results cannot be deleted when the pane exits.

**10. Decoupling development skills from orchestration**

Per PRD v0.2, the earlier BMAD-specific integration plan is removed. BMAD, Superpowers, and custom skills are all optional task guidance, not runtime dependencies. The core only receives a generic task contract, context references, an optional skill designation, and acceptance requirements.

The first version does no framework version detection, automatic story import, phase mapping, bidirectional state sync, or sprint file write-back, and introduces no generic workflow engine for that purpose. Requirement decomposition and development steps are guided by the lead and the skills the user chooses; the program handles dispatch, collaboration, continuous execution, and evidence recording. Framework-specific adapters, if needed later, are evaluated independently.

The earlier bmad-loop source and tests are kept only as research reference for verification, retries, and review isolation, and do not constitute a flow the product must support.

**11. Development boundaries and order**

| Work package | Main modification entry points | Delivery acceptance |
|---|---|---|
| P0 Unified adapters and role binding | board-core/harness, launch, daemon/spawner, profile/role tables | All five CLIs the user listed can be bound to every role, without depending on a dedicated supervisor |
| P1 Unfinished-task continuation and verification | engine/signals, lifecycle, watchers, dispatch/ops, persistent recovery records | Mid-task turn stops in idle/done are all continued; completion must be verified; no progress reaching budget escalates |
| P2 Persistent decisions and lead self-supervision | New inbox/outbox/receipt and role run records | A lead that stopped its turn can be woken; restart events are not lost and not applied twice |
| P3 Dependencies, lanes, precise queues | model/protocol/db migrations, dispatch/pass, TUI | Reordering affects actual dispatch; blocked tasks do not block unrelated ready work |
| P4 Session and worktree lifecycle | spawner, ownership, recovery, cleanup | Long-lived sleep is recoverable; temporary reviews can be reclaimed; nobody loses unlanded work |

This is a plugin extension requiring database migrations, a recovery state machine, and CLI adapters; it cannot be claimed to be achievable with just a few prompts. The first step is to implement the minimal closed loop of P0–P2, proving that executors on two different CLIs need no manual nudging after stopping their turns, and that behavior is unchanged after switching the primary agent to another CLI; then refine queues, lateral collaboration, and task context passing. A pretty board must not be finished first before verifying continuous work.

Development follows the existing Rust workspace boundaries: board-core manages pure policy/data, board-daemon manages runtime side effects, board-herdr manages transport, board-cli/board-tui manage interaction. Development uses a locally controlled branch, does not modify the Herdr main program, and does not connect multiple background writers directly to the same database. Upstream Cargo declares MIT; no standalone LICENSE text was found in the pinned snapshot, so license and attribution materials need checking before distribution; no fork has currently been published.

**12. Acceptance that must pass**

- The same set of failure tests applies to all roles: lead, executor, reviewer; swap Codex, Claude, OpenCode, Grok, Pi in turn, without relying on any brand-specific recovery shortcut.
- Stopping the turn after two lines, done without submitted results, submission unconfirmed, session exited, status unknown, long test with no output, message replay, and lead restart all have correct paths.
- After switching role bindings, late results from the old session do not change the new task state.
- After closing the TUI the background continues; an SSH disconnect does not equal worker death; reconnecting does not re-dispatch.
- Normal completions and recoverable failures from ten workers go only into summaries; only real decisions reach the primary agent.
- The subsequent order of each lane can be adjusted; reassignment while running must confirm the original execution right is released.
- The same worktree is not double-written just because workspaces differ.
- Independent reviewers use a fresh context, reviews are bound to the candidate commit; modifications after review are re-verified.
- CLIs that natively do not support resume continue with a new session and persistent context, clearly showing this difference, and must not claim the original chat was restored.

Target metrics: the number of manual nudges to "keep going" is 0; unfinished tasks with no owner/no recovery record are 0; duplicate dispatches are 0; primary-agent decision message volume, recovery latency, and supervision tokens are measurable. External service failures or authorization blocks must be explicitly escalated; success of models or networks cannot be promised.

**13. Evidence and current status**

This revision re-examined Board's harness, configured launcher, model/protocol, dispatch, supervisor, and database entry points, confirming the reusable boundaries and the parts still requiring development. No new real Agent run tests were added, and old test results were not presented as the new architecture having passed.

The previous round's original test results remain valid: 11 targeted BMAD scenarios passed; the Dispatch research probe confirmed the doing/done missed branch and the doing/idle prompt-count behavior; BMAD Herdr adapter 111 passed, 2 failed; Firstmate had selected test failures/timeouts. See [validation.json](validation.json). These are evidence about candidate implementations, not an acceptance certificate for this approach.

**Final conclusion: development is needed, and what is developed is a generic continuous-execution and role layer based on Herdr Board. The Agent choice for every role is decided by the user; the system's continued progress cannot rely on fixed Claude, fixed Codex, or any proprietary hook.**
