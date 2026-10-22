# Review (requirements for whoever reviews a task)

Reviews are done by **temp reviewers**: one fresh agent session per review, started by the captain (later the engine), on a different model from the writer. A re-review of the same task after rework resumes that reviewer's session. Conventions 17 §8 is the rule; this file is what the reviewer itself must deliver. **How** you review is up to you — your own approach, BMAD `code-review`, superpowers `requesting-code-review` all fit — as long as the requirements below hold.

The general lane rules in `ops/CONVENTIONS.md` apply (English only, git limits, no Herdr commands, no side effects).

## Requirements

1. **Independent.** Judge the work from the task packet, the contract tests and documents it names, and the diff. The writer's Result and notes are claims to verify, not facts. Never change the reviewed branch.
2. **Verified facts (mechanical acceptance).** Run them yourself on the reviewed commit; do not copy the writer's numbers:
   - the packet's Acceptance commands pass; `cargo build -p <crate> --all-targets` has zero warnings;
   - `git diff --name-only origin/dev...origin/<branch>` lists only the packet's "Allowed changes";
   - `git merge-tree --write-tree origin/dev origin/<branch>` reports no conflicts.
3. **Required conclusions**, written to `ops/reviews/T-<n>.md` (template below):
   - spec compliance: what is missing, extra, or misunderstood against the packet;
   - findings with severity, `file:line`, and why it matters — **P0** wrong data, lost work, broken contract or safety rule; **P1** wrong behaviour in a covered case; **P2** fragile or misleading code likely to cause a bug; **P3** style/wording;
   - what you considered and set aside, with the reason (nothing dropped silently);
   - questions only the captain can decide;
   - verdict: any failed fact or any P0/P1/P2 → `fail`; otherwise `pass` (P3s listed, never blocking).
4. **Re-review** (same task, resumed session): check every previous finding is resolved, review only the delta, and still report anything new.

## Delivering

- Work on branch `review/t<n>` (the captain created your worktree on it). Write `ops/reviews/T-<n>.md` with `Status: running` first, commit, push; at the end set `Status: done`, commit, push, and end your turn. You review exactly one task.

## Template (`ops/reviews/T-<n>.md`)

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

A `fail` becomes the writer's rework: write each finding so the writer can act on it without asking.
