# Adopting a project that already runs (a first frame)

`gear --skill adopt` — for the captain, **before the kickoff** of a project
that has been running without GEAR. GEAR lands in someone's working system: the
adoption is how it fits instead of fighting it. This is a frame to grow from
feedback, not a procedure to follow to the letter.

Write the analysis to `gear/adopt.md` and the standard to `gear/standard.md`
(both templates ship next to this file, in the skill's `templates/`
directory: `adopt.md` and `standard.md`). The analysis feeds the departure sheet:
what the project already has decides repos, integration branches, the gate, the
lanes and the model chain — not GEAR's defaults.

## 1. The instruction layer

Every CLI auto-loads something: `CLAUDE.md`, `AGENTS.md`, `.claude/skills/`,
`.agents/`, `opencode.json`, and whatever those point to. Two of them usually
contradict each other, and both contradict GEAR's loop.

- Find every file a CLI in use will read (ask the user which CLIs their team
  runs; `gear admin kickoff` lists what this machine has).
- Move that layer to an archive path **no CLI reads** (e.g. `archive/agent-notes/`),
  and leave one short project file: what the project is, what "done" means, and
  one line per CLI that says where the archive is.
- Nothing is deleted: the archive is where the team's reasons still live.
- The standard (step 2) is what the agents read from now on — one file, not
  five.

## 2. Exemplars and no-go patterns

Read the code the team is proud of and the code they complain about. Name a few
of each, by path, in `gear/standard.md`:

- **Exemplars**: two or three files a new change should look like.
- **No-go zones**: patterns that must not be copied, one line each, with the
  file that shows why (a bad pattern next to a good one is the clearest).
- The line that makes it usable: *new code meets the standard even if its
  neighbours do not.*

## 3. The gate

The gate is the project's own build and test, run before every merge. Measure
it before trusting it:

- **Time**: a gate of minutes makes lanes unusable; if it is slow, start with
  the fastest honest subset and say so.
- **Red**: run it on the untouched mainline. If it is already red, that is the
  first work of the adoption voyage — a red gate cannot judge a merge.
- **Flaky**: run it a few times. A test that fails at random is a broken gate,
  not a strict one.

If there is no gate, build a trusted one first (one command, offline, seconds).
`make test` or `scripts/check.sh` is enough; the captain writes the command into
`[conventions].gate`.

## 4. Keep, replace or drop

Decide, one line each, and put the decision in the departure sheet:

| Thing | Ask |
|---|---|
| branches | Which branch is the integration one, which is release? GEAR merges into the first, never into the second. |
| CI | Keep it; the gate runs the same thing locally. Do not let two gates disagree. |
| release | Who releases, and how? That stays the user's decision (GEAR stops at the landing). |
| tracker | Issues in GitHub, Jira, or the plan only? A tracker GEAR does not drive is a place to *read* from, not write to. |
| docs | Which docs are current? Move the rest to `archive/`, like the instruction layer. |

## The first voyage is the adoption voyage

Its done-sentence is small and checkable: the instruction layer is archived and
one project file stands, `gear/standard.md` exists and the task packets carry
it, the gate is green and timed, and the working agreement is in the config.
Only after that does the project's own work start — and the captain says so at
the landing.

## What this frame does not do

- It does not rewrite the project's history, branches or CI.
- It does not judge the team's past decisions; it records what they are.
- It does not do the migration alone: steps 1 and 4 touch the user's own files,
  so the captain proposes and the user confirms.
