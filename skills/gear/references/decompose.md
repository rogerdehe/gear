# Cutting work into tasks (`gear --skill decompose`)

For the captain. GEAR does not care which method produced the raw material — your own judgement, BMAD epics and stories, a superpowers plan, an issue list. It cares that every task you put in the plan satisfies the rules below, because lanes run unattended and never wait for a review.

## 1. Rules

1. **The owner's paths.** A task goes to the lane that owns the paths it touches; the paths are relative to that lane's repo. A task touching two lanes' paths is split, or meets the other at an interface (rule 3). A task changes one repo.
2. **Every task merges on its own** and leaves the integration branch building and green. New code may be dormant (not called yet); a half-broken state may not land.
3. **No lane waits.** `deps:` names what a task cannot start without. Write shared interfaces into both goals so the lanes can start. For a task that continues an earlier task's work, set `followup-of: T-<n>`: the same lane and CLI can keep or resume that task's accepted session, compacting if large; otherwise it starts fresh and reads the earlier report. Unrelated work starts a fresh session.
4. **Changes that must land together** are one task, or expand → migrate → contract (add the new path, move callers, remove the old one in a separate task).
5. **Right size: at most 60 minutes** (the project's `[conventions] max_task_min`; apply refuses more). The smallest unit with its own test cycle that a reviewer could reject while passing its neighbour. A lane works unattended and is reviewed only at its hand-in, so a long task is a long stretch nobody checks, and one wrong turn costs all of it: bigger work is cut into several tasks of the same lane, in queue order. Fold setup, config and docs into the task whose result needs them.
6. **Every `NEW` task has an `estimate:`** in minutes — the measured pace of the voyage is built from them (`gear --skill course` §3).

## 2. Writing a task

A task in the plan is a header plus `goal`, `accept`, `estimate`, `deps`. The engine turns it into the lane's packet together with the voyage's `done:` and `bar:`.

- **goal** — what exists afterwards and why (who uses it, for what). Add what the lane cannot see: what runs at the same time, the rules it must not break, the cases that are easy to miss (empty data, a change mid-flow, a restart, one item failing), the SOP or file that already holds the answer. A lane working in a child repo cannot see the parent repo's documents: write their absolute path into the goal.
- **accept** — checkable results, including **how it is proven in its real environment** (a real terminal, a real repository, a real agent) when the task touches one. Concrete acceptance is the best predictor of a first-time pass. When the crate's suite is slow (over a minute), the named check carries `TESTS=` with the changed module's tests. It names the exact capped check the lane runs (Run only `make check <the package this task changes>`; the merge gate runs `make gate`), never a whole-repo build or test.
- Write it so the lane cannot misread it. Lines that decide nothing ("handle edge cases", "add tests") are not goals.

## 3. Procedure

1. **Map files before tasks.** List the files to create or change and the single responsibility of each; split by responsibility, not by technical layer.
2. **Find the seams between lanes** and write their interfaces (rule 3).
3. **Cut the tasks per lane** (rules 2, 5), each with goal, accept, estimate.
4. **Order each lane's queue** so rule 3 holds; declare the real dependencies.
5. **Self-check**, then `gear plan apply`.

## 4. Self-check

- **Coverage:** every part of the voyage's `done:` maps to a task.
- **Size:** no estimate over the cap; the whole voyage is in the plan before departure.
- **Independence and balance:** each lane's queued tasks start without each other, and the lanes' startable work (after `deps:`) ends near the same time. When one lane holds most of it, re-split the owned paths for the whole wave at once, not one task per run-dry.
- **Merge-safety:** each task, merged alone, leaves the integration branch green.
- **Ownership:** each task touches only its lane's paths, in one repo.
- **Reachability:** the goal can be done with what is on the integration branch plus the interfaces written into it; every function, field, list and test the task names exists — grep each one, and read what a list it feeds into triggers.
- **Old tests:** a change of existing behaviour includes updating the tests that assert the old behaviour.
- **Proportion:** a task description longer than the code it asks for is writing the code; give signatures and test names instead.

## 5. From other methods

- **BMAD stories:** acceptance criteria → `accept`, the story's intent → `goal`. Split stories that cross lanes; BMAD's order is serial — reorder by rule 3. Keep the bar: a story's review depth is the voyage's bar, not BMAD's.
- **superpowers plans:** the file-structure step is §3.1; their tasks are reviewed one after another — under GEAR they spread over lanes and are reviewed in parallel, so re-check rules 3 and 6.
- **Issue lists:** one issue may become several tasks; keep the issue reference in the goal.
