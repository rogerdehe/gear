# GEAR Handover Receipt Acknowledgement

Date received: 2026-10-19. Current project directory: `/Volumes/Data/myai/gear` (`/Users/roger/myai/gear` is the same path); the gear directory is authoritative from now on.

Read `docs/HANDOFF.md` first, then read through `docs/PRD.md` (v0.7, 38 acceptance scenarios), `docs/TECH-MVP.md`, `docs/research/2026-10-18/plugin-feasibility.md`, and `docs/research/2026-10-18/orca-lessons.md`. Read the Herdr skill and confirmed the current environment has `HERDR_ENV=1`.

Confirmed taking on the following constraints:

- The product is the Herdr plugin `herdr-gear`, with planned CLI `gear`. A single plugin tab hosts one resident `gear run`, which in the same process handles the board, queue, nudging, mailbox, SQLite, and request entry; no separate UI daemon or watchdog is added, and Board/Dispatch backends are not chained in. The actual Agents and short-lived commands do not count as additional resident orchestration processes.
- One overall lead directly manages executors; controller, executor, and reviewer are not fixed to any Agent CLI brand. No tight BMAD binding; development skills are optional. Initialization confirms, through a versioned dialogue, each long-lived lane's type and configuration, the temp quota, and its usage rules; the full team is not started before confirmation.
- Parallel code writing uses an independent worktree by default; reassignment must hand over the sole write right; session release, worktree cleanup, and branch deletion are separate, and undelivered results and evidence are kept.
- The first version supports real Herdr remote: the local client operates the target execution host, and processes, files, and persistent state live on the execution host; the execution host/session is bound explicitly, the machine selected in the UI does not replace CLI routing, and the user is not separately required to go through an SSH operating flow.
- SQLite is owned by the plugin, located in `HERDR_PLUGIN_STATE_DIR`, and does not modify Herdr's internal database. The controller recovers from structured snapshots and a persistent mailbox; after handling one of ten items the other nine remain, acknowledged item by item, with support for crash recovery and generation isolation.
- Switching tabs, hiding the view, or disconnecting the local client should not stop the remote loop; actually closing the single plugin tab terminates the orchestration process, and the next open recovers. Without an external observer, no self-restart or instant alerting is promised.
- Board/Dispatch are reference only. From Orca we borrow Task/Dispatch separation, checking uncertain results first instead of redispatching, persistent message ack/generation, operation receipts, resource ownership, and safe reclamation; reading source and tests does not equal real-machine verification.

Currently in the documentation and design baseline stage: there is no GEAR product implementation, deployment, or passed GEAR probe yet. The old decision.md, the withdrawn Firstmate / fixed Claude supervisor recommendation, and the candidate-tool validation.json cannot replace the current baseline or serve as GEAR acceptance evidence.

Next, once the user instructs to proceed, first do the four minimal hosting probes: single instance; in-process UI/requests/loop not blocking each other; real remote disconnect continues; exit-and-recover without double start, and check the disable, uninstall, and upgrade boundaries. After they pass, verify following TECH-MVP a real closed loop with one controller, two heterogeneous long-lived lanes, and one on-demand temp reviewer slot, first proving stop-at-turn-end continuation and no message loss, then expanding to the full board and ten lanes. The specific Agent combination must be confirmed through the initialization flow.

This round only completed reading the handover and writing this acknowledgement file; no team was started, no probes run, no services deployed, and no existing Agents operated.
