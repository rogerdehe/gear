# T-152 attempt 3: live readiness at prompt time

Dispatch only plans task delivery for Idle or Done agents. Doorbell decisions retain Herdr's actual status: Blocked receives no prompt, Unknown receives urgent at-once messages only, and Working retains the at-once exception. Both the tick and immediate sender paths recheck the live status immediately before prompting. A queued doorbell is rechecked against the messages it covers so it cannot carry ordinary messages into a newly busy or Unknown agent.

A previously intended delivery remains the same Intended operation while the agent is Working, Unknown, or Blocked, and is submitted once when it becomes Done. Session preparation remains ahead of task delivery. Fake-Herdr regressions cover these transitions, both doorbell paths, a state change after doorbell intent, and the original Done-pi acceptance through fresh-session preparation.

All 19 focused readiness, doorbell and blocked-delivery tests passed. `scripts/test.sh` passed all 1,191 tests (241 s build, 62 s suite); shared build load and existing Git-heavy tests produced slow-test warnings. The earlier Done-agent fixture was updated to model fresh onboarding while retaining the session preparation step.
