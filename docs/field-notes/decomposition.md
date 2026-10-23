# Field notes: decomposing work so lanes never stop — on valuable work

Running experience log kept while GEAR builds itself. **Deliverable:** at the end these notes are distilled into the core skill `gear --skill decompose` (skills/gear/references/decompose.md) and shipped with GEAR. Until then nothing here is a rule; the distillation is done with care because the skill must stay small (see also [serial-traps.md](serial-traps.md)).

The question: how do we cut work so that every lane is always busy (never-stop, GEAR's soul) **and** the work is worth doing — it lands, it is not thrown back by review, it does not need redoing? The opposite of BMAD's serial "build one, review, then the next".

Two measures to watch, per lane: **idle time with no startable task** and **rework rate** (attempts sent back / attempts handed in).

## Observation log

| Date | Task(s) | What happened | What it suggests |
|---|---|---|---|
| 2026-10-22 | plan v1 | The decompose skill already said "no lane is forced to wait; three independent tasks queued per lane" — and the captain still gave each lane one task. | A written rule is not enough. Check it mechanically: at `plan apply` (queue depth per lane) and in the engine (warn when a lane runs low — T-7). |
| 2026-10-22 | T-6 → T-2 | Dependency added because the two tasks touch nearby files. | Proximity is not a dependency. "Merge dev before handing in" handles it; a dependency is only "cannot be built without". |
| 2026-10-22 | T-1 v1 → v2 | Contract changed mid-attempt because v1 asked the lane to do something lanes may not do (operate Herdr). The lane finished to v1. | Write the role's limits into the contract from the start (who runs live probes, who installs). A contract checked before apply avoids mid-attempt changes, which cost a re-adoption or an attested verdict. |
| 2026-10-22 | T-3 round 1 | Rework: the board's tests compared strings; in a real terminal the rows drifted (raw mode). The reviewer found it by running the real path in a PTY. | Acceptance must name **how it is proven in its real environment** (a PTY run, a real git repo, a live command), not only unit asserts. That turns the reviewer's discovery into the writer's own check — rework avoided. |
| 2026-10-22 | T-3 round 2 | Rework passed first time; resumed reviewer checked only the delta (one short turn). Rework cost: one extra lane task + one short review — cheap because the finding was precise (file:line, cause, fix, how to verify). | A precise finding makes rework cheap; the review template's "write each finding so the writer can act without asking" pays off. |
| 2026-10-22 | T-6 round 1 | Rework: a race between config re-init and onboarding (two read transactions) — the reviewer reproduced it on a scratch store. The contract said nothing about concurrency with the running engine. | When a task changes data the engine reads every tick, the contract should say "the engine runs concurrently" and ask for a test of an update landing mid-tick. |
| 2026-10-22 | T-5 round 1 | Rework: two crash/restart windows (start accepted but not recorded; activated but no job sent). One more finding was stale — the branch predated a fix on dev. | For engine work the contract should name the restart points ("the engine may be restarted between any two steps") and ask for a test per point. Reviewers should review the branch merged with current dev, or the writer merges dev just before handing in. |
| 2026-10-22 | T-9, T-13 round 1 | Rework: T-9 hid empty lanes (the empty case); T-13 broke when the captain changed state mid-flow (cancel during review, contract change, duplicate messages) and one error aborted the whole tick. | Engine-automation contracts need a short list of **adversarial scenarios**: empty data, the captain changes state mid-flow (cancel / new contract / duplicate), engine restart between steps, an error in one item. Ask for one test each. Rule for engine code: one item's error never stops the tick. |
| 2026-10-22 | T-5 round 2 | The reviewer found three NEW P1s, all multi-fault coincidences (crash at an exact point + agent exit while down; cancel during start; quota lowered mid-tick). The captain's review prompt had asked for "a restart between any two steps" — the reviewer followed it to the extreme. Captain passed T-5 with the findings logged; only the slot leak became a small task. | Rework can also be the captain's review instruction: ask for likely faults (one restart, one captain action), rank by likelihood, and say what is out of scope for the skeleton. Endless review rounds are a serial trap too (perfectionism blocks the lane's flow). |
| 2026-10-22 | T-1, T-2, T-4 | Passed first time; all had acceptance naming concrete checks (FakeHerdr test shapes, live probe, manifest vs a reference file). | Concrete, checkable acceptance correlates with first-time pass. |
| 2026-10-22 | T-12 / T-13 | One feature (review verdict → merge) split across two lanes in parallel: data settles states, runtime executes the merge; the shared piece is a one-line enum both may add. | Experiment: split along the seam (state vs side effect) with the tiny shared part allowed on both sides. Watch whether it conflicts. |
| 2026-10-22 | quota | All lanes ran the same provider plan and hit its limit together; every lane stopped at once. | Spread lanes over providers (or keep a different-provider fallback ready) so one limit does not stop the whole fleet; quota handling is part of never-stop (T-15). |
| 2026-10-22 | T-11 | Queued only to keep a lane busy; duplicated T-7. Cancelled. | Never fill a queue with busywork. If a lane has no valuable work, re-plan (pull work from a busy lane, split a big task) — or let it stand by and say why. |

## Input from the user (2026-10-22)

- **Rolling horizon of 3–5 steps.** Plan only the next 3–5 steps per lane, chosen so they do not depend on each other and each stands on its own. Keep those near steps stable — avoid changing them while they run; when a lane's steps are done, plan its next ones. A BMAD-style full plan up front is only a hope.
- **Steer, don't swerve.** The captain adjusts course continuously from what it sees, like a helmsman: a big reversal (hard left to hard right) can happen but should be rare; most corrections are small, so the rowers keep rowing.
- Offered as experience to weigh, not a rule.

## Rework root causes (user, 2026-10-22: rework is not the reviewer being strict — the lanes only execute, so every miss is a constraint the captain did not hand down)

Seven reworks out of fifteen hand-ins so far. Each traces to something the captain knew, or the system had, that was not in the packet:

| Rework | What the packet did not say | Kind |
|---|---|---|
| T-3 board rows drift | prove it in a real terminal (PTY) | proof |
| T-5 reviewer stuck after restart | the engine may restart between any two steps | environment |
| T-6 config/roster race | the engine runs concurrently with `gear` commands | environment |
| T-9 empty lane hidden | why the snapshot exists: the captain must see a lane that ran dry | purpose |
| T-13 engine-wide halt | the captain cancels/changes contracts mid-flow; the version rule; one item's error must not stop the tick | invariants, adversarial |
| T-15 new conversation on restart | the known OpenCode fact: resume needs `-s <session id>`, Herdr does not report it | known facts |
| T-10 read-only command starts the engine | every `gear` call starts the engine — a hidden behaviour the captain itself added | hidden system behaviour |

**Contract checklist (try on every new task from now on, measure the rework rate):**

1. **Purpose** — why the task exists, who uses the result and for what (lets the lane judge cases the contract does not list).
2. **Environment** — what runs at the same time (engine ticks, other `gear` commands), and where it may be interrupted (restart between steps).
3. **Invariants to keep** — the system rules this code must not break (version rule, read-only, one error never stops the tick, never retype into a busy agent…).
4. **Adversarial cases** — empty data, the captain changes state mid-flow (cancel, new contract, duplicate), restart, an error in one item: one test each.
5. **Real-environment proof** — the command the writer runs before handing in (PTY, scratch store with the built binary, scratch git repo).
6. **Facts to read first** — the SOP or design section that already holds the answer (e.g. `gear --skill agent:opencode`).

## Emerging principles (candidates for the skill)

1. Queue depth is a mechanical check, not a memory.
2. Dependencies only on what a task cannot be built without; never on review or merge of a neighbour.
3. Acceptance names the real-environment proof; the writer runs it before handing in.
4. The contract states the role's limits up front; changing a contract mid-attempt is expensive.
5. Rework goes first in its lane.
6. Value over occupancy: no busywork; an idle lane is a planning signal, answered with real work.
8. Rework is a contract defect first: when a task is sent back, find which checklist item was missing and fix the packet template, not only the code.
7. Rolling horizon: 3–5 independent, stable steps per lane; re-plan a lane when its horizon runs down, with small course corrections rather than reversals.

## Open questions

- How deep should a queue be when tasks are large vs small?
- When a lane's owned area runs out of work, move the lane to another area or retire it?
- Does splitting one feature across lanes (T-12/T-13) pay off, or is one lane per feature better?
