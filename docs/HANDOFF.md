# GEAR Project Handover (2026-10-19)

The user has fully copied the project from /Volumes/Data/myai/orch to /Users/roger/myai/gear (actual path /Volumes/Data/myai/gear) and started an Agent in the new directory. From now on the gear directory is authoritative. The copy includes .git, all documents, and historical evidence; there is no product code yet, and GEAR is not deployed.

## Read first

1. docs/PRD.md (v0.7, current requirements baseline, 38 acceptance scenarios).
2. docs/TECH-MVP.md (minimal technical plan).
3. docs/research/2026-10-18/plugin-feasibility.md (plugin mechanism boundaries and four probes).
4. docs/research/2026-10-18/orca-lessons.md (source-code lessons from Orca; reading only the README is not enough).

The old decision.md is a historical candidate, not the current foundation decision; the Firstmate / fixed Claude supervisor recommendation in prior-research.md has been withdrawn. validation.json is an earlier experiment record on candidate tools, not proof that GEAR passed acceptance.

## Settled; do not overturn or keep asking

- Project name GEAR: Goals Executed through Agent Relay. Plugin ID herdr-gear, planned CLI gear; that command is not yet installed.
- The product is a Herdr plugin that solves general multi-Agent orchestration and collaboration and the problem of tasks stopping at the end of a turn with nobody continuing; it is not tightly bound to BMAD. BMAD/Superpowers/other skills are all optional.
- One overall lead directly manages the executors, with no supervisor layer. The controller, executors, and reviewers can each be any Agent CLI with a verified adapter; no fixed brand.
- The plugin adds only one resident process: board, queue, continuous nudging, mailbox, and SQLite in the same process. It must not be split into multiple daemons/UI/watchdogs, systemd is not required, and Board/Dispatch are not chained in. The actual working Agents and short-lived CLI/Git/test processes do not count as additional resident orchestration services.
- The plugin tab hosts the single gear run; switching tabs, hiding the view, or disconnecting the local client does not stop the remote loop. Actually closing that tab terminates the plugin; the next open recovers. Without an external observer, it must not be claimed that it can restart itself or alert instantly. This boundary is written into the PRD.
- SQLite is owned by the plugin and uses HERDR_PLUGIN_STATE_DIR; Herdr's internal state database must not be modified directly.
- worktree is a P0 foundation: parallel write isolation; reassignment requires handing over write rights; session reclamation is separate from code/branch cleanup.
- Initialization is a dialogue: discover the Agents available on the execution host → propose a team plan → the user adjusts the number/type mix of long-lived lanes and temp quota/purposes → put into effect after confirming the version. Mixing Pi/Codex/OpenCode etc. is allowed; executor types are not inferred from the controller type.
- The user mainly connects to the remote Herdr via local Herdr remote to operate Agents. Do not describe it as requiring the user to log in separately via SSH. The execution host/session must be explicitly bound; the machine selected in the UI does not automatically change the CLI context.
- The controller uses a persistent mailbox and a structured global snapshot; handling 1 of 10 messages still keeps the other 9, acknowledged item by item, recoverable after a crash. Global state is not managed through the last chat message.
- Herdr Board and Dispatch are reference implementations only, not runtime dependencies. Orca's encoded failure safeguards must be borrowed: Task/Dispatch separation, no duplicate dispatch when uncertain, message ack/generation, resource ownership, and safe reclamation.

## Next steps (not yet executed)

First do the four minimal hosting probes in plugin-feasibility.md: single instance, in-process UI/loop not blocking each other, remote disconnect continues, exit-and-recover without double start. Then, following TECH-MVP, run a real closed loop with one controller, two heterogeneous long-lived lanes, and one temp reviewer slot. First prove stop-at-turn-end continuation and no message loss, then expand the board and ten lanes. Do not implement the whole PRD at once.

This handover itself only requires receiving and acknowledging it; it does not equal user authorization to immediately change their active Agents, deploy services, or start a full-team experiment. Document reading and readiness checks can be completed; further progress follows user instructions.

## Tool facts and evidence

Local herdr --version is 0.9.1. Plugin startup is a one-shot hook, not a daemon supervisor; there is no general field for automatic skill installation. Remote plugins are installed on the execution host; local configuration/shortcuts are not automatically forwarded as remote actions. Documentation for later versions cannot be taken as capabilities this machine already has.

Temporary source research directories (not project runtime dependencies): /tmp/orch-deep-research, /tmp/orch-orca-research, /tmp/orch-plugin-research. Pinned SHAs and research evidence are in docs/research/2026-10-18. Reading source/tests does not equal real-machine testing. So far the only real operations were opening the PRD locally in File Viewer and moving it to a separate tab.

User preferences: short, definite, actionable; does not accept piling up architecture without verification, does not accept repeatedly fixing a role to a particular Agent, does not accept tight BMAD binding or a pile of background processes. Finish authorized work before reporting, do not stall at planning, and do not falsely claim verification.
