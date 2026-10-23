# 06 Tasks and Attempts

Implementation: [tasks.rs](../../crates/gear-app/src/tasks.rs).
Invariants upheld: one writer per write resource (together with 10 and the resources table); completion is bound to a contract and an output version; an old attempt cannot complete a new contract.

## Responsibilities

- State changes of tasks and attempts (per 02's transition table).
- At most one non-terminal attempt per task at any time; at most one **in-progress** attempt (DELIVERED/STARTED/STOPPED/UNKNOWN) per seat at any time.
- Contract version switching, preemption safe points, acceptance settlement.

## Not done

Does not judge when to continue (09); does not parse plan drafts (05); does not run verification (12).

## Key rules

- **Contract version**: plan apply changes the contract → new version; if there is an in-progress attempt, generate a needs_ack SYSTEM message "T-17 contract updated to v4". The attempt stays bound to `adopted_version` (the task packet renders the adopted version and marks "pending adoption v4") until `adopt_contract`. A result submitted under the old version is kept as a fact, but **cannot complete the new contract** (acceptance is checked against the task's current contract).
- **Safety conditions for taking the next item**: SUBMITTED only means a candidate is registered. Before a long-lived lane takes the next item: the original attempt has reached a safe end point, the work has a persistent copy (committed or a checkable persistent copy), there is no known unfinished activity or delivery needing check, and the new task's baseline has been checked (see below). Otherwise wait, and the captain decides. A temp worker is always bound to its own task and never takes other tasks from the queue.
- **Preemption**: first `preempt_request`, wait for the lane's `ack --preempted` and check the saved reference before moving to ABANDONED; registering a dirty tree only preserves the work and does not authorize mixing a new task into the same tree. When the MVP has no safely isolatable resource, the new task waits or the captain reassigns it to another lane.
- **Acceptance**: `settle(passed)` → attempt ACCEPTED, task DONE; `settle(failed)` → attempt REJECTED, task READY with the rework note saved; normal dispatch then creates a new attempt (a temp-worker task brings back the original temp worker via 10's resume path).

- **Dependency waiting**: ordinary waiting keeps the same attempt; when the resume condition is met, WAITING→RUNNING and the original attempt continues. Only when the old attempt has been explicitly settled/terminated and needs re-dispatch does it go back to READY.
- **Baseline for the next item**: a clean working tree or committed changes only prove the work is saved, not that the new task's baseline is isolated. Dispatching the next item must check the baseline accepted by the new task, and must not implicitly take the old task's unaccepted commits as input to the new task; without a safe resource, wait or reassign, no automatic migration.
- **Temp workers**: after onboard/resume completes, 10 calls `start_attempt` for its originally bound READY task and registers the first deliver; a READY task that already has a DELIVERED/UNKNOWN attempt is not first-dispatched a second time.

## Test points

Two in-progress attempts on one seat are rejected; next item not taken when submitted but working tree not clean; an old-version result does not complete the new contract but its original text is retrievable; adopt lowering the version is rejected; ack without a saved reference is rejected; after acceptance failure back to READY, a new attempt works normally.
