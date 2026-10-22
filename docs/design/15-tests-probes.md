# 15 Tests and Probes

## Tests (standard library unittest, about 1,700–2,700 lines)

Layout: `tests/unit/` (pure functions), `tests/flow/` (flow tests with a temporary SQLite + fake Herdr), `tests/live/` (real machine, triggered manually).

Fake Herdr: a programmable fake `herdr` executable (reads a scenario JSON, returns agent list/get/read results, records prompt/start/close calls, can simulate timeouts and being killed).

Scenarios that must be covered (mapped to PRD acceptance):

| Scenario | Modules |
|---|---|
| send receipt lost after commit, replay yields only one | 02, 07 |
| prompt timed out but was actually executed: no resend, no double open | 08, 09 |
| Ten messages, only one handled; two questions on the same task, only one answered; under an urgent flood ordinary old threads still get a share; a single sender flooding threads | 07 |
| Preemption: original task ID/results kept, new task dispatched only after the safe-point ack | 06, 05 |
| Contract v3→v4 interleaved with a late result | 06, 12 |
| Temp worker still writing after result → not closed; close unconfirmed → quota not released; resume success/failure falls back to a new session | 10 |
| Long-lived lane standby is silent, park re-evaluated when due, new task wakes it early, hold not interrupted | 10, 09 |
| A missing lane's worktree is not automatically reassigned | 10 |
| Manual captain replacement: frozen, old generation rejected, not effective until confirmed, survives restart | 03 |
| Captain quota exhausted: UNAVAILABLE, notify, no automatic replacement | 03, 08 |
| Two projects in the same session with identical task numbers do not cross; one slow project does not drag the other | 08, 07 |
| Observation failure is not treated as missing | 08 |
| Review same-cause merge, reopen when due, notify after two reminders | 11 |
| Old keeper incident regressions (nudge text containing a marker, done but still running, cooldown ≤ tick, HEAD unchanged but still continues) | 04, 09 |
| A child process of an old session carrying an old GEAR_CTX writes late after a new Agent replaces it in the same pane (same brand, no session_ref, rebinding between query and transaction) — all rejected; an Agent call without GEAR_CTX does not become admin | 03 |
| After a seat has taken the next task, a late result/ack for the old task goes to the original task's original attempt | 06, 07 |
| Doorbell submitted but no start observed: no resend even after cooldown; unrelated working does not confirm the delivery | 07, 08 |
| resume timed out but actually started → no second one; original session checked after resume; a new task with followup-of cannot borrow the old R; RELEASED is not judged MISSING | 10, 08 |
| Applying the same Staff target multiple times does not open twice; concurrent temp starts do not exceed the quota or create two worktrees for the same task | 05, 10 |
| Failed acceptance goes back to READY via apply and opens a new attempt; reply does not change task state; evidence present but conclusion fail does not allow DONE | 05, 06, 12 |
| Lane replies/acks only close the original item and do not produce endless mutual confirmations; resumes after the waited-on dependency is satisfied, a casual reply does not unlock it | 07 |
| A modified file modified again is still detected as a change; progress spam does not reset the no-output count | 12, 09 |
| apply does not overwrite the draft (including concurrent writes between compare and overwrite); opening the plugin repeatedly leaves no second resident process | 05, 08 |
| Onboarding reserves the generation only once (reused on retry/restart); a handover ACK without task arguments succeeds, other writes from the pending successor are rejected | 03 |
| A late blocked from an old task does not set the new task to WAITING; unlinked progress/question does not change task state | 03, 07 |
| An INFO reply not yet shown rings the doorbell, no more ringing after it is shown; the view record only confirms an operation when the delivered content was actually shown | 07, 08, 13 |
| After WAITING resumes, the same attempt continues; temp worker first dispatch happens only once; after a follow-up, release only after the safe point is re-declared | 06, 10 |
| Resubmitting a historical Verdict does not settle twice; fail does not need an evidence header; contract change plus acceptance in the same apply is validated against the final contract | 05, 12 |
| Expected starts/exits do not trigger "unexpected occupant change"; a vanished captain enters UNAVAILABLE rather than MISSING | 08, 03 |
| A v3 candidate can be failed after the contract moves to v4, and retried under v4 | 05, 06, 12 |
| The first onboarding delivery is sent only once (task/handover + skill pointer combined); reading the skill does not pass as a business confirmation | 16, 10, 08 |
| An ACK for an old skill version or old generation does not adopt the new version; no new work that depends on the new rules is dispatched before the new version is confirmed | 16, 07 |
| Both the no-install path (only `gear --skill …`) and a pending successor captain can read and complete the required confirmation | 16, 03 |
| Rework of the same item on a DONE task keeps the old ACCEPTED/Verdict, creates a new attempt and resumes the original temp worker; an ordinary follow-up does not reopen the task | 05, 10 |

## Probes (1–2 days before development, real machine)

1. Plugin: single instance, keeps running when switching tabs/hiding, recovery after closing and reopening the tab, placement=tab.
2. remote: local client disconnects and reconnects, the target-side loop continues.
3. Identity: ID changes after a pane moves; how agent get changes when the Agent in the same pane is replaced; whether each CLI has agent_session.
4. Every CLI actually used: screen signatures for busy/rate-limit/quota/auth; whether it falsely reports idle while a tool is running; how "prompt accepted but not started" looks; exit keys; **how to get session_ref, resume arguments, how to tell it "returned to the original session"**.
4b. After exporting `GEAR_CTX` in `pane run` and then `agent start`, the Agent and its child processes inherit that environment (verified separately for each CLI).
5. How `report-metadata --title` is displayed in the UI.
6. PATH of `gear` and state directory location inside Agent panes.
7. Herdr `worktree.create/open/remove` behavior: branch slug turns `/` into `-` in the path (confirmed); `remove` deletes the checkout and the Herdr workspace but keeps the branch (confirmed); `worktree.open` on an existing branch (e.g. resume of a temp worker's task branch) returns the original worktree/workspace.

Probe results are written into `verified` and the parameters in `profiles.py`; CLIs that fail are not allowed to take over real projects.
