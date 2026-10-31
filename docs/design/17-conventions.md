# 17 Default working conventions

GEAR ships one default way of working. `gear admin init` applies it; a project may override the items listed in §11 through `[conventions]`, but if it changes nothing, this is how its captain and lanes work. The same text ships in the version-matched skill (16), so the captain and every lane read identical rules via `gear --skill conventions`.

Purpose: when several agents build one product over weeks, every name, branch, worktree and report must still be traceable by a human and by the captain, and nobody — human or agent — may grade their own work.

Lineage: §5–§9 absorb the collaboration rules of the user's BMAD-Team skills (claim, dev, review, merge, review-loop discipline, fallback idempotency). BMAD-Team uses GitHub PRs and HTML markers as its message bus because it has no shared daemon; GEAR has one (the SQLite store + engine), so the *rules* carry over while the *transport* becomes `gear send` / `gear reply` / the plan.

## 0. What a GEAR project is, and the kickoff

**Scope.** A GEAR project is one *piece of work* with a goal, not necessarily a new repository. Two shapes are supported equally:
- **Greenfield:** a new product; `integration`/`release` are the repo's own `dev`/`main`.
- **Iteration in an existing repo:** one requirement or feature inside a large codebase. The project gets a short name from the requirement (`billing-v2`, `sso-login`) — that name prefixes every agent, workspace and branch (§2), so several iterations can run in one repo and one Herdr server side by side. `integration` is a feature integration branch cut from the repo's mainline (e.g. `feat/billing-v2`), lanes' `owns` cover only the paths the requirement touches, and `release` is the repo's mainline; delivering into it follows the repo's own process (usually a PR), which the user decides.

**Kickoff — align with the user before any lane starts.** The captain (later `gear admin init`) proposes, the user confirms:
1. **Inventory** what is actually installed: agent CLIs Herdr can start (`herdr agent` kinds) and, per CLI, the models it can reach (e.g. `opencode models`), including plan/quota limits the user knows about. Never plan around a model that is not there.
2. **Work and names:** the goal, greenfield or iteration, the project name, the branches (above).
3. **Lanes:** duties and `owns` (§1), how many, and which agent kind each runs.
4. **Model plan** — per role an ordered chain, primary first, then fallbacks (`[models]`, design/01):
   - e.g. lanes `MiniMax M3.1 Flash → DeepSeek V4.1 Flash`, reviewers `DeepSeek V4.1 Flash → MiniMax M3.1 Flash → DeepSeek V4 Pro`.
   - **On a provider limit, one of two policies, chosen per role at kickoff** (`on_quota`):
     - `fallback` — switch the agent to the next model in its chain and continue; switch back when the provider resets.
     - `wait` — park the agent until the provider's reset (`quota_reset_h`, e.g. 5 h for a 5-hour plan window, or the reset time the error message states), then wake it on the same model to continue its inbox. Cheaper or more consistent output, slower calendar time.
     Either way the stop is detected from the screen (quota markers, SOP), never mistaken for a finished turn, and every switch/park is logged.
   - **Reviewer independence wins over order:** a reviewer takes the first model in its chain that differs from the writer's *current* model; if none differs, the review waits and the captain tells the user.
   - Models are named by their full provider/model id (or full picker label), never a display prefix (lesson L-29).
5. **First milestone** and how the user wants to be involved (release gate, questions).

The agreed answers are written to the project config and the plan; changing them later is a plan change the user confirms.

## 1. Duty first, then a lane

- Every long-lived lane declares in project config: `duty` (one sentence), `owns` (directories/modules it is responsible for), `not` (explicitly out of scope, may be empty).
- A lane whose duty cannot be stated is not opened — the split is not thought through yet. `init` rejects lanes without `duty`/`owns`, and lanes whose `owns` overlap.
- A change of duty is a plan change (05), not a rename: retire the old lane, open a new one.
- Temp workers have no duty of their own; their task is their scope.

## 2. Names

| Object | Rule | Example |
|---|---|---|
| Lane name (`name` in config) | one short word from its duty, `[a-z][a-z0-9-]*` | `data`, `runtime`, `heat` |
| Agent name / Herdr workspace label | `<project>-<lane>`, ≤ 32 chars (Herdr agent names: `[a-z][a-z0-9_-]{0,31}`) | `gear-data`, `harbor-heat` |
| Temp seat (long-lived workspace) | `<project>-<kind>-<k>`: the kind of temp work plus the seat number; kinds are not listed in advance — `review` is the common one, others (e.g. an investigation) are named the same way when needed | `gear-review-1` |
| Temp agent (one job in a seat) | `<project>-<kind>-t<n>`: kind plus the task it works on | `gear-review-t12` |
| Captain | `<project>-captain` | `harbor-captain` |
| Task id | `T-<n>` in text, `t<n>` in branch names | `T-12`, `t12` |

The same word appears in the Herdr sidebar, the agent name, the branch and the GEAR home view. The project prefix keeps several projects on one Herdr server collision-free. Names are derived, never invented ad hoc (`lane-a`, `agent-3`, `bot` are not allowed).

Herdr clears an agent's name when it re-detects the occupant (a Herdr server restart, a CLI that changes its session), and `agent.prompt` refuses an agent with no name (T-134). So the name is not a one-time `agent.start` argument: every engine tick gives each seat with an active binding its name back, and every prompt, doorbell or key send to a seat's pane makes sure of it first. A name another pane holds is reported to the captain once and never stolen.

## 3. Branches

| Branch | Role | Who merges into it |
|---|---|---|
| `main` | released / production state | `dev` (or `staging`) after milestone acceptance, confirmed by the user |
| `staging` | pre-release environment; **optional**, only if the project has one | `dev` |
| `dev` | daily integration; always builds and passes tests | captain, after reviewing a task branch |
| `<lane>/<task-id>-<slug>` | task branch, cut from `dev`, merged back, then deleted | the lane commits |
| `home/<lane>` | anchor branch of a lane's long-lived worktree; never committed to | — |

- `home/<lane>` exists because git cannot hold both a branch `data` and a branch `data/t1-…`; it only names the worktree and parks it between tasks.
- **Task branches are pushed.** The lane cuts its task branch from the latest `dev` when it starts the task (the packet names it), pushes it (`git push -u origin <branch>`) and keeps pushing as it commits. Review reads the pushed branch. After the merge into `dev` the captain deletes the branch on the remote and locally (on GitHub, also enable "automatically delete head branches"). The remote therefore only holds `main`, `dev` (`staging`) and branches currently in development or review.
- Lanes may create, switch and push **their own task branches** only. They never merge, rebase shared branches, push `dev`/`main`/`staging`, force-push, or delete branches. Merging and cleanup are the captain's job (§8).
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

## 5. Claiming work and not waiting for review

- Work is visible before it starts: a lane only works on tasks queued for it on the board (an attempt exists once it starts). Lanes do not invent work.
- **A lane never idles waiting for review.** After handing a task in (§6.6) it takes the next `ready` task in its own queue at once, without waiting for the captain.
- In-flight cap per lane: **one task in development + at most two in review.** At the cap the lane stops taking new work and reports it — review is falling behind.
- Rework comes first, at a safe point: when a task in review comes back with follow-ups, the lane finishes the checklist item it is on, commits and pushes it, switches to the reworked task's branch, does the follow-ups, pushes, hands it in again, and switches back. It does not switch with uncommitted changes and does not stash. If the captain marks the rework `after-current`, the lane finishes its current task first.
- A lane that drops a task (blocked, out of scope, lost context) says so with `gear send --kind blocked`; the captain reassigns. No silent abandonment, no zombie branches.

## 6. The task loop (lane side)

0. **Start.** Cut the task branch named in the packet from the latest `dev` (`git fetch origin && git switch -c <branch> origin/dev`), set the packet's `Status: running`, commit, push.
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
5. **Commit small, check the inbox.** Each commit builds and is pushed. After every finished checklist item, look at your inbox (`gear`) for rework, answers and the next task. Subject `<type>(<task-id>): <slug>` (`feat|fix|refactor|test|docs|chore`), ≤ 70 chars; body says why, plus the verification that ran. No `--no-verify`.
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

   Set `Status: review`, push, then go straight to your next `ready` task (§5). The result is a handoff, not a self-approval.

## 6a. Decomposing work

GEAR does not prescribe a decomposition method (BMAD, superpowers or anything else may produce the raw material); it prescribes what the resulting tasks must satisfy. Full rules and the self-check: `gear --skill decompose`.

1. **Every task merges into `dev` on its own** and leaves `dev` building and green. New code may be dormant (not yet called); a half-broken state may not land.
2. **A lane's queue never forces waiting.** The first few tasks queued for a lane are mutually independent and each can start from current `dev`, so the lane moves on while earlier tasks are in review.
3. **Cross-lane dependencies are cut by contract first.** The captain fixes interfaces, signatures and contract tests and merges them into `dev` before the work that implements and the work that uses them; consumers develop against fakes.
4. **Changes that must land together** are one task, or are sequenced expand → migrate → contract (add the new path, switch callers, then remove the old one in a separate task).
5. **Unavoidable dependencies are declared** (`depends on`). A task is `ready` only when everything it depends on is merged into `dev`; until then the lane works its other tasks.
6. **Re-plan continuously.** A lane with no startable task is a planning failure and a review trigger (11: `idle_lanes`, `low_ready`): pull independent work forward, split a task, or add a contract task.

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

Review is asynchronous: the lane has already moved on. The captain fetches the pushed branch and reviews it; a pass merges it, a fail writes follow-ups into the packet (committed and pushed on that task branch) and marks it for rework.

**Writer ≠ reviewer, and the captain does not review.** Each review is done by a **temp reviewer**: a fresh agent session started for that one review, on a different model from the writer, given only the packet, its contracts and the diff. It writes its review file — the project chooses the path — and hands the findings in with `gear send --kind review …` (verified acceptance facts, spec compliance, findings P0–P3, set-aside items, questions, verdict) and ends. GEAR keeps the findings text, so the writer reads them from GEAR and the reviewer's worktree may be gone. The captain only decides — pass/merge or fail/rework — from that review, looking at code only where the review raises a question; a 10+ minute review per task would otherwise block the captain all day. A re-review of the same task after rework resumes the same reviewer session (it knows its own findings) and checks the delta. GEAR prescribes these requirements, not the method: a reviewer may follow BMAD `code-review`, superpowers `requesting-code-review` or its own approach.

**Review loop discipline:**
- Read the findings first, count them, then decide. Never announce "clean" before the review output has been read; output present ≠ review finished — confirm the reviewer actually completed.
- Severity: P0/P1/P2 trigger rework; P3 (style, naming, "could be clearer") never does — it is noted in the verdict only.
- A new, concrete finding in a later round is normal convergence (a fix can introduce a defect): keep going.
- The same finding repeated after it was addressed, an architectural/spec-level finding, any P0, or the round cap (`review_max_rounds`, default 3) → stop: the captain decides the next step itself (rewrite the contract, split, cut or take the work another way); more rounds will not help.
- A small diff in a high-blast-radius area (process lifecycle, persistence, concurrency, an interface everyone implements) is reviewed regardless of size.

**Merge checklist** — all must hold, otherwise the verdict is fail with the missing item named:
1. The final brief exists with every heading.
2. The diff touches only files the packet allowed.
3. Acceptance commands re-run by the captain on the task branch pass; build has zero warnings.
4. Review converged per the rules above (or was explicitly waived by the user).
5. No unanswered question on the task.
6. The branch merges cleanly into current `dev`. If not, the captain does not resolve it: it sends the task back for the lane to merge `dev` into its branch and resolve, then reviews the delta.

**Merge as one operation:** `git fetch` → on `dev`: `git merge --no-ff origin/<branch>` → re-run acceptance on `dev` → push `dev` → delete the branch on the remote and locally → mark the task DONE → mark tasks whose dependencies are now all merged as `ready`. For a temp worker, also `worktree remove` after the merge. Half-cleanup is worse than none.

**Conflicts:** the captain never rebases or resolves someone else's branch; the owning lane merges `dev` into its branch and resolves in its own worktree.

**Shared append-only files** (registries, route tables, migration lists, the errors table): lanes only append their own line; never reorder. Conflicts there resolve mechanically by keeping both sides.

## 8a. Long-lived lane or temp worker

- A **long-lived lane** is for work where accumulated context is an asset: a duty and owned paths it gets better at over time. It pulls its own queue asynchronously.
- A **temp worker** is for work that must start clean every time and ends with one deliverable — reviews, one-off investigations, isolated fixes. The captain/engine pushes the job to a fresh session; nobody has to remember to clear anything. Same job coming back (rework, follow-up) resumes the same session.
- **Clean context comes from a new session, never from `/clear`.** Measured 2026-10-22 (OpenCode on Herdr, this Rust repo): a fresh temp agent costs ~20 s (worktree 0.8 s, shell init ~12 s, `agent start --auto` 4.4 s, model check ~3 s) plus a cold build (~1 min here); tokens are the same either way, because a cleared session re-reads everything too. When cold setup is expensive, a **warm seat** is allowed: keep the worktree and its build cache, and before each job the captain/engine exits the agent and starts a new session (`agent start` without `-s`, seconds). It is still push-dispatched temp work; a re-review resumes that job's session id.
- There is no such thing as "a long-lived lane that must be cleared before each task": an agent cannot clear its own context, and an asynchronous lane is never cleared by anyone. If a role needs a clean context per task, it is temp work.

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

Not overridable: kickoff alignment (§0), duty declaration, the naming scheme, worktrees through Herdr, writer ≠ reviewer, the merge checklist.
