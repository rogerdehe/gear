# M4 backlog (ideas, not yet tasks)

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

