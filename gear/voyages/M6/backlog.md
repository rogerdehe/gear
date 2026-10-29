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

- Slash commands typed into an agent (`/new`, `/resume`, `/compact`) go in one character at a time and are slow; type them in one go, and handle the dialogs some CLIs open after them (codex asks whether to work in the current worktree or a new one).

- M6 goal: solve every performance problem, both slow operations and high resource use.
- Machine and fleet health watch (decided 2026-10-29: M6 does it). It watches the CPU and memory of the fleet's own lanes and reviewers, the processes they start included, and the machine's overall health. Internal cause (a lane's or reviewer's processes over the safe line): the engine interrupts that seat and stops it, and tells the captain, who holds the lane and fixes its cause. External cause (the machine is short but our seats are not the reason): an urgent item to the captain to find the cause; when another captain on the machine is behind it, the captain tells that captain to act at once. The watch itself must cost almost nothing: no watch that loads the machine it protects.
- Performance: GEAR is a scheduler and must run near idle (the engine and each board under a few percent of one core, tens of MB). Measured 2026-10-29: engine 15-78 % CPU in spikes, boards 20-34 %, about 20 MB each; the engine's cost is SQLite reads in the overrun step (fixed in M5 by T-406) and the board's full rebuilds (T-384). M6 reads the M5 probe lines (T-407) and fixes the slow operations they show, starting with every multi-tick step that waits a whole tick between steps.

## A new codex seat opens another seat's conversation
- 2026-10-29 M5: review-11 (generation 325, no session id recorded) started inside review-9's Codex conversation; Codex refused it ("this conversation is open in another app", r retry / f fork), so its review deliveries stayed unconfirmed for 14 minutes. The captain forked by hand and retyped the delivery. Find how a fresh codex seat chooses its session (a resume of the latest conversation in a shared directory is the likely path), make every new seat start its own, and treat that dialog as blocked so the stall item names it.

## Lost-merge cleanup behind the old filters
- 2026-10-29 M5 (T-408 review F1): the scan drops a task with a merge in flight or an open merge-failed item before the finished-voyage / gone-candidate check, so its open merge-lost item stays open in that case. Check the skip rule first for tasks with an open merge-lost item, then apply the old filters for new items.

## A repeated confirmed goal/bar/eta change records no second decision
- 2026-10-29 M5 (T-410 review F1): the decision key in plan.rs record_voyages holds only project, milestone, field and the old and new text, so the same confirmed transition a second time in one voyage is skipped by insert_decision_if_new. Add the plan version to the key and a test with three confirmed transitions.

## A lane that reports blocked keeps its task and gets nothing else
- 2026-10-29 M5: mast reported T-367 blocked on T-366 (`gear send --kind blocked --on T-366`) and then sat idle about 40 minutes: the attempt stayed Started, so the engine assigned no next task. A blocked report with `--on` should park the attempt and free the lane for its next startable task, and the captain should hear when a lane is idle with queued work.

## A held seat cannot report
- 2026-10-29 M5 (F-176): while the user's stop held mast, every `gear send` was refused ("only active, standby or retiring may do this"), so a finished task could not be handed in and the lane could not even ask the captain. A held seat should be able to send questions and results; only new work stays held.

## A test flake in gear-cli under one-process cargo test
- 2026-10-29 M5 (F-153): two gear-cli tests share the scratch tag "stale" and race under plain `cargo test`; nextest's process per test hides it. Give each test its own tag.

## The shared compile cache barely hits
- 2026-10-29 M5: sccache (rustc-wrapper in the machine's cargo config since M4) hit 1.4 % (70 misses, 1 hit, 331 requests not cacheable): every lane, reviewer and merge worktree builds our own crates from a different absolute path and incrementally, which sccache does not cache. Find what makes one worktree's build reusable by another (a path-independent cache key, CARGO_INCREMENTAL=0 for check runs, or one shared target directory per crate set) and measure it before turning it on.

## The stall item fires on a delivery that is still being sent
- 2026-10-29 M5: three captain stall items in an hour (M-5520, M-5544, M-5584) named a lane "stuck" on a delivery that was `intended, 0 min` and confirmed within 30 s. The stall check should wait out `[timing] delivery_grace_s` (180) before it counts an unconfirmed delivery.

## macOS scans every new build binary
- 2026-10-29 M5: syspolicyd (Gatekeeper) ran at about 250 % CPU for hours: every test binary, build script and fresh tool a lane, reviewer or merge gate builds is assessed on its first run, so git calls and test starts slowed many times over on a busy machine, and a reviewer saw compiles stall on signature checks. The fix is the machine's: `sudo spctl developer-mode enable-terminal`, then System Settings → Privacy & Security → Developer Tools on for the terminal app that hosts Herdr, and restart it. Candidate for the skill (`gear admin kickoff` could check it and say so on macOS), since every Rust or Go fleet on a Mac hits it.

## A warm reviewer is told the previous review's directory
- 2026-10-29 M5 (F-185, F-197): a reviewer seat reused for another task got a packet whose working directory was the previous review's worktree (review/t336 for T-331, review/t407 for T-420); the reviewer found the right one itself each time, at a few minutes' cost. The packet of a warm seat should name the worktree of the task it now reviews.

## The slow-step report misses a failed observation
- 2026-10-29 M5 (T-386 review F1): crates/gear-engine/src/lib.rs fn tick returns early when observe::snapshot fails (about line 302), before note_slow, so a slow failed Herdr observation is not in the tick's slow-step report. Note the step before the early return.

## init names the reviewers by [temps] kind, not by the review entry's CLI
- 2026-10-29 M5 (another project's captain): with [temps] kind = "pi" and [models].review = [{ model = "gpt-6.1-terra", cli = "codex" }], `gear admin init` prints "reviewers: 4 x pi gpt-6.1-terra" and crates/gear-app/src/kickoff.rs (about line 236) checks the review chain against the [temps] kind, while the engine starts the reviewers on codex as the entry says. The line and the model check should follow each entry's own CLI.

## No command files a bug issue on the forge
- 2026-10-29 M5 (another project's captain): a voyage policy asked for a GitHub issue per production bug a test found; GEAR's [github] mode only imports issues and comments on task PRs, so the captain filed them with gh by hand. A captain command that files an issue through the engine's forge ops (once, retried, recorded) would keep that path inside GEAR.

## Hand-in and review checks against the packet
- 2026-10-29 M5 (another project's rework audit: 58 of 135 reviewed tasks passed first time): a plan apply that warns when two Ready packets name one file; a pre-hand-in check that the diff stays in the task's allowed files and the report matches the adopted contract; a reviewer start that verifies its worktree, branch, task, attempt and adopted contract before it reads anything; a captain's direction-check answer carried into the review packet.

## A delivery marked submitted never reaches the composer
- 2026-10-29 M5 (both projects): our hull showed "Continue T-425" typed with no Enter; the other project's review-2 and review-3 sat at empty Codex composers with their direction-check deliveries recorded as submitted, until the captain nudged them to run `gear`. The engine should confirm a delivery by the agent turning busy and resubmit within a bounded time, not wait for the stall item.
- 2026-10-29 14:00 (the other project, on 0.5.2: review-3 T-225 O-2642, review-1 T-226 O-2652): each review seat sat done with 0 % context and an empty composer right after a fresh Codex conversation, while GEAR said in review. Likely cause, unverified: with Codex 0.165 the `/new` that starts a fresh conversation opens "Where should the new conversation run?"; the delivery's text lands in that dialog and its Enter picks the preselected "Use current Git worktree", so the conversation starts empty. The root fix is the same as for the startup dialogs above: the codex profile answers that dialog (or starts so it never shows) and waits for an empty composer before it types the packet, and a delivery is confirmed by the agent's first `gear` call, resent once if none comes within a bounded time.

## False stall items on a delivery into a busy lane
- 2026-10-29 M5 (both projects, about ten today: our M-6054, M-6089, M-6121, M-6144; the other project's M-2782, M-2797, M-2809): right after a hand-in the engine delivers the next task while the lane is still ending its turn. The confirmation looks for the agent turning busy, which a busy agent never does, so the window ends unconfirmed, crates/gear-engine/src/ops.rs (about line 947) flags the seat, and the item says "delivery … 0 min, no check-in this generation"; it resolves itself once the lane reads the queued text. A delivery into a working agent should be confirmed by the attempt's first check-in (or the agent's next idle-to-busy turn) within a bounded window before any item opens.

## A notice typed into a busy lane aborts its running check
- 2026-10-29 M5 (T-407 rework, M-6183): the overrun and quiet-lane notices arrive as prompts while the lane runs a 9-minute `make check`; pi stops the command to read them, twice, so the acceptance never finished. A notice to a lane whose agent is working should wait for its next idle moment, or be marked to queue without interrupting.

## Codex 0.165 startup dialogs stop reviewers
- 2026-10-29 M5 (another project): after `/new` Codex asks "Where should the new conversation run? 1. Use current Git worktree / 2. Create new Git worktree", and on a resume from another directory it asks which working directory to use; Herdr reports both as done/idle, so the engine waits on a reviewer that never starts. GEAR's codex profile should recognise both screens as blocked and answer them (current worktree; the seat's own directory), or start Codex so they never show.

## A reviewer resumes another seat's direction-check conversation
- 2026-10-29 M5 (the other project, after T-423 shipped: review-2 for T-226 resumed the direction-check conversation review-1 had run, and Codex locked it, "This conversation is open in another app"; O-2609 sat submitted 45 min). T-423 compares only each live agent's current session, but a Codex process keeps the writer lock of every conversation it opened (seen as `$CODEX_HOME/thread-writer-locks/<id>.lock` held by the other seat's process), so a conversation another seat ran earlier still passes the check. Root fix: a review round resumes an earlier round's conversation only when the same seat ran it (V1 as written); any other seat starts fresh.

## An accepted review whose branch never reached the remote
- 2026-10-29 M5 (the other project, T-265, M-3539): the verdict was accepted, then the merge refused because origin/<project>/review/t265 did not exist; the reviewer's push had not landed and nothing checked it. The engine should confirm the review branch on the remote when it records the verdict (or push the seat's local branch itself) before the task is accepted.

## Direction-check and review packets quote the goal, not the rework that replaced it
- 2026-10-29 M5 (the other project: T-254's direction check M-3565 held the lane to the original goal's "all 3 clickable" while the applied rework narrowed it to two Settings cases and a prototype exemption; T-271 similar). A packet for a rework round should show the latest applied rework text first, marked as the instruction that governs where it differs from the goal, and the reviewer should judge against it. Also: T-270 was a second accepted review whose branch never reached the remote (see above).
- 2026-10-29 (the other project, third case: T-293 attempt 2, direction check M-3872 on 27bcee99e quoted contract v1's goal against the captain's applied rework after issue #339; the lane stopped to ask, M-3875, and the captain corrected review-4, M-3877). Recurring in every rework round with a narrowed goal: the direction-check packet must carry the current attempt's rework text.

## A review job replaced mid-review learns it only at hand-in
- 2026-10-29 M5 (the other project, T-283, M-3715): the engine rebound review-4 to a new job on the attempt's current candidate while the previous job was still reviewing an earlier commit; the old job finished, judged the stale commit, and its hand-in failed NO_TARGET ("review-4 not bound"). A replaced job should be told at once (stop, your binding moved), and a review packet should refuse a candidate that is not the attempt's current one.
- 2026-10-29 (the other project, F-215, review-2 on T-284 attempt 2): the seat's worktree was switched to T-292 while its agent was still reviewing T-284; the agent restored an accidental unpushed commit and rebuilt the T-284 tree by hand. Same root as T-283: the engine treats a seat as free (Standby after a verdict or direction result) while its agent is still working, and prepares the next job's tree in the same worktree. A seat is free only when its agent is idle and its last job's hand-in is recorded; until then the next job waits or takes another seat.
