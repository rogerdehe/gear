# Adoption analysis — <project>

<!--
Template for `gear/adopt.md`. The captain fills it before the kickoff of a
project that already runs, and turns it into the departure sheet. One line per
answer where a line is enough; delete the instructions.
-->

## The instruction layer

What the CLIs in use load today (files and what they point to):

| Path | Read by | Says |
|---|---|---|
| `CLAUDE.md` | Claude Code | |
| `AGENTS.md` | Codex, others | |
| `.claude/skills/` | Claude Code | |
| | | |

Where it was archived: `archive/agent-notes/` (or the path the user chose).
The one project file that replaces it: `AGENTS.md` (short: what the project is,
what done means, where the archive is).

## Exemplars and no-go patterns

Exemplars (new code should look like these):

- `src/good.py` —
- `src/…` —

No-go patterns (do not copy these):

- `src/bad.py` — mutable default argument, global state, bare `except`
- `…` —
- `…` —

The standard they are written into: `gear/standard.md`.

## The gate

- Command: `sh gate.sh` (or `make check`, or the project's own)
- Time on the mainline: __ s (a lane needs seconds, not minutes)
- Runs on the untouched mainline: green / red (if red: what fails)
- Repeated runs: stable / flaky (which test)

If there was no trusted gate, what was built first:

- `scripts/check.sh` — one command, offline, seconds

## Keep, replace, drop

| Thing | Today | Decision |
|---|---|---|
| Integration branch | | keep `main` / `dev` / … |
| Release branch | | keep; GEAR stops at the landing |
| CI | | keep (`ci.yml` runs the gate) / drop / replace |
| Release ritual | | keep as the user's / replace |
| Tracker | | read from / leave alone / GEAR drives it |
| Docs | | which are current; the rest to `archive/` |
| Branch rules | rebase, force-push, … | replaced by GEAR's one-branch-per-task loop |

## The adoption voyage

Done-sentence (small and checkable):

- the instruction layer is archived and one project file stands;
- `gear/standard.md` exists, and every task packet carries it;
- the gate is green and timed;
- the working agreement is in `gear/gear.toml` and confirmed in the sheet.

Open questions for the user:

- …
