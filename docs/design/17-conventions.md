# 17 Default working conventions

GEAR ships one default way of working. `gear admin init` applies it; a project may override the items listed in §11 through `[conventions]`, but if it changes nothing, this is how its captain and lanes work. The same text ships in the version-matched skill (16), so the captain and every lane read identical rules via `gear --skill conventions`.

Purpose: when several agents build one product over weeks, every name, branch, worktree and report must still be traceable by a human and by the captain, and nobody — human or agent — may grade their own work.

Lineage: §5–§9 absorb the collaboration rules of the user's BMAD-Team skills (claim, dev, review, merge, review-loop discipline, fallback idempotency). BMAD-Team uses GitHub PRs and HTML markers as its message bus because it has no shared daemon; GEAR has one (the SQLite store + engine), so the *rules* carry over while the *transport* becomes `gear send` / `gear reply` / the plan.

## 1. Duty first, then a lane

- Every long-lived lane declares in project config: `duty` (one sentence), `owns` (directories/modules it is responsible for), `not` (explicitly out of scope, may be empty).
- A lane whose duty cannot be stated is not opened — the split is not thought through yet. `init` rejects lanes without `duty`/`owns`, and lanes whose `owns` overlap.
- A change of duty is a plan change (05), not a rename: retire the old lane, open a new one.
- Temp workers have no duty of their own; their task is their scope.

## 2. Names

| Object | Rule | Example |
|---|---|---|
| Lane name (`name` in config) | one short word from its duty, `[a-z][a-z0-9-]*` | `data`, `runtime`, `heat` |
| Agent name / Herdr workspace label | `<project>-<lane>`, ≤ 32 chars (Herdr agent names: `[a-z][a-z0-9_-]{0,31}`) | `gear-data`, `atlas-heat` |
| Temp worker | `<project>-<task-id>`; the task is its direction; no random names | `gear-t12` |
| Captain | `<project>-captain` | `harbor-captain` |
| Task id | `T-<n>` in text, `t<n>` in branch names | `T-12`, `t12` |

The same word appears in the Herdr sidebar, the agent name, the branch and the GEAR home view. The project prefix keeps several projects on one Herdr server collision-free. Names are derived, never invented ad hoc (`lane-a`, `agent-3`, `bot` are not allowed).

## 3. Branches

| Branch | Role | Who merges into it |
|---|---|---|
| `main` | released / production state | `dev` (or `staging`) after milestone acceptance, confirmed by the user |
| `staging` | pre-release environment; **optional**, only if the project has one | `dev` |
| `dev` | daily integration; always builds and passes tests | captain, after reviewing a task branch |
| `<lane>/<task-id>-<slug>` | task branch, cut from `dev`, merged back, then deleted | the lane commits |
| `home/<lane>` | anchor branch of a lane's long-lived worktree; never committed to | — |

- `home/<lane>` exists because git cannot hold both a branch `data` and a branch `data/t1-…`.
- Lanes never merge, rebase onto shared branches, push, or delete branches. Merging and cleanup are the captain's job (§8).
- Merges into `dev` always name the base explicitly; nothing reaches `main` except through the milestone gate.

## 4. Worktrees — always through Herdr

Herdr creates and removes every worktree; its config decides the path (`<worktrees.directory>/<repo>/<branch slug>`, `/` becomes `-`). GEAR never invents paths.

| Who | Created | Used | Removed |
|---|---|---|---|
| Lane | once at onboarding: `worktree create --branch home/<lane> --base dev --label <project>-<lane>` | held long-term; each task branch is cut from `dev` inside it; build caches persist | when the lane retires and all its work is merged |
| Temp worker | per new task: `worktree create --branch <task-branch> --base dev --label <project>-<task-id>` | this task only | after its result is merged: `worktree remove` (removes checkout + workspace, keeps branch) |
| Temp rework | `worktree open --branch <task-branch>` | back into the same tree, resume the original session (10) | as above |

- One agent, one worktree; one writer per worktree at a time. Nobody edits in the main checkout.
- Closing a session, removing a worktree and deleting a branch are three separate acts; uncommitted changes, unmerged commits and evidence are never lost (PRD §9).

## 5. Claiming work

- Work is visible before it starts: a lane only works on a task that GEAR has assigned to it (an attempt exists). Lanes never pick their own work, and never start a second task while one is open.
- In-flight cap: one open attempt per lane; at most one more queued behind it. Context drift makes a third unreliable.
- A lane that drops a task (blocked, out of scope, lost context) says so with `gear send --kind blocked`; the captain reassigns. No silent abandonment, no zombie branches.

## 6. The task loop (lane side)

1. **Orient.** Read the task packet completely, the files it lists, and recent history of the touched area. If the packet says `confirm: scope`, send a short orient brief (`--kind progress`: what you understood, which approach you chose) and wait for the reply. Otherwise proceed — the captain may still interrupt.
2. **Reproduce before fixing** any bug-shaped task. If you cannot observe the bug, your model is wrong: stop and ask.
3. **Implement the minimum.** Only files the packet allows. No side refactors; anything noticed in passing goes into the final brief under "out of scope".
4. **Verify at the depth the change deserves:**

   | Change | Minimum verification |
   |---|---|
   | docs / pure refactor | build clean + tests of the touched unit |
   | logic | unit tests of the touched crate/package + the parent's tests |
   | interface / contract | the above + every test using a fake/mock of that interface |
   | process, IO, socket, external tool | the above + a real-environment run, stated in the brief |
   | schema / migration | migrate up on a real database + the store contract tests |

   Bug fix: show it red before, green after. Same failure three times → stop and send `blocked`; do not grind.
5. **Commit small.** Each commit builds. Subject `<type>(<task-id>): <slug>` (`feat|fix|refactor|test|docs|chore`), ≤ 70 chars; body says why, plus the verification that ran. No `--no-verify`.
6. **Final brief** (`--kind result`), exact headings, nothing padded:

   ```
   Scope: <task id> — <what shipped, one line>
   Branch: <branch> @ <short sha>
   Files: <path> — <what/why>, one line each
   Verification: <command> → <result>, one line each; pre-fix repro yes/no
   Open questions: <each thing you are not sure about> | none
   Out of scope: <noticed, not touched> | none
   Diff: <+/- lines across N files>
   ```

   Then stop. The result is a handoff, not a self-approval.

## 7. Asking for a decision

When the packet and the code do not settle a question, send `--kind question` in this shape:

```
Question: <one sentence>
Options: <a> | <b> | <c>
Context: <the facts the captain needs, bulleted>
Default: <the option you will take if no answer arrives, or "block">
```

The captain replies by picking an option (`gear reply <id> --resolve -m "<option>: …"`). If the packet gave a response window and it passes, the lane takes its stated safe default or blocks — never guesses silently. Architectural or spec-level questions always go to the captain; lanes do not decide them.

## 8. Review and merge (captain side)

**Writer ≠ reviewer.** Nobody accepts their own work. The reviewer runs on a different model than the writer: the captain reviews when its model differs from the lane's; otherwise the captain routes the review to a seat of another kind.

**Review loop discipline:**
- Read the findings first, count them, then decide. Never announce "clean" before the review output has been read; output present ≠ review finished — confirm the reviewer actually completed.
- Severity: P0/P1/P2 trigger rework; P3 (style, naming, "could be clearer") never does — it is noted in the verdict only.
- A new, concrete finding in a later round is normal convergence (a fix can introduce a defect): keep going.
- The same finding repeated after it was addressed, an architectural/spec-level finding, any P0, or the round cap (`review_max_rounds`, default 3) → stop and escalate to the user; more rounds will not help.
- A small diff in a high-blast-radius area (process lifecycle, persistence, concurrency, an interface everyone implements) is reviewed regardless of size.

**Merge checklist** — all must hold, otherwise the verdict is fail with the missing item named:
1. The final brief exists with every heading.
2. The diff touches only files the packet allowed.
3. Acceptance commands re-run by the captain on the task branch pass; build has zero warnings.
4. Review converged per the rules above (or was explicitly waived by the user).
5. No unanswered question on the task.
6. The branch is up to date with `dev` (fast-forward or clean merge).

**Merge as one operation:** merge into `dev` → verify the merge commit is on `dev` → delete the task branch → mark the task DONE in the plan. For a temp worker, also `worktree remove` after the merge. Half-cleanup is worse than none.

**Conflicts:** when a merge makes another in-flight branch conflict, the captain does not rebase someone else's branch; it tells the owning lane exactly what to run, and that lane resolves it in its own worktree.

**Shared append-only files** (registries, route tables, migration lists, the errors table): lanes only append their own line; never reorder. Conflicts there resolve mechanically by keeping both sides.

## 9. Status comes from facts

A task's state is derived from evidence, not from a report: an attempt exists → in progress; final brief + commits on the branch → in review; merged into `dev` → done; merged into `main` → released. "The agent said done", "the process exited" and "tests passed" are different facts (PRD §3).

Any retry, reminder or fallback timer is idempotent: it first checks whether the work already happened and does nothing if so.

## 10. Milestones

When every task of a milestone is merged into `dev` and `dev` passes the full acceptance suite, the captain writes a milestone brief (what shipped, open risks, what was deferred) and the user decides on `dev` → `staging` → `main`. The captain never merges into `main` on its own.

## 11. Overridable items (`[conventions]`)

```toml
[conventions]
integration = "dev"        # integration branch
release = "main"           # release branch
staging = ""               # pre-release branch; empty = none
review_max_rounds = 3
```

Not overridable: duty declaration, the naming scheme, worktrees through Herdr, writer ≠ reviewer, the merge checklist.
