# M6 backlog

## Candidates

- Endless thinking — postponed from M5 (2026-10-28, user: "可以推到 M6 将来再说"): pi has a `thinkingBudgets` setting and extension events with an abort signal, so an extension could stop a turn whose thinking passes N tokens; OpenCode has nothing. GEAR's own remedy would be an interrupt plus a new session. Decide only if a kept model shows it.
- Machine health watch — postponed from M5 (2026-10-28, user: "关于监控的事情，我考虑先放到 M6 里头…我们是不是应该去研发另外一套工具，而不是直接做成 gear 这个工具"): monitoring the machine may be another tool's job rather than GEAR's; this machine's development still needs it. The item as it stood in M5:
  - **Machine health watch (user, 2026-10-26 22:21: a long-term feature; "the ship must not break without us knowing").** M4 evidence: load 46 then 159 on 10 cores; two `find /` scans from an aborted lane turn ran 14+ min; four parallel builds plus another project's Go suite; fseventsd at 100% CPU and 3-4 GB for days; the user felt the lag before GEAR did. The engine samples every minute, as facts, not screen text: — User, 2026-10-27: "在 M5 里，把这个功能加为我们的长期功能".
    - load average vs cores, memory pressure, swap, free disk on the state and worktree volumes;
    - the processes under each seat's pane (by its shell pid): long-running children of a lane whose turn already ended (orphans, e.g. `find /`), builds and test runs per seat, and the top processes outside GEAR (named, never touched);
    - thresholds (kickoff-configurable): a captain item and a board line when load stays above 2× cores for 5 min, memory pressure is critical, disk is low, or an orphan of a lane runs over 10 min.
    - Actions GEAR may take on its own seats only: end an orphan of a finished lane turn; hold new dispatches while the machine is critical (no build lock: running work goes on). Never touch processes outside GEAR; name them to the captain, who tells the user.
    - Recorded in the voyage report: time spent above the thresholds and what caused it.
