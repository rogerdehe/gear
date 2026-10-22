# 10 Staff: long-lived lane standby, temp workers (incl. resume), retirement, missing seats

File: `gear/engine/staff.py`. Budget 240–300 lines.
Invariants upheld: one writer per write resource (when uncertain, do not hand over); results are never lost; quota is never bypassed.

## Long-lived lanes

**Onboarding and worktree**: config contains no hand-written worktree paths (01). At onboarding the engine calls Herdr `worktree.create` once (branch `home/<lane>`, base `dev`, label `<project>-<lane>`), records the returned workspace/pane/path in `resources` (held by this seat), then `pane_run` exports `GEAR_CTX` → `agent_start`. The lane holds this worktree long-term; each task branch `<lane>/<task-id>-<slug>` is switched to from `dev` inside it (09, 17 §3–§4). The worktree is removed only after the lane is RETIRED and all its results are merged.

| State | Entered when | Behavior | Leaves when |
|---|---|---|---|
| ACTIVE | Has an attempt in progress | 09 continues | Result submitted and no next item → STANDBY |
| STANDBY (standby) | No assignable task | **Sends no messages at all**; records start time; included in review snapshot; triggers 11's "idle lane" | A task becomes ready → assign immediately |
| PARKED | Plan draft `park … until=/until-task=` | No assignment | Time reached / condition met → re-evaluate (assign if there is work, otherwise STANDBY); a new ready task also ends it early |
| HELD | Draft `hold … reason=` | No assignment; new tasks do not interrupt either | Only the draft releasing the hold |
| RETIRING | Draft `retire` | No new work; in-flight attempts go to a safe point via 06's preemption flow; unstarted tasks go back to unassigned (lane cleared, captain reassigns); submitted ones continue through acceptance | Handover complete → RETIRED; captain receives a merged review item |
| MISSING | 08 observes that the pane/Agent of a seat expected to have an Agent has disappeared | No assignment; in-flight attempts frozen; unstarted tasks may be reassigned by the captain | User registers a new session → NEEDS_CHECK |
| NEEDS_CHECK | Occupant changed / re-registered after going missing | No assignment; worktree `writer_ok=0` | User runs `gear admin seat confirm <seat>` (confirms write resources are safe) → ACTIVE |

**Write rights**: a worktree held by a MISSING/NEEDS_CHECK lane is **never** automatically handed to another lane; closing a pane does not prove background jobs have stopped. Reassigning it to another lane requires user confirmation (`gear admin seat confirm`) first. The MVP requires that managed tasks do not create write jobs detached from the terminal (stated in the lane instructions).

## Temp workers

**Quota and resources**: project config `quota_temps`; STARTING, ACTIVE, RELEASING, UNKNOWN all occupy quota; QUEUED, RELEASED do not. Start reserves quota in **one transaction**; there is no pool of pre-registered panes or worktrees (no slots). Concurrent starts can neither exceed the quota nor create two worktrees for the same task. Planned headcount and session quota are counted separately; QUEUED happens only when the quota is exhausted.

**Start** (draft `temp R-4 task=T-45 profile=…`; the same target appearing repeatedly does not start it repeatedly):

1. Transaction: check that the profile is verified (04) → reserve quota → seat STARTING → `intend(onboard)`.
2. Engine: Herdr `worktree.create` (branch = the task branch `<lane-or-temp>/<task-id>-<slug>`, base `dev`, label `<project>-<task-id>`) → record the returned workspace/pane/path in `resources` (held by this seat) → `pane_run` sets `GEAR_CTX` → `agent_start`.
3. Expected Agent observed → `onboard_complete` (03) records the fingerprint and **session_ref** (for profiles that declare the reference is obtainable, fetch it at onboarding; for CLIs that only print the reference on exit, capture it via the profile's exit flow) → the first task-packet pointer and the skill entry pointer are combined into a single delivery (16), not sent twice. Start timeout / unknown outcome → UNKNOWN (occupies quota); no second one is opened.

**First dispatch**: after onboard is confirmed, this module calls 06 `start_attempt` for the READY task the temp worker was originally bound to and registers the first deliver; 09 only continues established attempts and never takes other tasks from the queue.

**Automatic release after normal delivery** (executed only when all are met):

- The temp worker's `gear send --kind result --task … --attempt … --contract …` is registered and states the artifact path and safe end point;
- The artifact files exist; changes to be kept are committed or have a checkable durable copy (only registering a path or a dirty-tree digest does not count);
- Two consecutive observations of idle/done, no busy on screen, no known unfinished activity, no in-flight delivery.

Met → seat RELEASING → `intend(release)`: make the Agent exit via the profile's `exit_keys` → confirm the Agent has exited and session_ref is saved → the worktree is **kept** (its ownership record reads "released normally by R-4 at T-45") → seat RELEASED → quota released. `worktree.remove` is called only after the result has been merged; it removes the checkout and the Herdr workspace and keeps the branch (verified live). If any condition is unclear → do not close; give the captain an "R-4 needs check" item. Releasing quota does not mean acceptance passed, nor does it hand the worktree to anyone else.

**resume rules (user decision)**:

- **New work = new temp worker** (new R number, new session).
- **The same work needs it back** (rework after failed acceptance, follow-up questions, supplementary material): the draft writes `temp R-2 task=T-31 resume`; only allowed for the task R-2 was originally bound to.
- Execution: reserve quota inside a transaction → Herdr `worktree.open --branch <task-branch>` (back to the original worktree; its ownership record must be a normal release by this R number with no later use by anyone else and no unknown activity, otherwise needs check) → `intend(onboard, resume_ref=<session_ref>)` → `agent_start(args=resume_args(session_ref))` → confirm return to the original session using the profile's method → deliver the "rework instructions" pointer.
- Outcome distinctions:
  - resume explicitly unsupported (profile has no resume) or recovery confirmed to have failed → a new session reads the original task packet + rework instructions; seat `note=new-session recovery`, clearly marked on the home page and the board;
  - start timeout, lost receipt, unclear whether it returned to the original session → UNKNOWN, **do not open a new session**, wait for the check.
- A profile that never obtained a session_ref must not be marked "resume verified".
- **Follow-up questions and rework are separate**: for a follow-up on a VERIFYING/DONE task, after resume deliver a follow-up item linked to that task; do not create a new attempt, do not re-run the implementation, do not change the existing candidate; after the follow-up is handled the safe end point must be declared again, and the safe_point of the previous delivery is not reused to close automatically. When rework is needed: a VERIFYING task goes through a fail in `# Verdicts`; a DONE task goes through `state=ready` + `reopen:` (05); then create the next attempt and resume the original temp worker. **Follow-up only**: VERIFYING/DONE state and the original candidate are unchanged; after answering, re-declare the safe end point; it is neither needed nor allowed to fake a result or fail to close the session.
- Resources: a task's worktree belongs to that task and is never handed to a new task; every new task gets its own `worktree.create`. Before use, check `resources` transactionally; resume of the original task is checked against the original ownership record.

## Roster after retirement or replacement

The home page and board show: planned number of long-lived lanes, number available, number retiring/missing/needs check and their un-handed-over tasks; temp workers: in use/quota, queued, list of released-but-resumable. Planned capacity is adjusted only after retirement completes.

## Test points

Standby sends no messages, re-evaluation when due, new task wakes it early; hold is not interrupted; temp workers over quota are queued; concurrent temp starts do not exceed quota or create two worktrees for the same task; still writing after result → not closed; close unconfirmed → quota not released; worktree removed only after merge, branch kept; resume for a new task rejected; resume failure falls back to a new session and is labelled; a missing lane's worktree is not automatically reassigned.
