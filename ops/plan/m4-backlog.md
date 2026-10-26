# M4 backlog (ideas, not yet tasks)

## First item of M4 (user, 2026-10-26): a seat follows the work (B02-01)

A trial project had a lane bound to a repo whose work was a dozen lines; the voyage ran 30-40 hours, that lane stood idle 99% of the time and held its slot for good. M4 must fix it:
- a task stores its own repo (and base, gate) when it is created, so rebinding a lane never moves an old task's review or merge target (today a task has no repo column: it is read from its lane's config);
- the lane quota counts active seats, not configured or retired ones;
- a supported rebind: `- rebind <lane> repo=<name> owns=[…]` (or retire + register) that waits for the lane's open tasks to land, previews the change and keeps history;
- the captain's guide: a lane whose repo has no ready work is rebound at once, never left idle.

## Moved out of M3 (user, 2026-10-25)

- **Every built-in agent fully described by its profile (data, not code).** Today only OpenCode has `quota_markers`; Claude Code, Codex and Grok have none, so the engine cannot tell their quota stop from a finished turn. Only Claude Code has a `quota_bar` (profile.rs), so captain quota protection works only for a Claude captain. Fill, for all four kinds, from real screen captures:
  - quota-limit markers and server-error markers (e.g. "model overloaded");
  - the status-bar usage reading (window usage and reset time).
  Then quota fallback / wait-for-reset and captain quota behave the same for every kind in every role (captain, lane, reviewer). A kind with no built-in profile is added by the user with `[profiles.<kind>]`; the kickoff skeleton already emits one for a known kind without a built-in profile.
  Why deferred: the first trial user runs a Codex captain with ample tokens; quota exhaustion is not expected in M2.1.
  Moved to M4: M2 covers the kinds in use; take it up when a trial hits it.

## Added 2026-10-25 (user)

- **The captain's decisions are recorded, and the landing starts by aligning them with the user.** To run unattended, the captain decides on its own mid-voyage (verdict overrides, findings logged instead of reworked, tasks added/cut/moved, re-scoping at stations, model or config changes, hand fixes). Today these end up scattered in the voyage file's Decisions, mixed with notes, and at landing the user is asked straight away whether to reclaim the lanes — before they have seen what happened or why. Wanted:
  - every captain decision recorded as it is made, in one place, with what was decided, why, what else was possible, and what it changed (scope, time, quality, risk); hand fixes always recorded;
  - at landing, first a voyage review with the user: the report plus the decision log, decisions the user may want to revisit marked; the user's corrections are recorded too (and feed the next voyage's kickoff and the skill review);
  - only after that alignment, the end-of-voyage questions (reclaim the lanes, release).
  Today's captain guide and lessons say decisions go into `# Decisions` and the landing asks about reclaiming; both change with this.

## Station projection ignores parallel lanes (M3)
The station's "projected finish" multiplies the sum of all open estimates by the pace (M3: 800m × 1.73 = 1470m against a 6h eta), as if one lane did all the work. Project per lane (its own open estimates × measured actual/estimate) and take the slowest lane; show which lane sets the finish, so the captain sees an imbalance instead of a false overrun.

## Cut from M3 at station M-916
- T-66: the engine warns the captain before a lane runs dry, each tick (not only at plan apply).
- T-70: engine alerts de-duplicated by fact and closed when their cause is gone (trial G06, G10).
- T-89 F1 (logged at review): a busy agent start is retried inside one onboarding call (up to ~20 s, blocking the tick) instead of on the next tick.
- T-77: a switch to another CLI hands over the previous conversation (user idea).
- T-92 F1/F2 (logged at review): remerge does not refuse a candidate already on the integration branch (harmless: the merge confirms it); op ids compared lexically at the same second.

## GitHub PR-mode landing (descoped from M3 T-91)
`merge = "pr"` lands through the PR: on a passed gate the engine does not push or delete the head; it records readiness, sync marks the PR ready and merges it through the forge, then the engine records the merge and cleans up. Until then config refuses `merge = "pr"`. Also T-91 F4: reconcile an interrupted claim by its marker instead of quarantining the task.
- T-69 R2 (moved at review): when review/t<n> is held by a dirty reviewer worktree, start the review in that holder's seat instead of failing three times.
- T-94 F4 (moved at review): kickoff prints each model's derived picker/bar labels. F6: catalogue unavailable vs model absent kept apart in the label lookup.
- Push-race retry re-runs the full gate each time while dev moves (M3: T-68, T-88 lost three races each): re-merge onto the new tip and push at once when the resulting tree equals the gated one; gate again only when it differs.

- **A board a person can read (user, 2026-10-26).** Today's `gear board` pane is a text dump: every done task of every lane, retired fixture projects, every lane "agent unknown" while working, a dropped task (T-102) shown as done. The user closed it: it reads like a log for agents, not a board for a person. Design it with the user first: one screen answering what runs now, what is stuck and why, what waits for the user, how close the done sentence is; history and retired projects folded away; lane state from Herdr's real agent status.

- **Each task starts in the right conversation (user, 2026-10-26).** Today every task packet goes into the lane's one running session, whatever came before. Three cases: (1) an unrelated task arrives in a session already ~80% full — pure token waste; start a fresh session (`/new` or the CLI's equivalent). (2) The next task builds on the previous one — keeping the session is right. (3) A task comes back for rework — best resumed in the session it was built in (`/resume <id>`), so the lane picks up its own reasoning. Needs: a pre-task hook in dispatch that chooses new / keep / resume per task, and the store keeping task+attempt → agent session id (the reviewer side already stores a session per task, T-91 F3 / review.rs `session_key`, so the lane side can follow it). Decide per agent kind (Claude Code, Codex, OpenCode) how a session is started and resumed, and how relatedness is judged (same files, `followup_of`, deps, the captain's plan line).


- **A light procedure for a small task (user, 2026-10-26).** Every task costs ~10 min of fixed procedure whatever its size (reading the skill and packet, exploring, verifying from a clean clone, a report): a 3-line fixture task took 11 min. Under a loose bar and a tiny estimate, a lighter procedure (no clean-clone re-check, a one-line report).

## Cut from M3.1 (station M-2234, 2026-10-26)

- T-122 — `gear` knows its caller without env vars (trial B02-05; the Codex env-var friction). With T-104's real-pane proof, a call with no GEAR_CTX takes its seat from the pane it really runs in (the seat bound to that pane) and its store from GEAR_DB or the default path; GEAR_CTX and GEAR_DB stay as overrides. `gear whoami` prints the real pane, the seat bound to it, the store, and why a claim fails. The skills drop the long `GEAR_CTX=… GEAR_DB=…` prefixes where they are no longer needed. Accept: Tests: a caller in a bound pane with no GEAR_CTX is that seat; in an unbound pane it is anonymous; whoami prints pane, seat and store. `cargo test --workspace` passes.
- T-123 — An OpenCode lane's identity is proven, not only claimed (M3 hotfix 6d82c7d lets a caller outside every pane keep its env claim for an OpenCode pane, so one OpenCode lane can pose as another). Prove it with a fact the OpenCode service carries per session: the session id GEAR created for the seat (OpenCode's API: the session of the tool call, or an env var the service sets in its tool shells — find which on opencode's current version) must match the seat's recorded session. Accept: Tests with a fake: an OpenCode caller whose session matches the seat passes; another lane's session is refused with one urgent item. `cargo test --workspace` passes.

## Added 2026-10-26 (user)

- Tokens per task: next to the time and decision records, record how many tokens each task cost (lane and reviewer, per attempt), so a voyage report can show it.
- Quota reset time of connected models: with an API key OpenCode shows no reset time, and pi has the same gap. Find a way to learn when each connected model's quota resets (provider API, response headers, or the CLI). Research item; its scope in M4 is to be discussed with the user first.
- Live Codex quota fallback: watch a reviewer hit Codex's spent weekly limit and the review finish on opencode-go/deepseek-v4-pro (M3.1 could not: the quota reset first).
- Reviewer quota fallback in the engine (T-115 review F2): apply_quota_policy skips non-lane seats and never reads `on_quota.review`; a spent reviewer must close Codex and restart on the next `models.review` entry (opencode-go/deepseek-v4-pro) with the review delivered again. Required for M3.1 done clause (2), which was not met.

## Cut from M3.1 (station M-2277, 2026-10-26)

- T-120 — Plan hygiene (M3 acceptance): after a successful apply, used `# Staff` lines (`remerge`, `retry`, `retire`, `park`, `hold`) are removed like `# Verdicts`, so a stale line never fails the next apply; an empty section is removed too and a second `# Staff` section is not created by GEAR. The failed-merge item's `next:` names `- remerge T-n` under `# Staff`, not a fail verdict, while its candidate is still not merged. `gear plan apply` prints which lanes' idle warnings are off when a `- tail:` is in force (trial B02-08), and the home page shows the tail. Accept: Tests: an applied remerge line is gone from the file; a failed-merge hint names remerge; apply with a tail prints the lanes; the home shows them. `cargo test --workspace` passes.
- T-121 — `gear admin stop` never cuts a merge (M3, T-110 review): while any merge op is intended, executing or unknown it waits (polling the store, up to 10 minutes, printing which task merges) and then stops the engine; `--now` stops at once. The v0.3.0 upgrade guide's manual merge check becomes one line pointing at this. Accept: CLI test with a store holding an executing merge: stop waits and reports the task; after the op is confirmed it stops; `--now` stops at once. `cargo test --workspace` passes.
- Codex in app-server daemon mode (trial, 2026-10-26): the pane's Codex is a client and a shared daemon runs its tools, so the real-pane proof never reaches the pane (same shape as `opencode serve`); restarting the daemon does not help. GEAR starts Codex seats without the daemon (the no-daemon flag of the installed version) and the identity error names this case ("the tools run in a daemon outside the pane"). Pair with `gear whoami` (T-122) and the handover.
- A lane that works without checking in (it never ran `gear` for the delivery) is flagged stuck, its delivery op fails and the hand-in is refused ("attempt: delivered -> submitted"); M3.1 hull on T-125. The engine should see the lane is on the task (its branch moved, its pane shows the task) and record the check-in itself.
- Changing a lane's agent kind in place (user, 2026-10-26): a config re-apply that changes a lane's `kind`/models makes the engine, at the lane's next safe point, exit the old agent and start the new kind in the same pane and worktree, the task delivered again. No retire, no new worktree, the build cache kept. (M3.1 moved deck and hull from OpenCode to pi through retire + re-register + a removed worktree, losing `target/`.) Pair with the rebind item (B02-01).
- The engine keeps the Herdr session it was started with (M3.1): an engine started by a `gear` call from another session's environment, or before the Herdr server restarted, reached no lane ("agent_not_ready: … is not an active named agent") until restarted. The engine must talk to each project's own registered Herdr session and reconnect when that server restarts.
