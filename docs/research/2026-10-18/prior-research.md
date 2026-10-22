> Historical research record: the fixed Codex overall-owner / Claude execution-lead arrangement and the recommendation to build on Firstmate have been withdrawn. The current conclusion is in [decision.md](decision.md). This file only preserves the source observations and test evidence from that time; it is not an implementation plan.

# Research and Selection for Herdr Multi-Agent Continuous Delivery

Research date: 2026-10-18 (America/Chicago). Sources are pinned by the commits in [sources.lock.json](sources.lock.json). This document is a selection and implementation design; no plugin was connected to an existing production session.

**Decision: reuse Firstmate's crew management, messaging, supervision, and recovery mechanisms; reuse the Herdr runtime environment; reuse the Captain’s Deck terminal board; BMAD retains requirements, design, story, and acceptance standards. Keep Codex as the overall owner, first pairing it with one resident Claude Code execution lead, who manages heterogeneous executors and temporary reviewers. Development scope is limited to BMAD bridging, precise queue operations, and necessary compatibility fixes. Do not build a scheduling runtime from scratch, and do not put bmad-loop in the master-control position of the whole parallel crew.**

The selection can be settled; production readiness cannot yet be claimed. Isolated testing on this machine exposed failures and timeouts, which must be resolved as a go-live gate. Source tests prove mechanisms; they do not prove that a real model will never stall. The final guarantee should be "anything unfinished always has a recoverable record, always has a person or program continuing to handle it, and failures always escalate", not "every task will always succeed automatically without limit".

**1. Selection criteria**

The user's core demand is neither that the Bash scripts must be kept, nor that the same execution session must always be used, but rather:

1. Authorized tasks are not left with no one continuing just because the model ended a turn.
2. The controller does not bear routine nagging of each executor, repeated status inquiries, or routine fix conversations.
3. Requirements can be broken into tasks with acceptance conditions, forming a queue that can be viewed and adjusted.
4. When dependencies appear, dispatch can be adjusted while other workable items continue.
5. Native Codex, Claude Code, OpenCode, Grok, Pi, etc. can be mixed.
6. Runs on an SSH server, viewed and controlled from a terminal; closing the board should not stop tasks.
7. Resident management duties and temporary execution/review sessions have different lifecycles.
8. BMAD artifacts and development standards are retained continuously, not lost after migration.

Evaluation order: continuous progress and recovery > controller burden > heterogeneity and concurrency > state correctness > BMAD integration > queue interaction > install footprint. A nice-looking board cannot make up for a missing execution loop; an unattended serial story engine cannot automatically become a ten-lane scheduler either.

**2. Corrections to the previous two rounds' conclusions**

- BMAD Loop already has an external Herdr backend: [pbean/bmad-loop-adapter-herdr](https://github.com/pbean/bmad-loop-adapter-herdr). The earlier judgment "no Herdr backend found, so we need to write our own adapter" was incomplete.
- Herdr Dispatch does handle idle workers that have already claimed a task and are in doing. It cannot be described only as "nagging unclaimed tasks".
- Firstmate is not an ordinary board add-on. It already has a long-lived lead, heterogeneous workers, a persistent inbox, message acknowledgment, supervision, recovery, isolated worktrees, reclamation, and a remote lead mechanism; it should be the primary candidate.
- The web cache viewed in the previous round was not fully in sync with the current repositories. For example, the current bmad-loop 0.13.2 already includes the BMAD v7 ticket-tree mode. Selection is based on the sources pinned this time; old-version conclusions from search caches are not mixed in.

**3. Actual boundaries of the candidates**

| Option | Capabilities confirmed by source/docs | Decisive shortcomings for this need | Selection |
|---|---|---|---|
| Original four scripts | idle/done continuation, cooldown, hold/mute, partial rate-limit detection, reporting conventions | No unified task facts, completion acceptance, or persistent event acknowledgment; project config hard-coded | Used as failure cases and a source of requirements; not extended further into a general platform |
| Herdr itself | Native terminal, Agent state, start/prompt/read, workspaces, plugin entry points | Manages processes and sessions; not responsible for judging business task completion | Kept as the runtime base |
| hassox/herdr-kanban | pane as card, column triggers, conductor | Pane lifecycle coupled to task lifecycle; lacks a complete task recovery loop | Not used as the base |
| nelsonPires5/herdr-board | SQLite, TUI, card/run history, automatic column pipelines, heterogeneous launch, concurrent queue | Uncommitted idle/done results go to awaiting; lacks controller offloading and a long-lived lead layer | Not the main base this time |
| husniadil/herdr-tasks + herdr-dispatch | Dependency readiness, task leases, evidence and review, automatic workers, doing/idle nagging | The two input-receivable states are handled inconsistently; doing nagging has no corresponding count cap; provider paths specialized; review/merge left to external | Its overall scheduling not adopted; task leases and pure decision tests used as reference |
| Phoobobo/herdr-workboard | Workflow and role run records, TUI, idempotent state transitions | No resident execution engine; cannot solve stalls on its own | Not adopted |
| bmad-loop + Herdr adapter | Artifact verification, fix retries, independent review, recovery, stall wakeup, BMAD v6/v7 integration | A single run is currently serial; the concurrency setting is clamped to 1; the external adapter has interface drift; lacks multi-crew dynamic scheduling | Kept as a standalone serial batch option, not layered onto the default crew |
| Firstmate + Captain’s Deck | Crew/lead, heterogeneous workers, persistent messages, supervision, recovery, reclamation, terminal overview | Many tools in the chain; Codex controller supervision capability has limits; precise queue editing and two-way BMAD bridging need to be added | Adopt its mechanisms and core, without claiming it fully satisfies the need out of the box |

Official projects: [Herdr](https://github.com/herdrdev/herdr), [Herdr Board](https://github.com/nelsonPires5/herdr-board), [Herdr Tasks](https://github.com/husniadil/herdr-tasks), [Herdr Dispatch](https://github.com/husniadil/herdr-dispatch), [Firstmate](https://github.com/kunchenguid/firstmate), [Captain’s Deck](https://github.com/deimantasnork/captains-deck).

**4. Four pieces of source evidence that affect the selection**

**4.1 Herdr Board can detect stopping, but by default does not recover unfinished tasks.**

`crates/board-core/src/engine/signals.rs::decide_signal` maps `Done` and `IdleExpired` to `CardStatus::Awaiting` and emits a notification; `lifecycle.rs::decide_lifecycle` returns `TimeoutPaused` for an Awaiting timeout. This is an explicit product policy, not a keep-alive setting we overlooked. The result is that tasks may still need handling by an external lead.

[Pinned source](https://github.com/nelsonPires5/herdr-board/blob/7a215a179acb4e098dd1c2971332497806309360/crates/board-core/src/engine/signals.rs)

**4.2 Herdr Dispatch's half-done continuation mechanism still has real gaps.**

`internal/decide/decide.go::Decide` only nags when `task.Status == "doing" && agentStatus == "idle"`; `done` has no equivalent branch. `internal/loop/loop.go` writes the state it reads into the snapshot as-is and does not normalize done into idle there. The doing branch also does not use the todo branch's `MaxPrompts` cap.

This pass copied the unmodified pure decision package into an isolated directory and added a probe: task doing, 100 historical prompts, configured cap 2. Actual result: idle still returns Prompt; done, blocked, unknown, and working all return no action. Test file and output: [probe](evidence/dispatch_probe_test.go), [result](evidence/dispatch-probe.log). This does not prove that every done in production will stall forever, but it is enough to refute "the default already correctly covers all of Herdr's normal turn-end states".

[Pinned source](https://github.com/husniadil/herdr-dispatch/blob/852f94150a5159b261ca0e932aa126fb1f24b7ff/internal/decide/decide.go)

**4.3 BMAD Loop is not a ten-lane parallel engine.**

`src/bmad_loop/policy.py` explicitly executes `max_parallel=min(requested_parallel, 1)`. Loading `max_parallel = 10` directly, the measured result is 1; upstream also has `test_scm_max_parallel_clamped_to_one`. `docs/FEATURES.md` marks Parallel units as planned.

Multiple loops can be run in isolated repositories, but that additionally introduces unique task assignment, story state merging, branch merging, and cross-run dependency management. That is already writing a parallel master scheduler; it cannot be called getting ten-lane capability with a bit of configuration. Even less can ten loops be started on the same working copy, hoping that a state file write lock equals unique task claiming.

[Pinned source](https://github.com/bmad-code-org/bmad-loop/blob/a5a8f09781e151eb5691f55d55316f47f9f16f1c/src/bmad_loop/policy.py), [Capability notes](https://github.com/bmad-code-org/bmad-loop/blob/a5a8f09781e151eb5691f55d55316f47f9f16f1c/docs/FEATURES.md)

**4.4 Firstmate's supervision is not the same for every controller CLI.**

- Pi: an independent supervision conversation in the same process; task-level routine handling need not occupy the main conversation.
- Claude/Cursor: the supervision host can handle routine events while someone is present.
- Codex: by default a foreground checkpoint, during which it cannot reason; the host is pass-through in the attended state. The `quiet` daemon filters/batches events, but cannot pose as an independent supervision session with the same judgment capability.
- Firstmate's pure-Bash watcher consumes no model tokens; judging how to fix, reordering tasks, and redispatching may still consume lead model tokens. We cannot advertise "the whole orchestration costs zero tokens".

Therefore this plan does not bet that native Codex already has equivalent background supervision; instead it uses the existing secondmate role to push the ten lanes down to one Claude lead. Codex keeps global judgment and user communication and does not directly consume each worker's routine turn-end events.

[Supervision host](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/docs/supervision-host.md), [Codex protocol](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/docs/supervision-protocols/codex.md), [Pi branch](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/docs/pi-supervision-branch.md)

**5. Recommended organization and control authority**

```text
User
  ↕ requirements, key decisions, delivery results
Codex overall owner (Firstmate primary)
  ├─ holds PRD/architecture intent, task breakdown, priority, and change authority
  └─ one Claude Code execution lead (Firstmate persistent secondmate)
       ├─ task queue and supervision records for the same execution home
       ├─ Codex / Claude / OpenCode / Grok / Pi temporary executors
       ├─ independent review and verification sessions
       └─ Herdr panes + isolated worktrees

Captain’s Deck displays the existing facts of each home; it does not keep a separate copy of task state.
```

Phase one has only one execution lead; it does not mechanically split into ten team leads. Its charter is explicit: keep advancing within authorized tasks and technical constraints; handle routine turn-ends, questions already answered in the brief, test failures, and disconnect recovery itself; escalate to Codex only for requirement changes, interface semantics conflicts, exhausted budgets, and matters it cannot decide itself.

This adds one long-lived model session in exchange for controller context isolation and batched reporting. It is not free; the supervision tokens per completed task, total elapsed time, and number of human interventions should be measured. For small projects with only one or two short tasks, the controller can supervise directly, avoiding the fixed cost of an extra layer; this does not change the recommended topology for ten-lane projects.

Firstmate already has `data/secondmates.md`, a charter, home seed, `config/secondmate-harness`, cross-home backlog handoff, routed replies, and recovery flow. There is no need to implement a "team lead system" ourselves. For same-machine and server deployments, prefer keeping the whole home close to the code; the user SSHes into the server and uses the TUI. Across machines, use remote routing of the whole secondmate home, rather than having a local background process poll every worker one by one over SSH.

**Single control authority must be strictly maintained:** the overall owner sets strategy, the execution home is responsible for its tasks, and workers may only advance the work assigned to them. When the overall owner adjusts an in-progress item, it goes through the lead's steering path, without simultaneously sending a second set of instructions directly to that worker. After receiving a higher-level change, the lead updates the brief and leaves a record of the change and execution acknowledgment.

Important limitation: current Firstmate keeps `local-only` projects in the primary home; without modifying the rules, we cannot claim all projects can be handed to a secondmate. Projects that need direct local merges without PRs should use a single-home Claude execution owner plus external Codex strategy input, or make local-only secondmate support an explicit extension; the default rollout chooses PR delivery, and does not quietly change the delivery method of the user's original projects.

**6. How a task actually keeps going**

Take "implement T-21, run tests, submit for review" as an example:

1. **Preparation**: the task record contains the story link, input version, change scope, acceptance commands, expected artifacts, and dependencies; it is not dispatched if dependencies are unmet.
2. **Dispatch**: Firstmate `fm-spawn.sh` creates an isolated worktree and Herdr endpoint, and syncs the In flight status when tasks-axi is enabled. A task does not rely on the controller remembering it after dispatch.
3. **Routine stop**: the worker writes two lines and ends a turn. The watcher judges whether a wakeup is needed based on execution state, task declarations, and artifact/activity evidence; actionable events enter a persistent wake queue. The execution lead handles it, rather than letting the user discover it first.
4. **Continuation**: the lead gives the specific remaining work via `fm-send.sh`'s persistent steering inbox. Unacknowledged messages are retained and re-rung; a single CLI return of 0 cannot be taken as the task having been received.
5. **Correction and recovery**: first check the brief and current artifacts; correct in a targeted way if necessary; if truly stuck, replace the session in the original worktree via `fm-control.sh relaunch --note ...`. Stopping the old session and starting the new one must be verified; two executors must not write the same worktree at the same time.
6. **Verification**: preconfigured tests/builds and independent review verify the artifacts; a "done" reply is only a claim. Only when the branch, verification results, review results, and pending deliverables agree can it proceed to delivery.
7. **Failure handling**: if repeated fixes make no progress and restart also fails, preserve the work and the reason, create a blocked/failed record, and escalate. Other workable tasks continue.
8. **Reclamation and next item**: once delivery conditions are met, reclaim the sessions/worktrees it owns via `fm-teardown.sh`; the execution lead rechecks the ready queue and capacity and continues dispatching.

Of these, the tools and data channels for 2, 4, 5, and 8 already exist; 3 has the watcher and supervisor protocol; 6 reuses the existing delivery gate; whether task semantics are satisfied and when to switch work still requires the lead's judgment. We do not claim that all policies are already pure programmatic automatic decisions.

[Recovery flow](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/.agents/skills/stuck-crewmate-recovery/SKILL.md), [Control transactions](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/docs/agent-control.md)

**7. How BMAD connects, and what is reused directly**

BMAD keeps the development facts: PRD, architecture, UX, epics, stories/tickets, acceptance requirements. Firstmate keeps the execution facts: task queue, execution ownership, worktrees, supervision messages, review/delivery status.

Concrete integration conventions (these are proposed interfaces, not posing as existing commands):

- `orch bmad import <project>`: detect the actually installed BMAD generation and artifact format; idempotently import into tasks-axi by project + story/ticket ID; keep the original path and content digest. One story can be split into multiple execution items, but the parent-child mapping must be recorded.
- `orch bmad brief <task>`: generate a minimal work package for the executor: story and acceptance conditions, necessary architecture excerpts, interface contracts, verification commands, forbidden change scope, result format. Reference the installed BMAD development skill; do not broadcast the whole PRD to every executor.
- `orch bmad reconcile <project>`: one designated writer writes verified/delivered execution results back to BMAD state. If any required subtask/review is incomplete, the parent story cannot be marked done. Use v6 YAML/story or v7 ticket tooling according to the actual version; do not let multiple workers directly edit a shared sprint file.
- Freeze the requirement version when a task starts; a requirement change during execution must leave a revision record and a continue/reopen decision; the same task ID must not silently switch acceptance criteria.

**Choose only one delivery path.** By default, reuse the no-mistakes management already supported by Firstmate: review → test → docs/lint → PR/CI, with BMAD acceptance conditions and related design fed into its review/test instructions. It supports heterogeneous CLIs, independent verification worktrees, and fix loops.

If a task explicitly requires a specific native BMAD independent review workflow, that workflow is connected as a named independent review item or verification gate and its report saved; we cannot falsely claim that no-mistakes' ordinary review equals that BMAD workflow. Review conclusions must be bound to the same candidate commit; patches after review need re-verification. Whether to run two same-scope reviews must be stated explicitly in the task contract, to avoid unintentionally paying twice.

The first version does not apply bmad-loop and no-mistakes to the same story simultaneously. Both engines run verification, review, and state advancement, and may each create worktrees/commits; such stacking makes recovery and acceptance authority unclear. bmad-loop can be reserved for standalone serial batch projects and must not compete with the default execution queue for the same story.

[no-mistakes project](https://github.com/kunchenguid/no-mistakes), [Configurable review/verification entry](https://kunchenguid.github.io/no-mistakes/reference/repo-config/)

**8. Board and precise queue: what exists and what must be added**

Captain’s Deck already has per-crew/home views, working/waiting/done views, Agent and model info, jump-to-pane, Captain’s Call decision input, and sending messages to Firstmate. It uses the Firstmate snapshot and is suitable to keep reusing.

It is currently not a complete editable scheduler. tasks-axi has ready queries, dependencies, priority, hold/unhold, and task updates, but priority 0–4 cannot be advertised as arbitrary precise queuing, much less can we say the existing TUI already supports dragging any Agent's task queue.

Two cooperating surfaces need to be added:

| Added capability | Implementation location | State ownership |
|---|---|---|
| Explicit lane ownership and precise queue order | Extend the tasks-axi task model/adapter, or store equivalent fields via its controlled updates; introduce lane and queue_rank | Still written by the same task backend |
| Keyboard move, pin to top, reassign, pause/resume | Captain’s Deck action entry points | Call task CLI/Firstmate commands; do not edit the snapshot directly |
| Card details showing follow-up tasks, dependencies, acceptance, latest failure | Extend the existing snapshot projection | Read-only projection |
| Reassignment while running | Via the lead and `fm-control` transactions, preserving the original task and artifacts | Confirm handoff first, then change the single owner |

First support keyboard `move up/move down/pin to top/reassign/pause/resume`; no new web application is needed. Cross-queue moves across multiple projects or homes reuse the existing handoff; a running task cannot be moved directly as if it were a static card.

For arbitrary rank, both the ordering and the read path must be changed, with concurrent write protection added; just writing "rank first" in the Markdown body does not count as implementation. Temporarily adjusting by priority can serve as a transition, but is not full acceptance.

[Deck controls and boundaries](https://github.com/deimantasnork/captains-deck/blob/058ff8af75b336ffdc8537b92c7e4825025be0f4/README.md), [tasks-axi](https://github.com/kunchenguid/tasks-axi)

**9. Rollout rules for long-lived and temporary lanes**

- Long-lived: the overall owner, execution lead, lane responsibilities, and queues. Only duties that genuinely need ongoing project context keep resident sessions.
- Concrete implementation tasks default to short sessions; related consecutive tasks can keep context, but "permanently keeping ten chat windows alive" is not the default.
- Independent review uses a new session that only receives the task/design/candidate diff/verification evidence and does not inherit the implementer's conversation. After exit, the report and its commit association are retained.
- An empty queue sends no "any more work?" messages and consumes no model turns. A new task, a dependency being cleared, or an explicit recheck time triggers the next piece of work.
- Your original 20–30 minute rests were mainly to avoid the scripts constantly waking empty lanes. With events and a persistent queue, it can simply wait quietly; if periodic rechecks are still needed, store a separate wake_at; a mute flag that never auto-recovers must not pose as a timed pause.
- "Reclaim temporary Agents after use" means session resources can be released; worktrees must meet artifact preservation and delivery conditions. A process exiting does not authorize deleting uncommitted work.

**10. Dependencies and dynamic scheduling**

In real projects it is impossible for all tasks to have no dependencies. The default is to execute the ready queue, not blindly execute the first item of a text list.

For example, T-22 page integration depends on T-21's interface contract: "define the contract" can be pulled forward as a small task, with subsequent implementation in parallel; if the contract is still unclear, T-22 is blocked and the execution lead dispatches other ready tasks. Do not let executors guess the interface themselves, and do not pause the whole project each time.

Beyond dependencies, also consider shared external resources, database migrations, deployment to the same environment, branch baselines, and final merging. A worktree only isolates the filesystem and does not automatically resolve these semantic conflicts. Firstmate's project capacity is a worker capacity limit and cannot pose as a global model concurrency limit that includes all gate subprocesses; account separately for review/verification concurrency and resource overhead at deployment.

**11. How much needs installing, and where the lightness lies**

Firstmate's maintenance cost must not be hidden. It is not a small skill. General dependencies listed in the current docs include Node, Git, gh, no-mistakes, gh-axi, chrome-devtools-axi, tasks-axi, quota-axi; the Herdr backend also needs jq and treehouse. Lavish is a degradable presentation-layer dependency; there is no requirement to add a web board for this project. Which features are actually used and which tools the upstream bootstrap requires are two different questions.

The advantage is that these are all existing tools and terminal flows; we need neither Orca's standalone desktop work environment, nor to maintain our own PTY, model SDK, Agent launcher, remote control protocol, and recovery database.

Recommend keeping the pinned-version Firstmate core intact; do not extract dozens of shell files and stitch them into a so-called lightweight version. These scripts share locks, message acknowledgment, home routing, and permission conventions; once split apart, the maintenance cost falls back on us. Keep customization to private home configuration, the BMAD bridge, and small UI/task-model patches.

[Dependencies and configuration](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/docs/configuration.md)

**12. Implementation order and explicit deliverables**

| Phase | Actual work | Completion signal |
|---|---|---|
| A Pin the base | Pin Herdr/Firstmate/Deck/dependency versions; isolated test home; fix or locate this round's failures; keep the old scripts' running scope | A compatibility record matched to the target server version; tests do not touch the default session |
| B Off-the-shelf loop | Codex primary + one Claude secondmate + two kinds of workers; configure brief, capacity, reporting permissions, recovery; open the Deck | Without human nagging, routine interruptions recover, lead failure is detected, completion has evidence |
| C BMAD bridging | Idempotent import, minimal task packet, version detection, subtask mapping, single write-back, review evidence binding | A real story goes from approved requirement to delivery and is written back; failure states are not erased |
| D Queue interaction | lane/rank, ordering, pin to top, reassign, pause/resume, detail projection | TUI changes affect the actual next dispatch; closing the TUI does not affect execution |
| E Failures and scale | Ten-task stress, lead/worker restarts, disconnects, duplicate events, dependency changes, review failures | Replace the original scripts only after the acceptance matrix below passes |

This is integration plus limited extension, not zero development. The core runtime is not rewritten; BMAD and scheduling interaction need engineering work. Until A's test problems are resolved, the user's existing projects should not be taken over directly.

**13. Go-live acceptance matrix**

The following are proposed delivery gates and must not be taken as all having been measured and passed this time.

| Injected scenario | Required behavior | Not allowed |
|---|---|---|
| Worker changes only two lines and stops | Fails acceptance; supervision detects it; automatic continuation or recovery | No one handles it, or the task is marked done just because the Agent is done |
| Both idle and done turn-end states | Both enter task state reconciliation | done is missed because it was not normalized |
| A prompt submission response is lost | Read the current state/receipt first, then decide whether to resend | The same dispatch is executed twice |
| working but no task progress for a long time | Conservatively check tool/verification processes; escalate after confirming the stall | Killing a long test based only on terminal silence |
| Supervisor session exits | Persistent records remain; the parent detects and recovers the execution lead | Can only wait for the user to notice |
| daemon/watch crashes | Unacknowledged events can be replayed; ownership is not acquired twice | Messages lost while showing healthy |
| Herdr/SSH temporarily unreachable | unknown kept separate from dead; reconcile after reconnecting | Misjudged as dead and a duplicate worker started |
| T1 blocked, T2 workable | T1 records the specific dependency; T2 continues | All work stops at the head of the queue |
| Change the order of the next task | The next dispatch uses the new order, with a history record | Only the board order changed |
| Temporary reviewer completes | Report/commit persisted; session reclaimable | Reusing the implementer's context while posing as an independent review |
| Queue empty/manually paused | No repeated model turns; a new task or timed condition can wake it | mute permanently forgotten, empty lanes nagged endlessly |
| Ten tasks with mixed completion/failure | Lead summarizes; overall owner sees only anomalies and decisions | Every token/every routine fix directly disturbs the overall owner |

Suggested measurements: number of human "keep going" prompts (target 0), number of unfinished tasks with no one handling them (target 0), number of duplicate dispatches (target 0), stall detection and recovery latency, number of controller events, supervision tokens, total time per task. Latency thresholds should be set per actual task type; long builds and short file edits cannot be treated with one blanket threshold.

**14. Results and limitations of this round's verification**

This round cloned and pinned 9 repositories; inspected key source and upstream tests; verified using temporary worktrees, mock CLIs, and isolated state. It consumed no real coding-agent model quota, did not invoke the user's existing Agents, and did not upgrade the Herdr server. The local client version, queried read-only, is 0.9.1; the server version was not inferred from it.

- BMAD Loop: 11 targeted failure/policy cases passed, including continuation after a turn-end that produced no result, stall wakeup, prompt send failure counting, retries, recovery, no-work pause, and concurrency suppression. See [log](evidence/bmad-targeted.log).
- Broader BMAD test run: 565 passed, 1 failed, after which it was deliberately aborted to focus on key scenarios; the failure was the macOS temporary filesystem not accepting a test-constructed filename with invalid bytes. It cannot count as an implementation logic defect, nor as the full suite passing. See [log](evidence/bmad-tests.log).
- Herdr external adapter: 111 passed, 2 failed. One is that the `switch_client` return type convention has drifted from the current core; the other is that a Windows backend selection test still expects the old default. This does not prove Linux startup will necessarily fail, but it does prove the latest combination cannot yet be labeled compatible directly. See [log](evidence/bmad-herdr-adapter.log).
- Herdr Dispatch: upstream tests for the pure decision package passed; the research probe confirmed the actual behavior of doing/done and the count cap. The config package also passed; the loop package timed out at the 10-minute overall limit, and spawn did not get a completed result. See the verification summary for other details; unfinished parts are not recorded as passed.
- Captain’s Deck: 110 passed, 2 failed; involving macOS `/var` vs `/private/var` path comparison, and a 20-second timeout on simulated startup. After normalizing the temp directory to `/private/tmp`, these two were rechecked separately and both passed (12.31 seconds), without changing product source. The first full run therefore cannot be rewritten as all green.
- Firstmate: 5 Python cases for Herdr event-wait passed; among the 6 selected shell suites there were lock/timing/state assertion failures and 300-second timeouts. No all-green result for these suites was obtained on this machine; some suites keep producing output after failing, so success cannot be judged only by exit code or the last line. A separate recheck of checkpoint still failed on a cleanup timing assertion in the Perl fallback, so it cannot be attributed solely to parallel testing. The original logs are retained; product issues, test assumptions, and machine load need further separation in the target Linux environment.

These results are enough to determine the reuse boundary and implementation direction, but not enough for production certification. The final verification details are authoritative in [validation.json](validation.json) and the evidence directory.

**15. Why this path is more suitable than continuing to extend the scripts**

It has already turned the common problems you kept patching in the scripts into reusable mechanisms: persist messages before waking, consume only after acknowledgment, separate tasks from sessions, preserve work on failure, supervise the lead itself, separate pauses from anomalies, and reclaim temporary resources in a controlled way. What we mainly need to add is your BMAD task interface and the scheduling experience.

Continuing to extend the original scripts means re-solving these foundational problems; starting from Herdr Board means separately adding supervision and recovery; starting from bmad-loop means separately adding concurrent scheduling. Firstmate's cost is accepting an existing toolchain and the lead role, but among the candidates found so far it is the shortest path to "ten Agents working continuously, with the controller handling a small number of key decisions".

**Final adopted route: Firstmate handles collaboration and continuous supervision, Herdr handles running, BMAD handles the development contract, Captain’s Deck handles terminal visualization; build only the connection layer and queue interaction extensions. Complete isolated compatibility and failure acceptance first, then migrate real projects.**
