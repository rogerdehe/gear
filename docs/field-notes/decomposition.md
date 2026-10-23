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
| 2026-10-22 | T-1, T-2, T-4 | Passed first time; all had acceptance naming concrete checks (FakeHerdr test shapes, live probe, manifest vs a reference file). | Concrete, checkable acceptance correlates with first-time pass. |
| 2026-10-22 | T-12 / T-13 | One feature (review verdict → merge) split across two lanes in parallel: data settles states, runtime executes the merge; the shared piece is a one-line enum both may add. | Experiment: split along the seam (state vs side effect) with the tiny shared part allowed on both sides. Watch whether it conflicts. |
| 2026-10-22 | T-11 | Queued only to keep a lane busy; duplicated T-7. Cancelled. | Never fill a queue with busywork. If a lane has no valuable work, re-plan (pull work from a busy lane, split a big task) — or let it stand by and say why. |

## Input from the user (2026-10-22)

- **Rolling horizon of 3–5 steps.** Plan only the next 3–5 steps per lane, chosen so they do not depend on each other and each stands on its own. Keep those near steps stable — avoid changing them while they run; when a lane's steps are done, plan its next ones. A BMAD-style full plan up front is only a hope.
- **Steer, don't swerve.** The captain adjusts course continuously from what it sees, like a helmsman: a big reversal (hard left to hard right) can happen but should be rare; most corrections are small, so the rowers keep rowing.
- Offered as experience to weigh, not a rule.

## Emerging principles (candidates for the skill)

1. Queue depth is a mechanical check, not a memory.
2. Dependencies only on what a task cannot be built without; never on review or merge of a neighbour.
3. Acceptance names the real-environment proof; the writer runs it before handing in.
4. The contract states the role's limits up front; changing a contract mid-attempt is expensive.
5. Rework goes first in its lane.
6. Value over occupancy: no busywork; an idle lane is a planning signal, answered with real work.
7. Rolling horizon: 3–5 independent, stable steps per lane; re-plan a lane when its horizon runs down, with small course corrections rather than reversals.

## Open questions

- How deep should a queue be when tasks are large vs small?
- When a lane's owned area runs out of work, move the lane to another area or retire it?
- Does splitting one feature across lanes (T-12/T-13) pay off, or is one lane per feature better?
