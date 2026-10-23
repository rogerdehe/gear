# Orca's Project Orchestration Capabilities and Failure Safeguards Worth Borrowing

Research purpose: learn the work organization and failure handling Orca has already implemented, to avoid repeating the same mistakes. This document does not recommend that the user migrate to Orca, nor does it treat Orca's GUI, runtime, or organizational conventions directly as our requirements.

Pinned source: `stablyai/orca@76f9978880752f2b3ed63bb8c2698b8a7236440d`. This pass read the official repository's coordination guide, core source, kanban source, and regression test assertions; it did not run the Orca application, did not install its full toolchain, and did not execute its upstream tests. The source version, fetched files, and checksums are in [orca-sources.json](orca-sources.json). "Confirmed" here means the implementation/contract exists; it must not be read as having passed production testing. Previously reading only the README was not enough to satisfy the user's request; this pass fills in the orchestration layer that had been missed.

**1. Answers to the user's three direct questions**

| Question | Conclusion | Actual basis and limits |
|---|---|---|
| Does Orca have a kanban board? | Yes, and at least two different kinds | The Agent board shows snapshots grouped as Needs You / Working / Done / Idle; the Workspace board supports status columns, moving, ordering, and optional Linear status sync. Neither of these UI states may be treated as a Task-DAG acceptance result |
| Does Orca dispatch work? | Yes, with structured orchestration | Run, Task, Dispatch, worker-start, dependencies, ready queries, decision gates, messages, and explicit results; it is not just sending text to a terminal |
| Does it handle Agent stopping, reminders, and recovery? | It has explicit supervision/recovery mechanisms | Separate PTY/Agent liveness, prompt delivery, worker_done, lost-contact-unknown, and cleanup ownership; the coordination loop is responsible for continuous checking, and process exit can be escalated to the Run inbox and wake waiters. There is no evidence that would let it be summarized as "the background will unconditionally make any Agent continue forever" |

**2. The actual organizational model**

Orca's orchestration guide separates three objects: a Run is a persistent namespace and coordinator inbox; a Task is work; a Dispatch is one authoritative execution attempt of a piece of work. The Run itself is not responsible for scheduling or placing workers.

The coordinator first creates an independent wave of tasks, records the real dependencies, then starts workers. It supports `task-list --ready` to find executable tasks and `worker-start --task` to start an existing task; a retry uses the same Task with `--retry-of` pointing at the original attempt. A completion report must carry the current Task/Dispatch identity and an explicit succeeded/failed result.

A valid worker_done settles both the Task and the Dispatch; therefore "who reported the settlement" and "whether the work content was verified" must be judged separately. Our product requires corresponding acceptance/review evidence after a completion claim; a single success claim cannot be taken directly as final delivery.

[Coordination contract](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration.md), [DAG and reuse](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/coordinator-loop.md)

**3. What the kanban boards actually manage**

`AgentKanbanBoard.tsx` groups from the DashboardSnapshot into four columns: attention, working, done, idle, sorted by status-change time, and can open the corresponding terminal. It expresses the Agent's current state and items needing attention.

The workspace board is a different surface. `use-workspace-kanban-status-actions.ts` manages status names, icons, colors, column order, and which workspace items belong to after a status is deleted; it carries executionHostId when updating worktree metadata. `WorkspaceKanbanDrawer.task-status-sync.test.tsx` has assertions on Linear sync after drag/menu changes, and also makes explicit that "only changing manual order, without changing status" does not trigger status sync.

So what can be borrowed: project the UI from real snapshots; preserve execution host identity; distinguish what a board operation means; make sync failures visible. **This pass did not prove that its workspace card ordering is the actual task dispatch priority for orchestration; we cannot claim that an existing feature already covers the per-Agent follow-up task queue the user needs.**

[Agent kanban source](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/renderer/src/components/dashboard-popout/AgentKanbanBoard.tsx), [Workspace actions](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/renderer/src/components/sidebar/use-workspace-kanban-status-actions.ts), [Sync tests](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/renderer/src/components/sidebar/WorkspaceKanbanDrawer.task-status-sync.test.tsx)

**4. Encoded failure safeguards and their mapping to our requirements**

The following are contracts, implementations, or test scenarios that can be read explicitly. Having a regression test does not mean we know the full history of each incident, so we do not fabricate every safeguard into "an incident that happened in production".

| ID | Failure mode | Orca's handling | Into PRD |
|---|---|---|---|
| O-01 | Two dispatchers claim the same task/terminal at the same time | Task/Dispatch transactions and occupancy checks; the operation that loses the race is rolled back/rejected | FR-02/04: single write attempt, atomic dispatch |
| O-02 | Dependency incomplete at query time, but already complete when the task is inserted | Creation and status-reconciliation tests cover this race; failed/blocked dependencies cannot unlock downstream | FR-04: real readiness conditions, avoid being stuck in pending forever |
| O-03 | Prompt was pasted, but no new turn start was observed | Marked dispatched-unobserved; no re-paste, task not put back to ready; a late report can still settle correctly | FR-05: uncertain delivery is not the same as no delivery |
| O-04 | Handler crashes after receiving a message | Persistent Delivery replays FIFO, ack only after processing; type filtering only decides when to wake, it does not allow skipping old messages | FR-07/12: messages not lost, processing recoverable |
| O-05 | Old worker recovers and tries to ack the new worker's messages | Takeover increments consumer generation; old consumer receives consumer_fenced, new consumer gets unread messages | FR-03/12: prevent old sessions overstepping authority and messages being wrongly swallowed |
| O-06 | Start fails, and each retry creates another live process | Start failures counted across retry-of; circuit breaks on the third and rejects the fourth; the worker's own business failures are not counted the same way | FR-05: distinguish start failure from task failure, set a recovery budget |
| O-07 | PTY still alive, but the Agent is actually dead or stalled | Fleet liveness and worker-show's PTY observation are layered; terminal liveness cannot substitute for executor liveness | FR-05/06: multi-layer state and evidence sources |
| O-08 | Worker exits while the coordinator is still waiting | Exit escalation written into the owning Run mailbox; tests cover waking check --wait, notifying only the correct Run, and the case where the coordinator is not yet bound | FR-07/12: an exit cannot just leave a log, it must produce a pending item |
| O-09 | Remote end loses contact and local cleanup/restart begins | The execution host owns the actual facts; lost contact is unverifiable; operations are routed by Dispatch, no guessing remote terminal handles | FR-12: unknown is not dead, no cross-host misoperation |
| O-10 | When releasing an old task's terminal, it has already been reused or taken over by the user | Release checks settlement, ownership, process incarnation, and archival; does not clean pre-existing, setup, coordinator, active, or user-taken-over terminals | FR-10: cleanup must be based on resource ownership, not name matching |
| O-11 | Deleting a worktree also deletes unmerged commits | Tests require safe branch -d; when unmerged, keep the branch and head, do not escalate to branch -D | FR-02: session, directory, and branch reclaimed separately |
| O-12 | Calling again after a request timeout, causing duplicate mutation | request ID and request-show recover completed/pending state; absent also does not automatically prove it never executed | FR-05/12: retries must reconcile operation receipts |

Corresponding source and tests:

- O-01: [Task/Dispatch invariants](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db-task-dispatch-invariant.test.ts), [Concurrency races](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db-task-dispatch-races.test.ts). These cover "an old failure cannot overwrite an already completed worker report".
- O-02: [Dependency readiness race](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db-task-create-readiness.test.ts).
- O-03: [Dispatch implementation](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/coordinator-task-dispatch.ts), [Unobserved start test](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/coordinator-dispatch-unobserved-prompt.test.ts), [Start record correction](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/worker-start-unobserved-prompt-settlement.test.ts).
- O-04: [Persistent delivery contract](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/messaging-and-gates.md), [Mailbox implementation](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db/messages/role-mailbox-delivery.ts).
- O-05: [Old consumer revocation test](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db/dispatch-mailbox-consumer-fencing.test.ts).
- O-06: [Failed-start circuit breaker test](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db/worker-dispatch/failed-start-circuit-breaker.test.ts). The counting semantics cannot be taken directly as the cap for all business fixes.
- O-07: [Recovery guide](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/recovery-and-cleanup.md), [Attention item classification](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/shared/orchestration-fleet-attention.ts).
- O-08: [Exit escalation test STA-4604](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/lightweight-run-worker-exit-escalation.test.ts).
- O-09: [Remote placement and capability negotiation](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/placement-and-remote.md).
- O-10: [Resource ownership](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/worker-terminal-ownership.ts), [Release transaction](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db/worker-terminal/worker-terminal-release.ts).
- O-11: [Worktree removal and branch preservation test](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/git/worktree-remove-branch-deletion.test.ts).
- O-12: [Mutation recovery contract](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/recovery-and-cleanup.md). Additionally, `RemoteRuntimeTerminalCreateIdempotency` only merges identical in-flight create requests and removes the in-memory entry on completion; this small piece of code cannot be advertised as persistent idempotency across restarts.

**5. Reminders and keep-live: what clearly exists, and what still needs verification**

The worker contract requires checking for follow-ups at natural checkpoints and once more before worker_done; after completion it ends the current turn and must not start unrelated work on its own. The coordinator waits for results and questions and acks after processing; after consecutive empty waits it enumerates workers and checks them based on liveness, attention, and nextAction.

A successful message enqueue only proves persistent enqueueing; wake/nudge is best-effort and does not prove the other side read or acted on it. An `ask` timeout keeps the original question and recovers using the original message ID. A heartbeat indicates liveness, not completion.

`check-keepalive.ts` periodically writes `_keepalive`/`_heartbeat` to stderr, indicating the wait command is still alive; it does not start a new Agent turn. `coordinator-task-dispatch.ts` has a stale-heartbeat warning, but the warning itself is not automatic recovery either.

What can be borrowed: define states precisely, route anomalies into a persistent inbox, provide recovery paths, and avoid blind resending. This product's "automatically continue when a turn ends normally with work unfinished, and keep supervising even when the controller itself ends its turn" still needs separate end-to-end verification; it cannot be inferred as already working from the heartbeat field.

[Worker contract](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/worker-contract.md), [Wait command keepalive](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/cli/handlers/orchestration/check-keepalive.ts)

**6. Worktree lessons and the Herdr reuse boundary**

Orca distinguishes new Agent terminals, new worktrees, current/existing workspaces, independent top-level worktrees, and stacked child worktrees; repository setup can use a start-immediately or wait-for-setup policy. Remote current/new-child is rejected because its meaning is ambiguous; a precise remote selector is required.

Its guide defaults to being able to use the existing workspace and does not require every worker to create a worktree. Our product is stricter per the user's requirement: parallel code writing defaults to an independent worktree. What we borrow is the full lifecycle of creation/setup/ownership/reclamation, not its shared default.

Herdr already has CLI/API for worktree list/create/open/remove; create parameters include branch/base/path/cwd/workspace; the open operation validates checkout identity, reuses an already-open workspace, and returns workspace/tab/root pane along with worktree info. Deleting a dirty worktree has explicit handling. These primitives should be evaluated and reused directly, then the business layer adds single write authority, task association, acceptance, and cleanup policy.

Basis for Herdr's own capabilities: [worktree schema](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/src/api/schema/worktrees.rs), [worktree/workspace integration](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/src/app/api/worktrees.rs), [Git operations and deletion checks](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/src/worktree.rs). This pass did not operate on the user's existing Herdr sessions.

**7. Concrete constraints for the next implementation step**

Take the [PRD](../../PRD.md) as primary first; stop reverse-engineering user requirements from "which tool looks nice". Implementation discussions must answer each of the following:

1. Which Herdr primitives are reused directly? Who persistently maintains business state?
2. How are Task, attempt, session, worktree, and message receipt associated one-to-one?
3. How do we avoid stale completion, double dispatch, lost acks, and killing on lost contact?
4. What fact does moving/ordering on the board actually change, and does it really affect the queue?
5. Which external process remains responsible after the model ends its turn? How do we prove it is itself still alive?
6. How do we preserve and verify failure artifacts, rather than maintaining busy through endless nagging?

These mechanisms have explicit Orca implementations to reference and should become inputs to our failure acceptance. Whether to port modules directly, reuse another base, or only borrow the design requires checking runtime dependencies and interfaces in the next phase; this pass does not pass off source reading as a completed portability assessment.
