# GEAR Minimal Implementation Technical Plan

Basis: [PRD v0.7](PRD.md). Status: to be implemented; this document decides the first runnable slice and does not claim hands-on validation has passed.

Project name: GEAR; plugin ID: `herdr-gear`; CLI: `gear`. These are proposed names; the corresponding commands are not installed yet. Acronym: **G**oals **E**xecuted through **A**gent **R**elay (completing goals through Agent relay).

**1. What to deliver first**

One project, one Herdr execution host, one controller, two heterogeneous long-lived lanes, and one on-demand temporary reviewer slot. Types are confirmed in the initialization conversation; role brands are not fixed. Get this working end to end: confirm the team → dispatch work into isolated worktrees → continue after a mid-run stop → handle issues through the mailbox → verify → next task/reclaim.

After the first slice passes, expand to the PRD's five CLIs, ten lanes, and the full interactive board. The first version has a terminal table, details, and adjustment commands first; do not cram the entire PRD into the first implementation.

**2. Explicit implementation choices**

Build a Herdr plugin whose core is one resident `gear` process: Python 3 standard library, SQLite, Unix socket. Board rendering, queue scheduling, automatic continuation, mailbox, and state persistence all live in the same process; do not split into board/controller/daemon/watchdog, and do not start another plugin to act as a background process. The first slice manages only one project.

Herdr Board and Herdr Dispatch serve only as implementation references; they are not installed as dependencies, and their databases or background processes are not chained in. Specific references and limits are in [Plugin mechanism verification](research/2026-10-18/plugin-feasibility.md).

| Reuse directly | Minimal parts we add ourselves |
|---|---|
| Herdr remote, session, pane, Agent start/get/prompt/read | Task queue, execution attempts, continuation rules |
| Herdr worktree create/open/remove | Write ownership, verification, preserving output |
| Herdr plugin pane entry point | Terminal table inside the same `gear run` process |
| Orca's Task/Dispatch, persistent mailbox, and unknown-state anti-redispatch designs | Concrete tables, transactions, and failure tests; do not port Orca's GUI/runtime |

The skill only teaches Agents to use `gear` commands, submit results, and handle the inbox; the background program carries continuous execution. Neither BMAD nor Superpowers is a runtime dependency.

**3. Where it lives, how remote is used**

```text
Local Herdr client ── Herdr remote ── Target Herdr session
                                      ├─ Controller / executors / temporary reviewer
                                      └─ Plugin tab: one gear run process
                                          ├─ Board and keyboard input
                                          ├─ Queue / nudging / mailbox
                                          └─ SQLite / local request entry point
```

`gear` is installed on the project execution host; SQLite lives under `HERDR_PLUGIN_STATE_DIR`, separated per project, and configuration goes in CONFIG_DIR. The resident process is the sole writer. The `gear` CLI that Agents call is a short-lived request client: it sends a request to the Unix socket and exits, with no resident loop. Herdr CLI, Git, and test commands may be worker subprocesses with bounded concurrency; the actual executing Agents are still managed by Herdr. The "one process" constraint here means the plugin adds only one resident orchestration process, not that the whole development environment has only one OS process.

Use one event loop to multiplex terminal input, socket requests, periodic checks, and UI refresh. No keyboard input, the user sitting on a detail page, or the UI temporarily not rendering must not block scheduling; long-running Herdr/Git/verification calls are awaited asynchronously with timeouts and are never executed synchronously in the draw path. Database transactions are short and executed centrally. The socket is restricted to the same OS user; it does not provide security isolation against a malicious Agent running under the same account.

The plugin action only starts or focuses the existing `gear run` tab; a single-instance lock prevents duplicate launches, and no second launcher stays resident. Explicitly bind the execution host and session, and persist resource identities. No systemd/launchd, HTTP service, or extra network protocol is required; once installed on the target host, the same tab is viewed via local Herdr remote.

**Lifecycle boundary:** when the user switches away from the tab, hides the board within the plugin, or disconnects the local client, the remote `gear run` continues; no separate long-running board client is opened. Actually closing this single plugin tab terminates the control process; we cannot also promise that "closing the tab hosting the process still continues". The plugin provides an explicit stop operation; an ordinary exit from the board only exits the view, not the process. A native Herdr forced tab close cannot be guaranteed to be intercepted by the plugin; after an unexpected termination, the next open entry point checks and recovers, records are not lost, but there is no promise of self-restart or proactive alerting without an external observer.

Do the minimal plugin hosting probe first, then the task features; a failed single-process probe must not be solved by splitting out a second background process.

**4. Initialization has only four steps**

1. On the target execution host, read Herdr's support information and detect CLI paths and configuration; mark supported, installed, and verified separately. When Herdr has no ready-made install list, fill in with read-only probing.
2. The controller proposes a team draft: number/types/responsibilities of long-lived lanes, temporary quota and type rules, model configuration, worktree baseline, and budget.
3. The user adjusts it in conversation; `gear init` saves a versioned draft and does not spin up the team early.
4. After that version is confirmed, `gear apply` lands it idempotently. The controller may register existing sessions; long-lived lanes execute only when they have tasks, and temporary reviewers start on demand. Failed items keep their reasons, and retries do not re-create items that succeeded.

**5. Minimal data and commands**

SQLite stores only six groups of records: `project` (config/version), `lanes` (type/capacity/session), `tasks` (contract/queue/dependencies/acceptance), `attempts` (generation/worktree/status/evidence), `messages` (recipient/handling status/deadline), `operations` (side-effect intent/receipt). Dependencies are lists of task IDs, and every change is validated as acyclic in a transaction by the resident process. Worktree write rights and active attempts have uniqueness constraints; actual paths are normalized before checking for aliases.

The minimal command surface is below; all are proposed interfaces, not existing Herdr commands:

| Command | Purpose |
|---|---|
| `gear init / apply` | Team draft, confirm configuration, and land it |
| `gear task add / show / move / pause / resume` | Create tasks, view, reorder or reassign, pause and resume |
| `gear status --json`, built-in board in `gear run` | The same state snapshot; the table shows current/next task, blocks, and inbox backlog |
| `gear inbox pull / resolve / defer`, `gear send` | Bounded batch claim, record outcome or wait condition per item, lateral communication |
| `gear report progress / blocked / result` | Executor reports; must carry task, attempt, and generation |

Mutating commands carry a request ID and expected version. Write the side-effect intent first, then call Herdr, then save the receipt; after a resident-process crash or call timeout, reconcile against actual resources, and do not pretend to be "exactly once" by resending. When it cannot be confirmed, freeze that attempt and escalate, without blocking unrelated tasks.

**6. Core loop: truly guaranteeing someone keeps managing it**

Reconcile every 5 seconds initially, using polling at small scale first; the interval is configurable and does not constitute a timeliness guarantee. Herdr calls have timeouts and run with bounded concurrency; one blocked prompt/wait must not stall the whole project.

| Observation | Program action |
|---|---|
| Ready task, idle lane, dependencies and capacity satisfied | Claim atomically → prepare an isolated worktree → start/bind a session → send the task packet |
| Not finished, previous turn confirmed ended, currently idle/done | Continue the same task, preserving output; an attempt has only one pending-confirmation prompt at a time |
| working | Do not nudge; record evidence. When no progress persists until the budget is reached, file a diagnostic item; do not kill processes based on silence duration |
| blocked / awaiting authorization / rate-limited / explicitly paused | Keep the reason; wait for the corresponding condition, do not loop sending "continue" |
| Input result unclear, prompt_stalled, disconnected, or unknown | First reconcile Herdr state with the current attempt's report; do not immediately resend or double-launch |
| Executor submits result | Pin the candidate commit → run the verification commands in the contract → start a new session to review when needed |
| Verification passes and review conditions are met | Mark the task done and unlock downstream; long-lived lanes take the next item, temporary sessions are reclaimed per ownership |
| Fix failures reach the budget | Preserve output and evidence, and file one continuously tracked issue into the controller's mailbox |

Herdr's idle/done is not proof of completion; `prompt --wait` also does not mark an independent task turn. The adapter layer must distinguish submitted, observed started, turn stopped, and delivery unclear. Initial state and stale snapshots must not trigger continuation. The first hands-on gate is proving exactly this; a CLI whose observation is unreliable is provisionally marked as not passing integration and must not be advertised as supported.

By default the same problem is auto-fixed at most 3 times, with a separate configurable time limit; the continuation limit and the fix count are separate. Progress evidence includes new artifacts/commits, verification results, and concrete progress reports; mere terminal output does not reset the no-progress budget. All automatic actions stay within the scope approved at initialization.

Freeze the candidate code during verification; the minimal version integrates passing candidates in a single order and verifies the integrated result. Conflicts go to the controller. Reclaim sessions only when there is no active writer; the first version does not automatically delete worktrees or branches, and preserving output takes priority.

**7. The controller does not rely on chat to remember ten messages**

Every item requiring a decision first enters the persistent mailbox, then wakes the controller in a merged fashion. One pull takes at most 5 items, FIFO within the same priority, and items that have waited too long are raised in priority. Each item is individually `resolve`d and must carry a handling result/operation receipt; seeing or claiming does not count as handling. Handle one, and the other nine remain.

While the controller is busy, do not inject new prompts back to back; wake it again when its turn has stopped and there are still items to handle. After a claim expires or a session is replaced, reconcile and recover; an old generation cannot acknowledge new items; `defer` must carry the awaited object and wake condition. Ordinary progress is written to the board, without forcing the controller to reply item by item.

Each turn, the controller first reads the snapshot and inbox. Initially, every 5 minutes a mergeable global review item is generated; when the queue is about to empty or there is a critical block, an item is put in the box immediately. Configured thresholds monitor the oldest wait time and handling progress, and a persistent backlog shows an alert to the user. The user can query at any time; the model does not need to think continuously.

**8. Write in this order, accept by these results**

| Order | Minimal delivery | Pass condition |
|---|---|---|
| 1: Herdr hands-on probe | remote binding, worktree, start/input/observe for two user-chosen CLIs | Two lanes editing a same-named file do not conflict; after a deliberately stopped turn, the external program can start work again; a start of unclear outcome is not treated as redispatchable |
| 2: Execution loop | SQLite, single-process loop, queue, verification, recovery | Two real small tasks delivered without manual nudging; after killing the plugin process, recovery does not double-launch; disconnecting the local client does not stop remote progress |
| 3: Collaboration loop | Controller mailbox, initialization conversation, temporary reviewer, terminal table | After handling only one of ten decision messages, nine remain; nothing lost on a mid-run restart; temporary reviewer created on demand with results preserved; the loop still holds after switching the controller's CLI |

First pass the [four hosting probes in the plugin mechanism verification](research/2026-10-18/plugin-feasibility.md), then execute the table above. If step 1 fails, fix the integration; do not build the full TUI first. Only after step 3 passes, refine the full keyboard board and expand to ten lanes. The full PRD remains the subsequent acceptance baseline; the reductions above are the first slice's scope and do not delete the original requirements.

Basis: [Herdr verified version and source](research/2026-10-18/sources.lock.json), [Orca concrete failure safeguards](research/2026-10-18/orca-lessons.md). The target execution host's actual Herdr/CLI versions, state observation, and plugin deployment still need hands-on verification in step 1; this document did not perform deployment or take over existing Agents.
