# Decomposing work for GEAR (`gear --skill decompose`)

For the captain (or whoever plans). GEAR does not care which method produced the raw material — your own judgement, BMAD epics/stories, a superpowers plan, an issue list. It cares that the tasks you put on the board satisfy the rules below, because lanes run unattended and never wait for review.

## 1. Rules every plan must satisfy

1. **Duty first.** Each long-lived lane has a declared duty and owned paths (conventions §1). A task goes to the lane that owns the paths it touches. A task touching two lanes' paths is split, or its interface part goes first (rule 4).
2. **Every task merges into `dev` on its own** and leaves `dev` building and green. New code may be dormant (not called yet); a half-broken state may not land.
3. **No lane is forced to wait.** The first three tasks queued for each lane are mutually independent and each can start from current `dev`. A lane hands a task in and immediately starts the next one; review happens in parallel.
4. **Contract first across lanes.** When lane B will use what lane A builds, a contract task (interfaces, signatures, error codes, contract tests, a fake for consumers) is merged into `dev` before either implementation starts. A plays against the contract tests; B develops against the fake.
5. **Land-together changes** are one task, or expand → migrate → contract: add the new path, move callers, remove the old path in a separate task.
6. **Remaining dependencies are explicit** (`Depends on: T-a, T-b`). A task is `ready` only when everything it depends on is merged into `dev`. Never stack a task on another task's unmerged branch.
7. **Right size.** A task is the smallest unit that has its own test cycle and that a reviewer could reject while approving its neighbour. Fold setup, config and docs into the task whose deliverable needs them. Target: a lane finishes it within one working session.
8. **Re-plan continuously.** A lane with no startable task is a planning failure: pull independent work forward, split a task, or add a contract task. Do this at every review.

## 2. Procedure

1. **Map files before tasks.** List the files to create/modify and the single responsibility of each; split by responsibility, not by technical layer. This is where boundaries are decided.
2. **Find the seams between lanes.** For every place where one lane's code calls another's, write the contract: exact signatures and types, error behaviour, the contract tests, and a fake. That becomes a contract task (usually the captain's own, merged first).
3. **Cut tasks per lane** following rules 2, 5, 7. For each task write the packet (§3).
4. **Order each lane's queue** so rule 3 holds; declare remaining dependencies (rule 6).
5. **Self-check** (§4), fix inline, then put the tasks on the board (`gear plan apply`).

## 3. Task packet

```markdown
# T-<n> <title>

Task: T-<n> · Lane: <lane> · Branch: `<lane>/t<n>-<slug>` (cut from latest dev by the lane) · Depends on: none | T-a, T-b
Status: ready

## Goal
<one or two sentences: what exists afterwards and how it is proven>

## Read first
<conventions, the design section, the exact files>

## Interfaces
- Consumes: <exact signatures from dev this task uses>
- Produces: <exact signatures later tasks / other lanes rely on>

## Allowed changes
<exact files; everything else is off limits; the packet's own Status/checkboxes/Lane record>

## Tasks
- [ ] 1. <one action with a checkable result — names the tests that prove it>
- [ ] …

## Acceptance
<commands and the output that means pass>

## Lane record / Review follow-ups
<left for the lane and the reviewer>
```

A step is written so the implementer can write exactly one reasonable thing: a test step names the test and its assertions; a code step names the exact signature and file; a verification step gives the command and the passing output. Lines that decide nothing ("handle edge cases", "add tests") are not steps.

## 4. Self-check before applying

- **Coverage:** every requirement of the spec/milestone maps to a task.
- **Independence:** for each lane, the first three ready tasks start from current `dev` without each other.
- **Merge-safety:** each task, merged alone, leaves `dev` green.
- **Ownership:** each task touches only its lane's owned paths (plus its own packet).
- **Type consistency:** names and signatures used in later tasks match the Produces blocks of earlier ones.
- **Contracts:** every cross-lane call has a merged (or first-queued) contract task with tests and a fake.
- **Reachability:** every Goal item can be done with the packet's Allowed changes and the `pub` API already on `dev`; if not, add the API first (contract-first) or drop the item (lesson L-28). Same for follow-up packets written from review findings.
- **Proportion:** a packet longer than the code it describes is writing the code; replace bodies with signatures and test names.

## 5. Using other methods' output

- **BMAD:** an epic's stories are the raw material. A story's Acceptance Criteria → Goal/Acceptance; its Tasks/Subtasks → the checklist; Dev Notes → Read first. Split stories that cross lanes; BMAD's sprint order is not dependency-safe for parallel lanes — reorder by rule 3.
- **superpowers `writing-plans`:** its File Structure step is §2.1; its Interfaces block (Consumes/Produces) and step style are used as-is; its Task Right-Sizing matches rule 7. Its execution model is serial (one task reviewed before the next starts) — under GEAR, tasks of one plan are spread over lanes and reviewed asynchronously, so re-check rule 3 and rule 6 after converting.
- **Issue lists:** one issue may become several tasks; keep the issue reference in the packet title.
