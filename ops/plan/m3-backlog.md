# M3 backlog (ideas, not yet tasks)

## Moved out of M2.1 (user, 2026-10-25)

- **Every built-in agent fully described by its profile (data, not code).** Today only OpenCode has `quota_markers`; Claude Code, Codex and Grok have none, so the engine cannot tell their quota stop from a finished turn. Only Claude Code has a `quota_bar` (profile.rs), so captain quota protection works only for a Claude captain. Fill, for all four kinds, from real screen captures:
  - quota-limit markers and server-error markers (e.g. "model overloaded");
  - the status-bar usage reading (window usage and reset time).
  Then quota fallback / wait-for-reset and captain quota behave the same for every kind in every role (captain, lane, reviewer). A kind with no built-in profile is added by the user with `[profiles.<kind>]`; the kickoff skeleton already emits one for a known kind without a built-in profile.
  Why deferred: the first trial user runs a Codex captain with ample tokens; quota exhaustion is not expected in M2.1.
