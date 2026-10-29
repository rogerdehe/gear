# Working conventions (`gear --skill conventions`)

The one default way of working every GEAR project shares. The captain and every lane read the same text.

## 1. A project, a voyage, a lane

- A **project** is one piece of work with a goal: a new product, or one requirement inside an existing repo (then the project gets a short name, e.g. `billing-v2`, and its own integration branch cut from the mainline). Its code may be one git repository or a folder of several (§9).
- A **voyage** is one milestone: a `done:` sentence, a `bar:`, an `eta:` style, and its own folder `gear/voyages/<milestone>/` holding its plan (`plan.md`), its `reports/` and its `reviews/`. Its id is any word without spaces (`M1`, `sprint-3`, `2026-q4`); this guide writes `M<n>`. The folder is a convention too (`voyage_dir`, §3).
- A **lane** is a long-lived agent with a declared **duty** (one sentence) and **owned paths**; lanes never own the same path. A lane whose duty cannot be stated is not opened. A changed duty is a new lane, not a rename.
- A **temp worker** (today: a reviewer) starts clean for one job and ends with one deliverable. Clean context always comes from a new session, never from clearing an old one.

## 2. Names

| Object | Rule | Example |
|---|---|---|
| Lane | one short word from its duty | `engine`, `data` |
| Agent / workspace | `<project>-<lane>` (≤ 32 chars) | `acme-engine` |
| Reviewer seat (workspace) | `<project>-review-<k>` | `acme-review-1` |
| Reviewer agent (one job) | `<project>-review-t<n>` | `acme-review-t12` |
| Captain | `<project>-captain` | `acme-captain` |
| Task | `T-<n>` in text, `t<n>` in branches | `T-12`, `engine/t12` |

Names are derived, never invented (`lane-a`, `agent-3`, `bot` are not allowed).

## 3. Branches

| Branch | Role |
|---|---|
| `main` (release) | released state; reaches it only through the user's decision at a voyage's end |
| `dev` (integration) | always builds and passes the gate; the engine merges passed tasks into it |
| `<lane>/t<n>` | a task branch, cut from the latest integration branch of the lane's repo, pushed as you commit, deleted after the merge |
| `home/<lane>` | the anchor of a lane's long-lived worktree; never committed to |

Lanes create, switch and push their own task branches only. They never merge, rebase shared branches, push `dev`/`main`, force-push or delete branches. They never run `git stash`: every worktree of the repository shares one stash, so a stash can carry their work to another lane or theirs to them — work set aside is committed on the task branch and pushed.

The branch names, the remote and the voyage folder are configuration: a project names its own in `[conventions]`, as templates with the placeholders filled in. These are the defaults (the names GEAR itself uses):

| `[conventions]` | Default | Filled in |
|---|---|---|
| `integration` / `release` / `staging` | `dev` / `main` / none | — |
| `task_branch` | `{lane}/t{n}` | `{project}`, `{lane}`, `{n}` |
| `review_branch` | `review/t{n}` | `{project}`, `{lane}`, `{n}` |
| `seat_branch` | `{project}/{seat}` | `{project}`, `{seat}` |
| `remote` | `origin` | — (the git remote a hand-in is checked against) |
| `voyage_dir` | `gear/voyages/{milestone}` | `{project}`, `{milestone}`; its `plan.md`, `reports/` and `reviews/` |
| `merge_message` | `merge({task}): {source}` | `{project}`, `{task}`, `{source}` |
| `review_message` | `review({task}): {what}` | `{project}`, `{task}`, `{attempt}`, `{what}` |
| `plan_message` | `{milestone}: {what}` | `{project}`, `{milestone}`, `{what}` |

## 4. Worktrees — always through Herdr

Herdr creates and removes every worktree; its config decides the path. A lane keeps one worktree for life (its build cache persists); a reviewer seat keeps one between jobs. One agent, one worktree. Nobody edits in the main checkout. Removing a worktree, closing a session and deleting a branch are separate acts; nothing uncommitted or unmerged is ever thrown away.

A lane's worktree holds only its repo (§9).

## 5. The lane's task loop

0. **Start.** The delivery names the task, its branch and its packet: run `gear` first, then in the lane's repo (§9) `git fetch origin && git switch -c <branch> origin/<integration>` (or switch back to it for a rework). The delivery names `origin`; a project whose remote is called otherwise (`[conventions].remote`) fetches from that name.
1. **Orient.** Read the packet completely (goal, acceptance, the voyage's `done:`/`bar:`), the files it names, the recent history of the area.
   **You implement; you never design.** When the packet is unclear, contradicts the design or a message, or you catch yourself weighing two readings, stop at once and ask (§6) with your default, then go on with the default. A design question is the captain's work, not yours.
2. **Reproduce before fixing** a bug. If you cannot observe it, your model is wrong: ask.
3. **Implement the minimum** the goal needs. No side refactors; what you notice goes into the brief under "Out of scope".
4. **Verify at the depth the change deserves:**

   | Change | Minimum |
   |---|---|
   | docs, pure refactor | build clean + tests of the touched unit |
   | logic | the unit's tests + its callers' tests |
   | interface | the above + every test using a fake of it |
   | process, IO, an external tool or agent | the above + one run in the real environment, stated in the brief |
   | schema / migration | migrate a real database + the store's tests |

   A bug fix is shown red before, green after. The same failure three times → `gear send --kind question`; do not grind.
5. **One branch per task, every round.** Commit small and push to the task's branch at least every 15 minutes; each commit builds. A rework continues on the same branch — never a new one. Before handing in, merge (never rebase) the latest integration branch into it, build, and push: the engine merges only what is on `origin/<lane>/t<n>`.
6. **Hand in** with the exact command in your packet; the result text is the final brief:

   ```
   Scope: <task> — <what shipped, one line>
   Files: <path> — <what/why>, one line each
   Verification: <command> → <result>, one line each
   Open questions: <…> | none
   Out of scope: <noticed, not touched> | none
   ```

   Add `--friction "<what cost time the captain could fix>"` to a result or review when applicable; it is required when the round exceeds its budget. Include a missing permission, slow build or test, unclear contract, missing tool or fixture, or costly step. **Then end your turn.** The next task is delivered as soon as you are idle.
7. **Rework comes first.** A sent-back task is delivered before new work; do exactly the rework note.

When GEAR asks at the estimate, answer with `gear send --kind progress --task T-<n> --attempt <a> --contract <c> --left <minutes> --blocker "<what stops you, or none>" "<done; left>"`, then continue. To wait for another task's merge, send `gear send --kind blocked --task T-<n> --attempt <a> --contract <c> --on T-<m> "<done; what waits>"`; GEAR pauses the clock and resumes this session when it lands.

## 6. Asking for a decision

```
Question: <one sentence>
Options: <a> | <b> | <c>
Context: <the facts, bulleted>
Default: <the option you take if no answer comes, or "block">
```

Send it with `gear send --kind question`, then continue with other ready work if there is any. Architecture and scope questions always go to the captain.

## 7. Review and merge (what the engine does)

- A hand-in starts a temp reviewer on a different model from the writer's, on its own branch `review/t<n>`. What it delivers: `gear --skill review`.
- A **pass** is settled and merged by the engine: it merges the candidate into the integration branch of the task's repo, runs that repo's gate (`[[repos]].gate`, falling back to `[conventions].gate`), pushes, and deletes the task and review branches. A merge that fails (conflict, red gate) goes to the captain once.
- A **fail** goes to the captain, who judges it against the bar and writes the rework (`gear --skill course`).
- Status comes from facts: an attempt exists → in progress; handed in → in review; merged → done. "The agent said done" is not a fact.

## 8. Overridable (`[conventions]`)

```toml
[conventions]
integration = "dev"     # the integration branch
release = "main"        # the release branch
gate = "make test"      # the project's own build + test, run before every merge; no default
max_task_min = 60       # no task is planned longer than this; a plan that asks for more is refused
```

`max_task_min` (default 60) caps a task's `estimate:`: a lane works unattended and is reviewed only at its hand-in, so a task of hours is a stretch nobody checks and one wrong turn costs all of it — bigger work is cut into tasks of at most `max_task_min`, each with its own test cycle (`gear --skill decompose` §1 rule 5). A rework estimate (`estimate=` on a fail verdict) is not capped.

`[conventions] task_branch` (default `{lane}/t{n}`) and `review_branch` (default `review/t{n}`) name the branches; `{project}`, `{lane}`, `{n}` are filled in, e.g. `task_branch = "{project}/{lane}/t{n}"`. `seat_branch` (default `{project}/{seat}`) names the branch of each seat's own worktree, and of the merge worktree as seat `merge`, so Herdr names the worktree `<project>-<seat>`.

The gate has no default: GEAR cannot know another project's build and test, so a project that names none is refused at `gear admin init` until it does (`make gate`, `go test ./...`, `npm test` — whatever exits non-zero on any failure and runs in seconds).

A repo may also carry a `[github]` mode (issues in, pull requests out); what the engine writes there and when: `gear --skill github`.

Everything else here is not overridable.

## 9. A project of several repos

A project may be one git repository or a folder of several. The config names the repos; a lane names the repo it works in.

```toml
[[repos]]
name = "api"                 # [a-z][a-z0-9-]*, unique
path = "services/api"        # relative to project.repo
integration = "dev"          # optional; defaults to [conventions]
release = "main"             # optional
gate = "make test"           # optional; else [conventions].gate, which then must be named

[[lanes]]
name = "api"
duty = "The API service"
owns = ["src/"]              # relative to the lane's repo
repo = "api"                 # the [[repos]] entry; without it the project root
```

A lane without a `repo` works in the project root, exactly as before.

A lane may also run its own model and CLI instead of `[models] lanes`: a `models` list in its `[[lanes]]` block replaces the lane chain for that lane alone, its first entry being the model the engine starts and restarts it on (and the one the quota policy walks). E.g. two lanes on one model, one on another, one on a CLI of its own:

```toml
[models]
lanes = ["provider/fast-model"]

[[lanes]]
name = "api"
duty = "The API service"
owns = ["src/"]
repo = "api"
models = [{ model = "<model-id>", cli = "pi" }]   # pi | codex | opencode | claude

[[lanes]]
name = "web"
duty = "The web client"
owns = ["app/"]
models = [{ model = "<other-model-id>", cli = "codex" }]
```

- A lane's worktree holds only its repo: it is cut from that repo's integration branch, reviewed against it and merged into it with that repo's gate.
- `owns` paths are relative to the lane's repo, so two lanes may own the same path in two repos.
- A repo may have several lanes, each owning its own paths. A task changes one repo; a requirement that crosses repos is cut into a contract task first, then one task per repo (`gear --skill decompose`).
