# M3 backlog (ideas, not yet tasks)

## Moved out of M2.1 (user, 2026-10-25)

- Every built-in agent's quota markers and status bar: moved to gear/voyages/M4/backlog.md (user, 2026-10-25); M3 keeps Codex's 429 back-off (trial G11).

- **Warn before a lane runs dry, from the engine (user, 2026-10-25).** The voyage should expose where a lane is about to stand idle, early; an idle lane is only the last net. M2.1 T-58 warns at `gear plan apply` for a lane with nothing startable behind its current task. M3: the engine's tick does the same continuously (a dep finishing late, a task cancelled, a hand-in sooner than planned), and forecasts from estimates and the lane's pace ("engine runs dry in ~15 min: its next task waits on T-52 in review") so the captain refills before the gap. Today's low-queue item fires only at 0 ready tasks — too late.

- **Review the newest contract (M2.1, T-58).** A contract rewritten mid-task (the lane is told to rework to it) still sent the reviewer the attempt's original v1 packet; the reviewer had to ask the captain which one governs. A hand-in after a contract change is reviewed against the newest contract.

- **Hand the conversation over on a cross-CLI switch (user, 2026-10-25).** An in-place OpenCode switch keeps the session; a switch to another CLI (OpenCode → Codex) starts a fresh agent that gets only the task packet, the worktree and the progress lines. Add the previous agent's kind and session id to the continue message, with the one command that reads it (e.g. `opencode export <session>`, `codex resume`'s transcript), so the new agent may look at what the old one was doing. Verify each CLI's read command live; the session id comes from Herdr (`session_ref`) or, for OpenCode, the session API the engine already uses.

- **Parallel-change check skips landed tasks (M2.1).** It warned "T-58 and integration dev share main.rs" after T-58 had merged (state verifying/done): a task whose branch is already in the integration branch is not a parallel change.

- **A task built on another unreviewed branch merges after it (M2.1).** T-59 was built on T-53's branch and merged while T-53 was in rework, carrying T-53's failed attempt into dev. The merge of a task should wait until every task whose commits it carries has passed.

- **Merge the approved commit, not the branch name (M2.1, T-53).** The lane pushed its rebased rework to a new branch `engine/t53-v2` (to avoid a force push); the engine merges `<lane>/t<n>` and refused: "the branch moved after the review". The approved candidate was fine. The merge should take the approved candidate commit wherever it is, and a failed merge needs a retry path (today the captain had to point the branch and reset the op in the store by hand).

- **A hand-in must be pushed (M2.1, T-60).** The lane handed in cf58f57 on a local `config/t60` that tracked origin/dev and was never pushed; the reviewer (same machine) read it and passed it, the merge found no `origin/config/t60`. `gear send --kind result` refuses a candidate that is not on `origin/<lane>/t<n>`.

- **The engine finishes a lane's retirement (M2.1 landing).** `- retire <lane>` sets the seat Retiring; release.rs closes only reviewers' panes, so the lanes stayed up until the captain closed their panes and set the seats Retired in the store by hand (as at M2's landing). An idle Retiring lane with no open attempt: exit the agent, close its pane, set it Retired.

- **The captain registers the project (user, 2026-10-25).** The user asked why re-registering a changed config was handed to them. `gear admin init` refuses any caller with GEAR_CTX ("never in an agent"), so the captain cannot do it without dodging the guard. Allow the captain's seat to run `init` (the confirmed kickoff sheet is the user's consent), then drop README step 3 and the "user command" line in the captain guide.

- **Join a team's existing tracker, GitHub issues first (user, 2026-10-25).** Some teams coordinate through GitHub issues (like the team setup the user pointed to). Two modes to weigh: local (GEAR's plan and store as today) and graft (issues become the voyage's work). First thought, to be designed with the user: GEAR's plan stays the one source of truth; issues are an adapter at the edges — in: the captain reads issues with `gh` as raw material for decompose, and a task carries `issue: #n`; out: the engine writes hand-in, verdict and merge back to the issue (comment, label, close on merge). Study the referenced setup before designing.

- **Fit GEAR to a project that already runs (user, 2026-10-25; design to be talked through with the user).** A new project uses GEAR as is. A project that already has its own way of working — running well or badly — needs an analysis first, so GEAR does not land blindly ("水土不服"): what it already has (repos and branches, CI and gate, release process, trackers, docs like BMAD, review habits, other agent setups), what GEAR should take over, adopt from it or leave alone, and how to migrate step by step. Likely a skill topic (e.g. `gear --skill adopt`) the captain runs before kickoff; its findings feed the kickoff sheet (conventions, repos, gate, lanes) instead of GEAR's defaults.

## From the first trial's feedback (2026-10-25; project details left out on purpose)

Must be solved in M3 (GEAR's own faults):

- **Writes go only where the project allows (trial G12, critical).** A company project may push only to its own git server; any other destination (e.g. GitHub) must never be written. Today nothing in GEAR enforces where a lane, a reviewer or the engine pushes. At registration, freeze each repo's resolved push destinations (`git remote get-url --push --all`, after URL rewrites); every GEAR worktree gets a GEAR pre-push hook that refuses any other destination; the engine checks the same before its own merge push. A refused push is its own captain alert (command, repo, destination host, refused) — never retried, never routed around. Negative tests: a disallowed destination, a rewritten URL, an unknown remote. (Test isolation from the user's git config already shipped in v0.2.4.)
- **An empty first model list is "not loaded yet", not "no such model" (G01, G03).** OpenCode's first `model.list` for a new worktree can be empty until its plugins settle; onboarding marked every lane NeedsCheck and no agent started. Wait and re-query the same directory for a bounded time; only a settled list without the model is an invalid model. Kickoff's inventory says "not loaded yet" the same way.
- **A seat flagged NeedsCheck can be retried (G02).** There is no way back from NeedsCheck short of re-registering in a new store, and `gear admin seat confirm` is listed in `--help` but answers "not implemented". Give the captain one retry-onboarding path; remove unimplemented commands from the help.
- **The home page shows why a seat is down (G04).** Every lane showed "agent unknown / no current task" while each seat was NeedsCheck with a clear reason in the store. Show the seat state, the reason and the recovery action.
- **Start-up dialogs: Codex's folder-trust prompt (G08).** Reviewers sat on "Trust this folder?" although the unattended flags were passed. Pass the trust for the worktree's repo root in the start args (a `-c` override for that path), within the repos the user registered; never trust a wider directory.
- **A blocked seat is not re-delivered every tick (G09).** While a seat is Herdr-blocked the engine created a failed delivery op every ~4 s. Stop delivering while blocked (one pending op), let the blocked-seat alert (T-57) carry it, deliver once when it clears.
- **A re-review on another seat must not fight the old checkout (G10).** The new seat looped on `checkout -B review/t<n>` "already used by worktree" (the previous reviewer's checkout still held it) and held a temp slot with no agent. Reuse the task's previous review checkout, or free the branch when that checkout is clean; a failed checkout is bounded and reported. Show running / starting / blocked separately in capacity.
- **Alerts close when their cause clears, and are de-duplicated by fact (G06 part, G10 part).** A stall alert stayed open after the delivery confirmed; a "temps full" item came back after it was deferred (dedupe used "actionable", so a waiting item did not count). Dedupe by task/attempt/reason including waiting items; resolve automatically when the condition is gone.
- **Provider back-off is not a stall (G11).** Codex retrying a 429 for minutes looks "working" with no output; GEAR raised stall alerts. Part of the agent-coverage item: per-kind rate-limit/back-off signs, shown as such, no stall alert meanwhile.
- **Every CLI that runs a seat has the skill (G06 part).** The README installs the skill for the captain's CLI only; lanes on another CLI ran without it. Kickoff checks every kind in the config and `init` refuses a kind whose skill is not installed (or installs it).

Logged, not taken (not GEAR's fault, or not worth it now):

- G05 early stall alert: the lane read its material before running `gear` (agent order); only the auto-close above is ours.
- G06 a lane edited files on its home branch before switching to the task branch: an agent slip, caught by the captain; no pre-commit guard — the skill being installed fixes the cause.
- G07 the board reading the default store: the trial re-registered into a non-default store by hand; the default works. Revisit only if the board should show which store it reads.
- G12 extras (full audit trail of every permission event, reports labelled "local transport"): beyond what the allow-list and its alert need.
