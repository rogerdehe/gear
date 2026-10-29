# Review (requirements for whoever reviews a task)

Reviews are done by **temp reviewers**: one fresh agent session per review, started by the captain (later the engine), on a different model from the writer. A re-review of the same task after rework resumes that reviewer's session. Conventions 17 §8 is the rule; this file is what the reviewer itself must deliver. **How** you review is up to you — your own approach, BMAD `code-review`, superpowers `requesting-code-review` all fit — as long as the requirements below hold.

The general lane rules in the voyage's `CONVENTIONS.md` apply (English only, git limits, no Herdr commands, no side effects).

## Requirements

1. **Independent.** Judge the work from the task packet, the contract tests and documents it names, and the diff. The writer's Result and notes are claims to verify, not facts. Never change the reviewed branch.
2. **Verified facts (mechanical acceptance).** Run them yourself on the reviewed commit; do not copy the writer's numbers:
   - the packet's Acceptance commands pass, run scoped to what the candidate touched: `make check CRATE=<crate> TESTS='<nextest filter for the changed modules and test files>'` (the merge gate runs the whole suite; a whole-crate run of your own is not needed); `cargo build -p <crate> --all-targets` has zero warnings;
   - `git diff --name-only origin/dev...origin/<branch>` lists only the packet's "Allowed changes";
   - `git merge-tree --write-tree origin/dev origin/<branch>` reports no conflicts.
   - test times: a test over 10 s that the candidate added or changed is a finding; one it did not touch is not a failed fact of this task (the suite runs while other lanes build) — list it under Set aside, never as blocking.
3. **Required conclusions**, written to `gear/voyages/<M>/reviews/T-<n>.md` (template below):
   - spec compliance: what is missing, extra, or misunderstood against the packet;
   - **every blocking finding carries its fix**: the smallest change that satisfies the milestone bar — file, function, what to change, about how many lines — and never a redesign or refactor (the writer executes it as written; the captain may pick a smaller one). Read the bar first: under "fastest to the goal" only what breaks the chain or loses data is blocking;
   - findings with severity, `file:line`, and why it matters — **P0** wrong data, lost work, broken contract or safety rule; **P1** wrong behaviour in a case normal operation reaches (one fault at a time: one restart, one captain action); stacked coincidences are P3 with their likelihood; **P2** fragile or misleading code likely to cause a bug; **P3** style/wording;
   - what you considered and set aside, with the reason (nothing dropped silently);
   - questions only the captain can decide;
   - verdict: judged against the **current milestone's done-sentence** (the `# Milestone` block of the voyage's `gear/voyages/<M>/plan.md`), not against perfection. `fail` = a failed mechanical fact, a P0, or a finding that breaks that sentence; everything else is listed with its severity and does not block (`pass`). You review one screw; the milestone sentence says how tight the ship needs it now.
4. **Re-review** (same task, resumed session): check every previous finding is resolved, review only the delta, and still report anything new.

## Delivering

- Check out the reviewed commit **inside your own worktree**: `git fetch origin && git switch --detach origin/<branch>`, run the checks, then `git switch review/t<n>`. Never create another git worktree or clone (worktrees are Herdr's job; lesson L-18).

- Work on branch `review/t<n>` (the captain created your worktree on it). Write `gear/voyages/<M>/reviews/T-<n>.md` with `Status: running` first, commit, push; at the end set `Status: done`, commit, push, and end your turn. You review exactly one task.

## Template (`gear/voyages/<M>/reviews/T-<n>.md`)

```markdown
# Review of T-<n>

Status: running | done
Verdict: pass | fail
Reviewed: <branch> @ <short sha>   Reviewer: <agent name> (<model>)   Round: 1 | 2 | …

## Mechanical acceptance
- build: <command> → clean | <warnings/errors>
- tests: <command> → <N passed / M failed: names>
- allowed files: ok | <unexpected paths>
- merges into dev: clean | <conflicting paths>

## Spec compliance
- ok | missing: … | extra: … | misunderstood: …

## Findings
- [P1] <what is wrong> — `<file>:<line>` — <why it matters>
- … | none

## Set aside
- <behaviour considered> — <why outside this task> | none

## Questions for the captain
- … | none
```

Do not report the Result's `Branch:` sha naming the code commit rather than the hand-in tip, or the absence of diff counts — that is the format (the voyage's CONVENTIONS.md, lesson L-32).

A `fail` becomes the writer's rework: write each finding so the writer can act on it without asking.
