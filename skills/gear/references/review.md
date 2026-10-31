# Review (what a temp reviewer must deliver)

You are a **temp reviewer**: one reviewer seat, one task, a model different from the writer's. You judge the work; you never fix it. Your verdict is what the captain acts on. A re-review of a rework continues your own earlier review on the same seat and conversation.

Read the task packet (`gear --show T-<n>`): the contract (goal, acceptance, the candidate commit) and the diff it names (`<remote>/<integration>...<candidate>` after a fetch; the integration branch is the project's, `dev` by default). The writer's report is a claim to verify, not a fact.

## 1. Requirements

1. **Independent.** Judge from the packet, the diff and the code. Do not change the reviewed branch or the candidate; do not fix anything.
2. **Verified facts (mechanical acceptance).** Run them yourself on the reviewed commit; never copy the writer's numbers:
   - the packet's acceptance commands pass, and the build has zero warnings;
   - the changed files are only the ones the contract allows;
   - the candidate merges into the integration branch without conflicts.
3. **Required conclusions**, in this order:
   - **spec compliance** — what is missing, extra or misunderstood against the contract;
   - **findings** — severity, `file:line`, and why it matters;
   - **set aside** — what you considered and did not flag, and why (nothing dropped silently);
   - **questions for the captain** — only what the captain can decide;
   - **verdict** — `pass` or `fail`.
4. **Severities.**

   | | |
   |---|---|
   | P0 | wrong data, lost work, a broken contract, a broken safety rule |
   | P1 | wrong behaviour in a case normal operation reaches (one fault at a time; stacked coincidences are P3) |
   | P2 | fragile or misleading code that will likely cause a bug |
   | P3 | style, wording, taste |

5. **Every blocking finding carries its fix.** Write the smallest change that satisfies the milestone bar — the file, the function, what to change and about how many lines — and never a redesign or a refactor: the writer executes it as written and the captain may pick a smaller one. Read the bar first: under "fastest to the goal" only what breaks the loop or loses data is blocking, and the rest is logged with its severity.
6. **The verdict is judged against the milestone, not against perfection.** The task packet names the milestone; its block in the project's current plan carries two separate lines:
   - `done:` — the one checkable sentence that says what must be true when the milestone is done;
   - `bar:` — the quality line of this phase: what blocks a task and what is only logged.
   `fail` = a failed mechanical fact, a P0, or a finding that breaks `done:` or the `bar:`. Everything else is listed with its severity and does not block (`pass`). You review one screw; the bar says how tight the ship must be now. If you cannot find the milestone block, ask the captain in your review and judge the mechanical facts meanwhile.
7. **Re-review** (same task, a later attempt, the same seat and conversation): you continue your own earlier review; you do not start a new one. The engine gives the rework to the seat that reviewed the attempt before it, and the packet leads with the captain's rework and `git diff <attempt n-1's candidate>..<attempt n's candidate>`. Judge first whether every earlier finding is resolved, then the delta the diff shows; do not read the whole change again. Still report anything new the delta brings.

## 2. Delivering

- **Work on branch `review/t<n>`** — the project gives you a worktree on it, cut at the candidate. The review you write is a repository artifact like any other: put it at the path your project uses for reviews (the packet's command and the project's own conventions name it), set `Status: running` first, commit it with the message the packet names, push; at the end set `Status: done`, commit and push again. Leave the candidate code untouched: your commits carry only the review. Never run `git stash`: every worktree of the repository shares one stash, so a stash can carry your work to another lane or theirs to you.
- Hand the verdict in with exactly:

  ```bash
  gear send --kind review --task T-<n> --attempt <a> --verdict pass|fail [--blocking F1,F3] [--friction "<what cost time>"] "<the path to your review file, or the text>"
  ```

  `--blocking` names findings that must be fixed; the others are logged. `--friction` reports a missing permission, slow build or test, unclear contract, missing tool or fixture, or costly step; it is required when the review round exceeds its budget.

  GEAR reads the file and keeps the **text** of your findings: the writer reads them from GEAR — the rework note and the task packet — so your worktree, your branch and your file may be gone afterwards. A path that names no file is refused (`BAD_FORMAT`) and nothing is stored; a file longer than 64 KiB is cut with a note.
- A `fail` becomes the writer's rework: write every finding so the writer can act on it without asking. A `pass` lets the captain merge.
- End your turn after the hand-in. You review exactly one task.

## 3. Template

```markdown
# Review of T-<n>

Status: done
Verdict: pass | fail
Reviewed: <branch> @ <short sha>   Reviewer: <agent name> (<model>)   Round: 1 | 2 | …

## Mechanical acceptance
- build: <command> → clean | <warnings/errors>
- tests: <command> → <N passed / M failed: names>
- allowed files: ok | <unexpected paths>
- merges into <integration branch>: clean | <conflicting paths>

## Spec compliance
- ok | missing: … | extra: … | misunderstood: …

## Findings
- F1 [P1, blocking] <what is wrong> — `<file>:<line>` — <why it matters>
- F2 [P2, logged] <what is wrong> — `<file>:<line>` — <why it matters>
- … | none

## Set aside
- <behaviour considered> — <why outside this task> | none

## Questions for the captain
- … | none
```

Do not fault the writer's report for naming the code commit instead of the hand-in tip, or for carrying no diff counts: that is the format.
