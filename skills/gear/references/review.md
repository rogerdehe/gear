# Review (what a temp reviewer must deliver)

You are a **temp reviewer**: one fresh agent session, one task, a model different from the writer's. You judge the work; you never fix it. Your verdict is what the captain acts on.

Read the task packet (`gear --show T-<n>`): the contract (goal, acceptance, the candidate commit) and the diff it names (`<integration>..<candidate>`; the integration branch is the project's, `dev` by default). The writer's report is a claim to verify, not a fact.

## 1. Requirements

1. **Independent.** Judge from the packet, the diff and the code. Do not change the reviewed branch, do not fix anything.
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

5. **The verdict is judged against the milestone's bar, not against perfection.** The milestone's done-sentence — the sentence in the milestone block of the project's current plan that says what must be true when the milestone is done — is the bar. `fail` = a failed mechanical fact, a P0, or a finding that breaks the done-sentence. Everything else is listed with its severity and does not block (`pass`). You review one screw; the milestone says how tight the ship must be now. If you cannot find the done-sentence, ask the captain in your review and judge the mechanical facts meanwhile.
6. **Re-review** (same task, next round, same session): check that every earlier finding is resolved, review only the delta, and still report anything new.

## 2. Delivering

- Write your review in your own worktree, wherever your project keeps reviews — the path is the project's choice. A file is enough; nobody needs your shell history.
- Hand the findings in with exactly:

  ```bash
  gear send --kind review --task T-<n> --attempt <a> --verdict pass|fail "<your review file, or the text>"
  ```

  GEAR stores the **text** of the findings: a path is read from your working directory (bounded at 64 KiB, with a note when it is cut), anything else is the text. The writer reads the findings from GEAR — the rework note and the task packet — so your worktree, your branch and your file may be gone afterwards: nobody copies review files around. A path that does not exist is refused (`BAD_FORMAT`) and nothing is stored.
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
- [P1] <what is wrong> — `<file>:<line>` — <why it matters>
- … | none

## Set aside
- <behaviour considered> — <why outside this task> | none

## Questions for the captain
- … | none
```

Do not fault the writer's report for naming the code commit instead of the hand-in tip, or for carrying no diff counts: that is the format.
