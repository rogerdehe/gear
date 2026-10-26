# 08 Engine Loop, Observation, Side Effects

Implementation: [lib.rs](../../crates/gear-engine/src/lib.rs), [observe.rs](../../crates/gear-engine/src/observe.rs), [ops.rs](../../crates/gear-engine/src/ops.rs).
Invariants upheld: the only process that executes Herdr side effects; an unknown outcome is not resent; an observation failure is not treated as "nothing happened"; single instance.

## Startup (`gear run`, started in the background by any `gear` call — design/14)

1. Single-instance lock: `<state_dir>/engine.lock` (non-blocking file lock). If it cannot be taken → print "engine already running (pid, heartbeat, pane)" and exit, leaving no second resident process.
2. Open the DB, check the schema version, read all project configs and display the effective values.
3. **Recovery check**: all `EXECUTING` operations become `UNKNOWN` (the last process died mid-execution); `SUBMITTED` deliveries stay pending check; nothing is resent. Check that the execution host/session bound in the database matches the current Herdr; refuse to run on mismatch.
4. Enter the loop.

## Loop tick

- Low-frequency jobs: output sampling default 20 minutes, review timer check 1 minute, expired park/wait check every round.
- A project times out this round (e.g. Herdr is slow) → remaining work is pushed to the next round, other projects proceed as usual.
- Slow calls do not stall the main loop: long calls such as `agent_start` and `worktree_create` run in a bounded worker thread pool (concurrency ≤ 4); the main loop only registers them and collects results; neither Herdr calls nor git sampling hold the SQLite write lock.

## Observation and reconciliation

Reconciliation rules:

| Observation | Action |
|---|---|
| agent list call failed | all of this project's seats `present=None` this round, no judgement made, board marks "observation failed" |
| seat pane gone / Agent exited | lane (ACTIVE/STANDBY/PARKED/HELD/RETIRING) → `MISSING` (handled by 10); **captain → `UNAVAILABLE`** (03, relevant permissions frozen); expected changes in QUEUED, RELEASED, STARTING/RELEASING are checked against the corresponding operation and do not count as missing |
| occupant fingerprint changed | first check whether there is an onboard/release/handover operation for it: an expected start or exit is checked against that operation; only an unexpected change with no corresponding operation revokes the binding and sets `NEEDS_CHECK`, no dispatch, notify |
| status idle/done | read the screen once (tail N lines) to confirm no busy signature, then hand to 09 as "idle" |
| ratelimit/quota/auth matched | confirmed only after `confirm_rounds` consecutive rounds; captain auth → 03's UNAVAILABLE; captain quota uses only its configured footer status bar (T-37): pause new assignments, continue current work/reviews/merges, wake at reset and resume after a new view |

## Side effects

Side-effect intent, execution and confirmation are recorded in the database.

- names (T-134): once per tick, before any delivery, every seat with an active binding gets its Herdr agent name back when Herdr shows the pane's agent unnamed or under another name (`engine::names`); a name another pane holds is reported to the captain once and never stolen. A prompt, doorbell or key send to a seat's pane makes sure of the name first, so a delivery to a pane whose agent lost its name renames it and delivers in the same tick.
- kind: `onboard` (start and onboard, including resume; for a long-lived lane's first onboarding and for a temp worker's new task this includes `worktree.create`, for a temp worker's rework/resume `worktree.open`, all via Herdr, 04), `deliver` (dispatch/continuation), `doorbell`, `release` (make a temp worker exit; its worktree is kept until the result is merged, then removed with `worktree.remove`), `title`, `notify`. park/hold/retire are pure data targets completed by plan apply in its transaction and do not go through ops.
- Before execution, check current state against the generation, task/attempt, and plan version fixed at intent time; mismatch → `VOID`.
- `SUBMITTED` = the Herdr call returned success, which **does not mean** the Agent has started; `CONFIRMED` requires evidence linked to this operation: deliver/doorbell → a view record of the seat's current generation **actually displayed the content this delivery referred to** (13); ordinary progress, reports from an old round with the same attempt number, and other detail queries do not count; onboard → the expected Agent is observed and the fingerprint is recorded (and, where a worktree was created/opened, its path is recorded in 02 resources); release → the original Agent has exited and the needed records are saved; title/notify → an explicit success receipt is enough. working by itself is not evidence. When it cannot be reliably linked, keep UNKNOWN/needs check; do not relax.
- `UNKNOWN` (timeout, process killed, unparseable receipt): do not resend; keep observing for linked evidence within the check budget; beyond the budget → mark the seat "needs check" and notify.
- Only when Herdr explicitly rejects and it is known not to have executed (e.g. `agent_blocked`) is it marked `FAILED`.

## Test points

Fake Herdr: killed during execution and restarted does not resend; observation failure does not trigger MISSING; RELEASED is not judged missing; an occupant change is not inherited; working does not confirm a delivery; one slow project does not block another; failing to take the single-instance lock exits.

### Busy lane without development work (T-46)

After reconciliation, the engine times present active/standby lane agents whose
Herdr status is `Working`, whose development attempts are empty, and whose
eligible unclaimed ready queue is nonempty. Submitted attempts are review work.
After `timing.lane_busy_idle_min` (default 10, positive), one urgent captain item
opens. Timer/generation/incident state is stored in `meta`; intent and the latch
are atomic. Recovery closes an outstanding item and resets the clock. Failed
observations interrupt the timing evidence without silently resolving an alert.

## Parallel changes (T-51)

A new hand-in triggers a scan on the next tick; otherwise scans run every ten
minutes. Read-only Git diffs compare each task branch with the integration tip,
and compare different lanes' changed-file sets. Integration-side changes since
each branch's merge base count too. The scanner reads committed local refs and
known origin tracking refs without fetching or changing any worktree. Git runs
outside database transactions against captured commit IDs.

Each overlapping pair gets one captain item naming both sides and all shared
files. Persisted file sets suppress repeats across acknowledgment and restart.
A changed set resolves the old item and opens a fresh one; a cleared overlap
resolves its item. Git errors preserve the last evidence and appear in the tick
report. These warnings inform ordering and merging; they do not block a merge.
