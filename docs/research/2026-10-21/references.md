# Reference Projects and Herdr Native Mechanism Verification (2026-10-21)

All source code was studied read-only from pinned snapshots, without live runs in the user's environment (except the 2026-10-18 test records). Full evidence: round9-codex-refs.md, round12-codex.md, round13-codex.md in the session scratchpad; source directories and versions are in [../2026-10-18/sources.lock.json](../2026-10-18/sources.lock.json). The additionally read tasks-axi@9401ff899c0d1d8ae6b4fd8727b9025abda2032c, no-mistakes@7212652ddf416eb385fb171d07140784f322f96c, herdr-kanban@b48a00f9deebac069398c5e8300ccea241c07d73 in round9 are not part of the 10-18 lock; versions and per-file evidence are in section 12 of round9, and they do not pose as the versions of Firstmate's installed dependencies.

## 1. Herdr 0.9.1 plugin mechanism (source /tmp/orch-plugin-research, docs docs/versions/0.9.1)

- Official positioning: "Plugins have no SDK; the whole Herdr CLI is the plugin API"; plugins call Herdr via `HERDR_BIN_PATH`.
- The manifest can declare `build`, `startup` (one-shot, not a daemon), `actions`, `events` (event hooks; each event spawns a short-lived process, global concurrency limit 32), `panes` (ordinary Herdr panes that can host resident processes), `link_handlers`.
- **A plugin action cannot serve as a business interface for Agents**: the call parameters are only action_id / plugin_id / context, and no body can be passed (`api/schema/plugins.rs:306`); the command is started in a thread and immediately returns a Running log; output is truncated at 64KiB and goes only to the plugin log (`app/api/plugins/runtime.rs`). Suitable for user shortcut entry points.
- The runtime injects `HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_PLUGIN_STATE_DIR`, `HERDR_PLUGIN_CONFIG_DIR`, `HERDR_PLUGIN_CONTEXT_JSON`, and the workspace/tab/pane IDs when available.
- No plugin storage API; no declarable persistent scheduled tasks.
- Events: the socket API has `events.subscribe/wait`; in newer source, hookable events include pane.agent_status_changed / pane.exited / pane.agent_detected etc.; actual 0.9.1 behavior pending a probe.
- metadata token key names are limited to `[A-Za-z0-9_-]{1,32}` and must carry a source.
- `agent prompt` = write text into the pane terminal (following bracketed paste) + Enter; messages are not stored.
- Command count (subcommand paths listed by the local `herdr <group>` help) is about 84, of which agent 12, pane 28, plugin 11.

## 2. Firstmate (kunchenguid/firstmate @45dd50a) -- the most important comparable reference

**What it is**: self-described as an "Agent distribution". The cloned directory is the product: `AGENTS.md` (a 433-line primary-agent contract) + 29 skills + about 210 .sh files across the whole repo excluding tests/.git, plus TypeScript (26 files, about 10k lines, Pi/omp extensions, OpenCode plugin), JS/MJS (about 5k lines), Python (about 4.7k lines). The source extensions total about 140k physical lines (including blank lines, comments, and embedded text; not effective lines of code or runtime resource consumption). Launch a supported CLI in that directory and it becomes the primary agent (first mate).

**How it runs**: there is no unified service. A zero-token bash watcher observes the fleet, classifies events, and wakes the primary agent only for what needs action; how the watcher is re-armed round after round depends on the primary agent CLI (Claude Stop hook, Cursor stop hook, OpenCode plugin, Pi/omp extension, Codex foreground checkpoint, Grok background call). The host, away daemon, or extension branch is chosen by harness and run posture; not all listed backgrounds necessarily start at the same time; remote secondmates additionally have a job worker. Core state is persisted as files, directories, and logs; with external backends/tools it cannot be uniformly called database-free. Watcher classification does not call a model; subsequent supervision judgments may call a model.

**Primary agent support**: Claude Code, Grok, Pi are jointly the top recommendations; omp, Codex, OpenCode, Cursor have also been verified. Supervision depth varies by brand: when attended, only Claude/Cursor have the supervision host take over routine events; Codex/OpenCode/omp/Grok are passed through as-is to the primary agent; Pi uses an in-process branch.

**Against the capabilities we want**:

| Requirement | Firstmate |
|---|---|
| Know the result immediately when sending a message | Yes: returns after writing to the inbox, the doorbell reports separately (remote/typed paths have "unconfirmed" results) |
| Not tied to BMAD | Yes |
| Multiple projects | Yes: one primary agent manages multiple projects; independent primary agents per project need multiple homes |
| Temp worker on/off | Partial: task-type worker spawn/teardown; project capacity is an optional per-project cap, without distinguishing long-lived/temporary |
| Continue after turn stops | Partial: the watcher filters and aggregates; actionable wakes go to the primary agent or the corresponding supervision engine. For Codex attended, the host is pass-through and actionable wakes are still handled by the primary agent; this does not mean all raw events wake the primary agent. |
| Captain not flooded | Partial: persistent records, filtering, merging, grouped drain; not yet proven to have GEAR's full contract of thread prior context and fair bounded read order. |
| Periodic review | Partial: heartbeat starting at 600s, idle backoff up to 7200s, no-change can be absorbed; on receiving a heartbeat it requires a whole-fleet re-check and backlog update. Not proven that GEAR's five trigger reasons and the persistent-conclusion loop all exist. |
| Live plan queue-jumping | Partial: backlog, dependencies, hold; precise queue-jumping not proven |
| See output / drift | Partial: worktree write probing, verification and delivery evidence, primary-agent checks; not proven to have generic automatic semantic drift detection. |
| Long-lived lanes and idling | Partial: secondmates are persistent and healthy when idle, but they are subordinate primary agents that can dispatch others, not peer lanes |
| Whom to ask about dependencies | Reported to firstmate for coordination by default; has task-addressed sending, but not proven to provide a protocol for workers to autonomously discover and handle lateral dependencies. |
| Captain swap | Manually switching to a supported CLI and recovering from disk has a foundation; automatic cross-brand takeover on generic quota exhaustion not proven. |
| resume | No generic command; some backends have session references that can participate in recovery |
| Herdr plugin, single process, lightweight | No: Herdr is just one of the backends, multiple background components, many dependent tools |

**2026-10-18 local tests**: multiple key suites failed or timed out (watch-triage, wake-queue, checkpoint, dispatch-resolve, crew-state), see [../2026-10-18/validation.json](../2026-10-18/validation.json). This cannot be used to say it is unreliable, but no reliability evidence was obtained locally. Live agents were not used at the time; for Herdr 0.9.1 only the client version was checked; the 5 passing event-wait items are parser unit tests, not real Herdr/remote E2E. The root causes of the failures were not all established, and timeouts do not mean the full suite passed.

The recommendation was withdrawn earlier mainly because the suggestions to fix the primary agent brand to get a more complete supervision path and to adopt a supervisor layer contradicted the user's requirements; Firstmate itself does not only allow Claude or force secondmates. Supervision behavior is not equivalent across harnesses, and leveling out the differences involves its supervision architecture; the later-settled single-process plugin requirement also constitutes a deployment mismatch. Unclosed tests add verification risk, but are not evidence that "it does not have these capabilities". GEAR has still not proven itself simpler or more reliable.

**Trade-off**: do not adopt directly, do not fork; **borrow heavily** from the following designs:

1. Report "persistently stored" and "doorbell result" separately for sends; on lost receipts, reconcile by the original correlation ID, do not resend (`bin/fm-send.sh`).
2. The watcher first classifies and merges in the program (adjacent events merged within 30 seconds, benign events absorbed), handing only what needs action to the primary agent (`bin/fm-watch.sh`, `bin/fm-classify-lib.sh`).
3. Bounded worktree write probing serves only as a clue for "not stuck"; with no evidence, keep the original cadence (`crew_worktree_written_since`).
4. Declarative waiting `paused: ... until <time>` re-evaluates against the original record when due -- corresponds to our park.
5. When the wake queue drains, it is presented grouped as OPEN DECISIONS / UNREAD STATUS etc. (`bin/fm-wake-drain.sh`) -- corresponds to the captain home page.
6. Worker status vocabulary: working / needs-decision / blocked / paused / done / failed, requiring reporting less and only reporting actionable changes (`bin/fm-brief.sh`) -- corresponds to `gear send --kind`.
7. Before reclaiming, check landed results and dirty trees; if closing fails, keep a pending-close record; relaunch keeps the old records via a journal (`bin/fm-teardown.sh`, `docs/agent-control.md`).
8. Session lock + new session reads the persistent summary and reconciles (`bin/fm-session-start.sh`) -- corresponds to captain swap.

**Differences (why it is not the final form)**: what we want is a single process inside a Herdr plugin, a primary agent that is brand-agnostic with consistent behavior, peer long-lived lanes + temp worker quota, live plan queue-jumping, engine-scheduled reviews, a threaded inbox, and lightweight.

## 3. Other references

| Project | Messages/commands | Borrow | Avoid |
|---|---|---|---|
| Orca | send=persistent enqueue; ask blocks awaiting a reply; reply; Task/Dispatch separated | When the result is uncertain keep the original attempt and do not re-dispatch; three-way settle/retain/reclaim | Treating a batch ack as per-item handling; treating heartbeat as continuation |
| herdr-tasks | Own CLI/MCP calling a daemon, synchronous return; 34 actions | A single action registry; list summary/get details | Treating lease expiry or pane gone as safe to reassign |
| herdr-dispatch | 8 actions; pure Decide → side effects | Separating judgment from side effects; fair project rotation | Unbounded doing+idle continuation count; a separate background process |
| herdr-board | Workers only use comment/done; 57 management commands | Very few executor commands; done≠business success | Stopped without submitting results only marks Awaiting without continuing; UI separated from daemon |
| herdr-workboard | CLI modifies files directly, calls Herdr synchronously | request_id same-params replay, different-params rejection | Guessing the focused workspace when there is no context |
| herdr-kanban | Uses `agent prompt` directly, no inbox | Reusing Herdr metadata | Avoid treating lookup/reuse of the primary agent by name directly as GEAR's project binding and generation isolation; this round did not prove it has our multi-project multi-captain contract |
| bmad-loop | Hooks report events; serial (max_parallel=1) | The nudge budget and activity evidence for "stopped without submitting results" | Hook failure still returns 0 |
| no-mistakes | Agents use 8 commands + home | home gives the current state and next-step commands | Treating a droppable event buffer as a business mailbox |
| tasks-axi | Directly modifies storage and returns actual records | Returns actual state immediately after modification; ready and held separated | Treating a date hold as something that will start a model |
| /Users/roger/code/skills (BMAD-Team etc.) | GitHub PR comments as the message bus | Message format (question/options/background), review rules, priority vocabulary now/must/should/could/wont | Multiple questions on the same story all judged answered by one reply; approvals not bound to versions |

Gaps: peripheral projects such as Jev, File Viewer, gh-axi have no pinned source and were not audited.
