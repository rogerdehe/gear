# GEAR MVP Technical Plan

> **Status 2026-10-22.** The walking skeleton described here runs end to end on a real Herdr (Rust, not Python). Further work follows [PRD v0.9](PRD.md) and is driven by GEAR itself; this document is kept as history.

Basis: [PRD v0.8](PRD.md). Status: to be implemented; this document is the skeleton and effort estimate for the first landing step and does not claim real-machine verification. Previous version: [archive/TECH-MVP-v0.7.md](archive/TECH-MVP-v0.7.md).

## 1. What the MVP must achieve

**One complete minimal chain** that a running project can use: safely change the plan, drive lanes to work, handle messages, check output, and confirm delivery.

Principles:

- What is cut is **generality, degree of automation, and interface**; three bottom lines are not cut — no double writes, no lost results, never treating unknown as success.
- **The normal path advances automatically; the abnormal path stops and hands over to a human to check**; no piling up detection systems for full automation.
- Product code target ≤ 3000 lines (Python standard library, zero third-party runtime dependencies, excluding tests); when approaching the limit, cut optional automation and presentation first.
- Counted as maintainable product physical lines, including normal blank lines and comments; tests counted separately. Necessary shell/embedded scripts are listed separately; product logic must not be hidden in templates, tests, or overlong lines to get around the budget. "Zero third-party runtime dependencies" means no additional third-party Python packages are introduced; it still depends on installed Herdr, Git, and supported Agent CLIs.

## 2. Scope

### 2.1 MVP must do

| Capability | MVP achieves |
|---|---|
| State | SQLite as single authority; short transactions, request_id replay, generation and version checks |
| Plan and tasks | Plan draft in a strict small format → `gear plan apply` validates and applies by version; generates a bounded PLAN.md and task packets; queue jumps, reordering, manual refinement; awaiting verification kept separate from done; done requires evidence. The strict draft may only modify task contracts, ready order, staffing targets, and acceptance references; runtime observations, execution attempts, message handling, and actual write rights cannot be arbitrarily overwritten by the draft. In-flight contract switches and queue jumps follow PRD §6; changing the owner in the draft does not mean the handover is complete |
| Continuation | Poll Herdr; three kinds of action kept separate; single delivery that needs check, cooldown, budget; rate limit/quota/authentication/blocked/unknown handled separately; fixed screen-reading signatures for verified CLI configurations |
| Messages | `gear send` writes synchronously into the target inbox; threaded inbox (urgent first, reserved slot for the oldest ordinary item, per-sender cap, prior-context expansion); `gear reply` handles/waits item by item; replies go into the other party's inbox |
| Doorbell | One-line reminder when the recipient is idle; single unconfirmed doorbell |
| Dependencies | `--about <upstream task>` routes to the current owner, everything else goes to the captain; re-evaluate after upstream verification/delivery |
| Captain rhythm | Bounded briefing packet; five review triggers, same-cause merging, checkpoint expiry; conclusions via `plan apply`; notify the user after limited reminders; the captain is also supervised when stopped |
| Long-lived lanes | Standby (silent, timed, included in the review snapshot); park sets a deadline or a supported resume condition via the plan draft and re-evaluates on expiry (no separate park command, no arbitrary condition language); hold is not interrupted by new tasks; each lane's single long-term worktree is created once at onboarding via `herdr worktree create --branch home/<lane> --base dev --label <project>-<lane>`, task branches are switched inside it (see [design/17-conventions.md](design/17-conventions.md)) |
| Temp workers | Quota; for each new task the engine creates the temp worker's own worktree and pane via `herdr worktree create` and starts it with a fixed confirmed configuration; for rework/resume it reopens with `herdr worktree open --branch <task-branch>`; after the result is merged, `herdr worktree remove` (removes checkout + workspace, keeps the branch); automatic release only follows the PRD §9 normal-delivery safe path, and quota is released after the close is confirmed; when the same work needs rework/follow-up, resume the original session, and if unavailable a new session reads the task packet and is marked; new work opens a new temp worker; write rights are checked separately |
| Retirement/lost contact/unavailable | Roster plan vs. actual difference; stop dispatching; in-flight frozen; write-right anomalies go to manual checking; when the captain is unavailable: freeze, invalidate old generation, notify the user |
| Output | Periodic git snapshots (HEAD, working-tree changes, evidence files); missing evidence/longest-unreviewed creates "needs check"; the captain looks at the diff and registers a judgment |
| Multiple projects | Registration of multiple projects on the same execution host/session, routing isolation, fairness between projects |
| Identity | Managed Agents are all started and onboarded by GEAR and inherit a fixed call context `GEAR_CTX` (seat + generation); existing sessions must be restarted and onboarded by GEAR (supported CLIs use resume to keep context); unclear binding pauses that lane (design/03) |
| Visibility | `gear` home page (by role); text board in the plugin pane; pane title shows role and task |
| Runtime | Plugin manifest: pane hosts the single engine + an open-entry action; single-instance lock, heartbeat; TOML configuration (no hand-written worktree paths); real remote verification |
| Migration | Inventory the old keeper's actual instances, scopes, and running versions, keeping hold/waiting/in-flight state; explicitly hand over supervision, refuse to take over if overlapping controllers are found; stop the old before starting the new, and provide a checkable rollback path |

The MVP forbids managed tasks from proactively creating unregistered detached write jobs; no process-tree scanning. When onboarding historical sessions, it cannot be assumed they obeyed this restriction; abnormal write resources stay at needs check.

Worktrees and panes are created through Herdr's worktree API (`herdr worktree create/open/remove`); configuration contains no worktree paths. Default branch model: `main` (release), optional `staging`, `dev` (integration), task branches `<lane>/<task-id>-<slug>` from `dev`. See [design/17-conventions.md](design/17-conventions.md).

### 2.2 MVP deferred (in the final form, not in the first step)

| Deferred item | MVP alternative |
|---|---|
| Automatic transfer of write rights in abnormal situations | Mark "needs check", transfer after manual confirmation |
| Event subscription / `[[events]]` hooks | Polling + reconciliation; used as a latency optimization after probes pass |
| Arbitrary Markdown parsing, semantic diff, archive documents | One strict small format; archive only queried in the database |
| Automatic execution of standing rulings, lateral collaboration protocol | Sent to the captain |
| Automatic scheduled captain rotation, cross-execution-host, lanes shared across projects | Not done |
| Full TUI, metadata polish, complex audit filtering | Text home page and text board |
| Initialization dialogue | Static TOML carries the roles, CLI types, and configuration the user has already confirmed lane by lane, and shows the actual effective values; missing or changed items are confirmed explicitly first, never guessed silently from available models or pane names; the user is not required to re-confirm existing decisions |

**User decisions 2026-10-21**:
- Temp worker resume is included in the MVP: new work opens a new temp worker; when the same work needs it back, resume the original session; if resume is unavailable, fall back to a new session and mark it. Implementation in [design/10-staff.md](design/10-staff.md).
- Replacing the captain is switched only manually by the user; the MVP does no automatic switching of any kind; when the captain is unavailable: freeze, show, notify. Implementation in [design/03-identity-seats.md](design/03-identity-seats.md).

Module-level technical design is in [design/](design/00-overview.md); the code budget follows the current per-module budget in design/00-overview.md, and this document does not maintain separate numbers.

## 3. Architecture

```text
Local Herdr client ── Herdr remote ── Target execution host Herdr session
                                       ├─ Captain / long-lived lanes / temp workers (each in its own pane)
                                       │     └─ call gear (short-lived command) ──┐
                                       └─ Plugin pane: gear run (single engine)   │
                                             ├─ Poll Herdr, continuation, doorbell│
                                             ├─ Review scheduling, output sampling│
                                             ├─ Herdr side effects for temp/staff │
                                             └─ Text board                        │
                                       SQLite (HERDR_PLUGIN_STATE_DIR) ◀──────────┘
```

- **gear commands**: write SQLite directly in short transactions and return the actual result on the spot; only pure data changes.
- **Engine**: the only process that executes Herdr side effects; every side effect records intent first, then executes, then records the result; an unclear result stays unknown.
- **Herdr adapter layer**: a single wrapper that calls the CLI via `HERDR_BIN_PATH`, with unified timeouts and error classification; calls have a concurrency cap, and slow calls do not block other projects.

## 4. Data (minimal tables)

| Table | Contents |
|---|---|
| projects | Project, bound execution host/session, configuration version, quota |
| seats | Role seats: project, role (captain/long-lived lane/temp worker), pane, session reference, generation, status (working/standby/park/hold/retiring/lost contact/needs check) |
| plans | Plan version, draft text, apply result |
| tasks | Contract (versioned), order, status, owning lane, dependencies, evidence point, priority |
| attempts | Execution attempts of a task, generation, candidate version, result |
| messages | Thread, sender, recipient, type, urgency, body, status, handling result, waiting-on |
| operations | Side-effect intent/result/receipt (delivery, start, close, doorbell) |
| reviews | Review items: trigger reason, persistence info, decision, next checkpoint |
| samples | Output snapshots: HEAD, working-tree summary, evidence files, time |
| requests | request_id → result (replay) |

## 5. Return contracts of the four commands

| Command | Completed on the spot and returned | Does not mean |
|---|---|---|
| `gear` | Bounded view by role; closes no items | — |
| `gear send` | Written into the target inbox + message ID; doorbell status reported separately | The recipient has read or handled it |
| `gear reply` | The listed messages have a recorded result (closed or waiting), and the reply is in the other party's inbox | Task complete, write rights transferred |
| `gear plan apply` | New plan version in effect + list of pending execution/handover items | Temp worker started, retirement complete (shown on the home page after the engine executes) |

All failures give a reason on the spot; a receipt lost after commit is queried back with the same request_id.

## 6. Engine loop (one round every 3–5 seconds, configurable)

1. Read `herdr agent list` once and reconcile with registered seats (missing, identity changes, status). An observation failure is marked for this round; exit is not inferred.
2. For each lane: decide among the three kinds of action → deliver a task pointer when needed (respecting single delivery that needs check, cooldown, budget).
3. Doorbell: recipient idle, has pending items, no unconfirmed doorbell → remind. Dispatch, continuation, and doorbell jointly respect the per-lane single delivery that needs check; they must not double-deliver in the same round just because each category has no record of its own.
4. Plan and staffing: execute the pending items produced by `plan apply` (open/close temp workers including their Herdr worktrees, park expiry, retirement handover), checking version and actual state first; stale actions are not carried out.
5. Timed items: review trigger detection and merging; output sampling (lower frequency, e.g. every 20 minutes); reminder budget.
6. Write heartbeat; refresh the board and pane titles.

## 7. Code budget

Follows the per-module budget in [design/00-overview.md](design/00-overview.md): currently **2,510–3,140 lines**, exceeding the 3000-line budget; during implementation, compress presentation first, not behavior; if still over, return to the user with the numbers for a decision.

History: round11's independent estimate of the cumulative scope at that time was 3,820–5,900 lines (4,400–7,400 lines including rework); round11's restricted slice (excluding resume) was 2,230–2,860 lines. Both have been superseded by the current design and are kept for comparison only.

## 8. Tests

See [design/15-tests-probes.md](design/15-tests-probes.md), about 1,700–2,700 lines.

## 9. Probes first (1–2 days, can run in parallel with development)

1. Plugin hosting: single instance; switching tabs/hiding the view does not stop the loop; after closing the tab, reopening recovers without double start; disable/uninstall boundaries.
2. Real remote: local client disconnects and reconnects, the target loop is continuous, files are all on the target.
3. Identity: changes on pane move, Agent replacement, new session of the same CLI, missing session reference.
4. Each CLI actually used: reliability of state observation (whether it falsely reports idle while a tool is running), rate-limit/quota signatures, behavior when a prompt is accepted but not started.
5. Metadata key rules and display in the UI.
6. Resume capability of each CLI (obtaining session_ref, resume parameters, determining return to the original session) — currently mandatory.

If a probe fails, narrow the scenario or fix the onboarding; do not build the interface first.

Internal prototypes can be developed in an isolated plain pane, but before taking over a running project, minimal plugin hosting, real remote binding and recovery checks must pass, and exclusive migration from the old keeper must be complete; without passing these, it is not called an MVP that can onboard projects.

## 10. Landing order and effort

| Step | Content | Effort |
|---|---|---|
| 1 | Probes | 1–2 days (parallel) |
| 2 | Implementation + tests + stage review | 3–5 Agent working days (6–8 hours of effective development per day) |
| 3 | Templates and instructions, onboarding inventory and migration/rollback checks: task packet format, plan draft format, lane instructions, captain instructions, instructions for the 4 commands | Active non-code work totaling about 1.5–3 working days, can interleave with other steps |
| 4 | Onboard some lanes of one real project, deliberately creating queue jumps, waits, temp worker release, reviews, restarts | At least 2–3 days elapsed time |
| Total | | About 6–10 days elapsed time |

The above is a design estimate assuming a limited set of verified CLIs, Herdr's worktree API, an available remote environment, and quota are already in place; it is not a delivery commitment; re-estimate when probes fail, external waits occur, or scope is restored.

## 11. Main risks

1. **Whether Herdr / CLI state observation is reliable**: tool still running but showing idle, input accepted but not started, missing session reference — directly affects continuation and release; if probes fail, narrow the scenarios.
2. **Old on-site state**: old keepers, dirty trees, detached jobs, old captain actions not in the database — how much to take over can only be decided manually.
3. **Whether the plan draft format is usable**: too complex shifts the cost onto the captain every round; needs validation in real turns; the format may need rework.

## 12. Implementation points borrowed from reference projects

Details in [references.md](research/2026-10-21/references.md). Those that directly affect the MVP implementation:

- Report "stored" and "doorbell result" separately for sends (Firstmate `fm-send`, Orca `send`).
- During observation, merge adjacent events and absorb benign events, passing only actionable ones to the captain (Firstmate watcher classification and 30-second merge window).
- File writes serve only as a clue for "not stuck" and must be gathered in a bounded traversal; with no evidence, keep the original escalation rhythm (Firstmate bounded worktree probe).
- Declarative waits ("paused until some time") re-evaluate against the original record when the time comes (Firstmate `paused: ... until`) — corresponds to park.
- Before reclamation, check landed results and dirty trees; on close failure, keep a pending-close record (Firstmate `fm-teardown`).
- When the result is uncertain, keep the original attempt and do not redispatch (Orca uncertain dispatch).
- Separate "judgment" from "side effects" (herdr-dispatch pure Decide layer).
- Executor commands are minimal (herdr-board worker uses only comment/done); lists give summaries, details fetched separately (herdr-tasks).
