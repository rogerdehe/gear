# GitHub

`gear --skill github` — when a project's issues and pull requests live on
GitHub, and what GEAR does there. GEAR's plan and store stay the one source of
truth; GitHub is the door the team already reads.

## When to choose it

- The team works in GitHub: a request lands as an issue, a change is read as a
  pull request. GEAR meets them there instead of asking them to move.
- Not for a project whose work lives in the plan alone: without `[github]`,
  GEAR never calls a forge. That is the default and it costs nothing.

## The config

At the project root, and on its own per `[[repos]]` entry (a child never
inherits the root's section: it names another repository):

```toml
[github]
repo = "owner/name"
merge = "push"                 # or "pr": land a task through its pull request
merge_method = "squash"        # or "merge"

[github.issues]
labels = ["gear"]              # which issues become tasks; empty = every open one
```

## What the engine does, and when

The engine calls `gear_forge::sync` once per tick, reading what the store
already knows — no agent writes on GitHub:

| In GEAR | On GitHub |
|---|---|
| a task's first delivery | a **draft claim PR** for `<lane>/t<n>`, titled from the goal's first words, body `Closes #n` and the `ORCHESTRATE:CLAIM` marker |
| a hand-in | a comment with the `ORCHESTRATE:COMPLETION` marker |
| a verdict | a comment: the verdict and the findings |
| a merge, `merge = "push"` | a comment naming the merge commit |
| a merge, `merge = "pr"` | the pull request is merged, with `merge_method` |
| either merge | the issue is closed |

Every call is one op: once per event, at most three tries across ticks, and an
op the engine found unfinished is never repeated — a restart cannot post a
comment twice. A GitHub outage queues the calls and never stops local work.

## How an issue becomes a task

1. **`gear plan import-issues`** — one `## NEW` block per open issue the plan
   does not carry yet. `--label` narrows it (the config's labels apply too);
   give it the voyage file to check against the file you are editing.
2. **Paste and edit** the block in the voyage file: `lane`, `milestone` and
   `estimate` are the captain's — an issue says what is wanted, not who does it.
3. **`gear plan apply gear/voyages/<M>.md`** — the block's `issue: #n` binds the
   task to its issue, and the merge closes it.

## Agents never write with `gh`

Only the engine writes. A lane that needs something said on GitHub reports it
and the captain decides. This cannot be enforced by a hook — `gh` has no
pre-push — so it is a rule here and the captain's eye; the engine being the only
writer is what keeps the record true.
