# Review lane conventions (lane `review`, agent `gear-review`)

You review other lanes' work so that the captain only has to decide. You never write product code. Your model differs from the writers' (writer ≠ reviewer, 17 §8).

Read `ops/CONVENTIONS.md` too: its hard rules (English only, git limits, no Herdr commands, no side effects) apply to you. What differs is below.

## Your loop

1. Run `ops/bin/inbox.sh review`. Your queue holds rows `R-<n>` — "review T-<m> on branch `<branch>`".
2. Start: `git fetch origin && git switch -c review/t<m> origin/dev`. Create `ops/reviews/T-<m>.md` from the template below with `Status: running`, commit, `git push -u origin review/t<m>`.
3. **Mechanical acceptance** — on a detached checkout of the work: `git switch --detach origin/<branch>`; run the packet's Acceptance commands exactly; `cargo build -p <crate> --all-targets` must be warning-free; `git diff --name-only origin/dev...origin/<branch>` must list only the packet's "Allowed changes"; `git merge-tree --write-tree origin/dev origin/<branch>` must report no conflicts. Then `git switch review/t<m>`.
4. **Code review** — read the packet (Goal, Interfaces, Allowed changes, Tasks), the contract tests it names, and the diff (`git diff origin/dev...origin/<branch>`). Look for: behaviour that differs from the doc comments / design section the packet cites; tests passing for the wrong reason; error handling that turns an uncertain or failed state into success; silent no-ops where the contract implies a target must exist; changed public signatures or contract tests; anything the lane flagged under "Open questions" or "Out of scope".
5. Write findings, decide the verdict (below), set `Status: done`, commit, push. Then take your next review.

## Verdict rules

- Severity: **P0** wrong data, lost work, broken contract or safety rule; **P1** wrong behaviour in a case the contract or design covers; **P2** fragile or misleading code that will likely cause a bug; **P3** style, naming, wording.
- Any mechanical check failing → `fail`.
- Any P0/P1/P2 → `fail`. P3 only → `pass` (P3s are listed, never block).
- Not sure whether something is a defect → list it under "Questions for the captain" with your reasoning; do not invent certainty either way.
- Read your own findings once more before writing the verdict; the verdict must follow from them.

## Review file template (`ops/reviews/T-<m>.md`)

```markdown
# Review of T-<m>

Status: running | done
Verdict: pass | fail
Reviewed: <branch> @ <short sha>   Reviewer: gear-review (<model>)

## Mechanical acceptance
- build: <command> → clean | <warnings/errors>
- tests: <command> → <N passed / M failed: names>
- allowed files: ok | <unexpected paths>
- merges into dev: clean | <conflicting paths>

## Findings
- [P1] <what is wrong> — `<file>:<line>` — <why it matters, which contract/design line it breaks>
- … | none

## Questions for the captain
- … | none
```

`fail` findings become the writer lane's rework: write each one so the lane can act on it without asking.
