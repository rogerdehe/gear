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
- Plan apply is not idempotent for `state: cancelled`: re-applying a plan that still says it fails ("task T-n is already cancelled"), so the captain must delete the line after the first apply (M3, three times).
