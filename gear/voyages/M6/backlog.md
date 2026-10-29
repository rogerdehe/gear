# M6 backlog

## Candidates

- Endless thinking — postponed from M5 (2026-10-28, user): pi has a `thinkingBudgets` setting and extension events with an abort signal, so an extension could stop a turn whose thinking passes N tokens; OpenCode has nothing. GEAR's own remedy would be an interrupt plus a new session. Decide only if a kept model shows it.
- Machine health watch — postponed from M5 (2026-10-28, user): monitoring the machine may be another tool's job rather than GEAR's; this machine's development still needs it. The item as it stood in M5:
  - **Machine health watch.** M4 evidence: load 46 then 159 on 10 cores; two `find /` scans from an aborted lane turn ran 14+ min; four parallel builds plus another project's Go suite; fseventsd at 100% CPU and 3-4 GB for days; the user felt the lag before GEAR did. The engine samples every minute, as facts, not screen text: — User, 2026-10-27.
    - load average vs cores, memory pressure, swap, free disk on the state and worktree volumes;
    - the processes under each seat's pane (by its shell pid): long-running children of a lane whose turn already ended (orphans, e.g. `find /`), builds and test runs per seat, and the top processes outside GEAR (named, never touched);
    - thresholds (kickoff-configurable): a captain item and a board line when load stays above 2× cores for 5 min, memory pressure is critical, disk is low, or an orphan of a lane runs over 10 min.
    - Actions GEAR may take on its own seats only: end an orphan of a finished lane turn; hold new dispatches while the machine is critical (no build lock: running work goes on). Never touch processes outside GEAR; name them to the captain, who tells the user.
    - Recorded in the voyage report: time spent above the thresholds and what caused it.
- Queue order check: a later milestone's task ranked ahead of an open one of the current milestone (postponed from M2 #16; 7 voyages). — postponed from M5 (2026-10-28, user; captain's reason to drop: voyages now sail one at a time, so no other voyage's task sits in the queue).
- Drop the store's copy of a review after its task merges; git keeps the record (postponed from M2 #26; 7 voyages). — postponed from M5 (2026-10-28, user; captain's reason to drop: the store is a few MB and never caused trouble).
- Live checks never recorded: T-69 ends the branch-held loop; a reused reviewer gets no earlier job's notices (B02-02) (postponed from M3.1 #2 and #8; 4 voyages). — postponed from M5 (2026-10-28, user; captain's reason to drop: M4 rebuilt the seat model; those paths are gone and the cases did not recur in M4-M4.2).
- "Acceptance runs on real work where it can" is written in no skill or convention (postponed from M3.1; 4 voyages). — postponed from M5 (2026-10-28, user; captain's reason to drop: a working habit, not code; weigh it at the next skill review).
- A switch to another CLI hands the new agent the previous conversation (T-77; postponed from M3 #4 and M4 #8; 5 voyages; M4's design chose a fresh start instead — drop or take). — postponed from M5 (2026-10-28, user; captain's reason to drop: M4 starts every round fresh from the packet, and one CLI is used).
- Quota markers for claude and grok, a status-bar reading for every kind (postponed from M3 #1 and M4 #2; 5 voyages; provider readings may make them unnecessary). — postponed from M5 (2026-10-28, user; captain's reason to drop: quota is read from the providers now; screen markers are fragile).
- The live Codex quota fallback, never observed (postponed from M4 #21; 3 voyages). — postponed from M5 (2026-10-28, user; captain's reason to drop: M5's reviewers have no fallback (user: wait when Codex is spent)).
- Kickoff prints each model's picker and bar labels (T-94 F4) — postponed from M5 (2026-10-28, user; captain's reason to drop: display only).

## Collected during M5

- **An explicit departure, and a way to remove a milestone (another project's captain, 2026-10-28).** A milestone departs on the first apply that leaves any task carrying it, cancelled and [wont] included (gear-app plan.rs fn fix_departures counts every task), so a captain cannot register and review drafted tasks before the user says depart; one invented a holding milestone with a one-minute eta to park them, and every station of it fell due at once. Apply never deletes a voyage row either, so a milestone created by mistake stays in the store with its words. Asked: drafts that do not depart (an explicit `depart` line or command the user's words authorise), and removing a milestone that has no task and no history.
- **The tree is not rustfmt-clean (M5 F-140).** `cargo fmt` on one crate rewrites every file, and no check catches style drift. Making it clean is one sweeping reformat that conflicts with every open branch, so it waits for a voyage boundary: reformat once at M6's start, before any lane branches, then `cargo fmt --check` in `make check` and the gate.
- **Run a lane's builds and tests on another machine (another project's user, 2026-10-29: no heavy work on the laptop, it goes to a build host).** In 0.4.10 every seat and its commands run where its pane runs. Asked: a lane whose agent runs locally but whose build and test commands execute on a named remote host, and a GEAR-side bound on concurrent builds across lanes and reviewers (today only fewer seats and narrow per-task check commands bound it).

- A stall item rings the captain urgently while the lane's next delivery is only just intended: after a fail verdict, M-5036 (mast, T-392 attempt 2) and M-5045 (hull, T-300 attempt 2) each named a delivery 0 minutes old, with "no check-in this generation". Both deliveries were confirmed within a minute, and the engine closed both items before the captain could answer. A stall should wait out the delivery's confirmation window before it rings (crates/gear-engine/src/stall.rs and where the flag is set).

- Branch cleanup gaps (another project, 2026-10-29): a cancelled task's branch is never deleted, and a merged task's branch is kept when its tip moved past the merged candidate (one more push after the merge).

- A captain's coverage report (another project, 2026-10-29): accepted attempts whose candidate is not an ancestor of the integration branch, and failing cases that no task names (83 of 159 reds there were named by no packet, found only when a reviewer hit a package check that could not pass).
