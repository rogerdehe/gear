# M3 backlog (ideas, not yet tasks)

## Moved out of M2.1 (user, 2026-10-25)

- **Every built-in agent fully described by its profile (data, not code).** Today only OpenCode has `quota_markers`; Claude Code, Codex and Grok have none, so the engine cannot tell their quota stop from a finished turn. Only Claude Code has a `quota_bar` (profile.rs), so captain quota protection works only for a Claude captain. Fill, for all four kinds, from real screen captures:
  - quota-limit markers and server-error markers (e.g. "model overloaded");
  - the status-bar usage reading (window usage and reset time).
  Then quota fallback / wait-for-reset and captain quota behave the same for every kind in every role (captain, lane, reviewer). A kind with no built-in profile is added by the user with `[profiles.<kind>]`; the kickoff skeleton already emits one for a known kind without a built-in profile.
  Why deferred: the first trial user runs a Codex captain with ample tokens; quota exhaustion is not expected in M2.1.

- **Warn before a lane runs dry, from the engine (user, 2026-10-25).** The voyage should expose where a lane is about to stand idle, early; an idle lane is only the last net. M2.1 T-58 warns at `gear plan apply` for a lane with nothing startable behind its current task. M3: the engine's tick does the same continuously (a dep finishing late, a task cancelled, a hand-in sooner than planned), and forecasts from estimates and the lane's pace ("engine runs dry in ~15 min: its next task waits on T-52 in review") so the captain refills before the gap. Today's low-queue item fires only at 0 ready tasks — too late.

- **Review the newest contract (M2.1, T-58).** A contract rewritten mid-task (the lane is told to rework to it) still sent the reviewer the attempt's original v1 packet; the reviewer had to ask the captain which one governs. A hand-in after a contract change is reviewed against the newest contract.

- **Hand the conversation over on a cross-CLI switch (user, 2026-10-25).** An in-place OpenCode switch keeps the session; a switch to another CLI (OpenCode → Codex) starts a fresh agent that gets only the task packet, the worktree and the progress lines. Add the previous agent's kind and session id to the continue message, with the one command that reads it (e.g. `opencode export <session>`, `codex resume`'s transcript), so the new agent may look at what the old one was doing. Verify each CLI's read command live; the session id comes from Herdr (`session_ref`) or, for OpenCode, the session API the engine already uses.
