---
name: gear
description: Work inside a GEAR-managed agent team (captain or lane). Use when GEAR_CTX is set or the user mentions GEAR.
---

# GEAR

GEAR runs one piece of work — a new product, or one requirement inside an existing repo — with a captain and several lanes, each an agent in its own Herdr pane. The rules below are for whoever runs inside it. The text printed by `gear --skill` for the installed binary is authoritative; an installed copy of this file is only a pointer to it. If `gear` reports that the CLI and engine versions differ, stop the related changes and report it.

## 1. Guard

No `GEAR_CTX` in your environment → GEAR did not start you. Run no write command (`gear send`, `gear reply`, `gear plan apply`, `gear admin …`); tell the user.

## 2. Start every turn with `gear`

`gear` prints your home page: who you are (seat, generation), what is waiting for you, and which items you have not seen yet. What you saw is recorded — that is how GEAR knows a doorbell reached you. Run it whenever you are woken, and again before you end a turn.

## 3. Lanes

- Talk to GEAR only through `gear` and `gear send`. Do not prompt other agents or type into other panes.
- Report with `gear send --kind <kind> "<text>"`:
  - `progress` — a step done (one line; not a substitute for committing and pushing);
  - `question` — you need a decision; say Question / Options / Context / Default (conventions §7), then continue with other ready work if there is any;
  - `blocked` — you cannot continue at all: use the exact command in your task packet (it names task, attempt and contract); waiting on another task is `--about T-<x> --need "<what you wait on>"`;
  - `result` — use the exact command in your task packet. **Then end your turn.** The next task is delivered to you as soon as you are idle; do not wait for the review, do not investigate GEAR (its logs, its code, its store) to see why nothing came yet — a busy lane is never given work.
- Answer a captain instruction with `gear send --kind progress --reply-to M-<n> "<text>"` (the `ack` kind is not available in this version).
- A temp reviewer hands in its verdict with `gear send --kind review --task T-<n> --attempt <a> --verdict pass|fail "<findings path or text>"`; it reaches the captain and settles nothing by itself.
- In this version a lane reports `progress`, `question` and `result`; the captain sends `instruction`; a temp reviewer sends `review`. `blocked`, `ack` and `--about` are not available yet and return `BAD_FORMAT`.
- If a command prints `UNCONFIRMED`, run it again with the same `--request-id` it printed; never resend it reworded.
- Do not edit the plan or the task packets GEAR renders — report through `gear send`; do not start background jobs that keep writing after your turn ends. (A project that runs GEAR by hand may let a lane update its packet's status and lane record; that is the project's own convention, not GEAR's.)

## 4. Captain

- `gear` shows the inbox batch: answer with `gear reply M-<n> … -m "<text>"` (`--resolve` default, `--wait <who> --until +2h`, `--note`). Close exactly the ids you handled.
- `gear send --to <seat> --kind instruction "<text>"` gives a lane an instruction.
- Plan, staffing and acceptance go through a plan draft and `gear plan apply`; the captain's guide (kickoff, plan format, daily loop) is `gear --skill captain`.
- You decide; you do not review code yourself. Reviews are done by temp reviewers on a model other than the writer's; what a reviewer must deliver is `gear --skill review`.
- Ask the user only what is theirs: release to main, money, scope, destructive acts on things you did not create.

## 5. Default working conventions

Names, branches, worktrees, the task loop, review and merge: `gear --skill conventions`. Breaking a goal into tasks: `gear --skill decompose`. The captain's guide: `gear --skill captain`. What a reviewer must deliver: `gear --skill review`. Driving a specific agent CLI (start, resume, switch model, quota signs): `gear --skill agent:<kind>`: `agent:claude`, `agent:codex`, `agent:opencode`, `agent:grok`.

The user installs this skill into an agent CLI with `gear admin install-skill --agent <claude|codex|opencode>`; the installed copy is only a pointer — the text `gear --skill` prints for the running binary is the one that counts.

## 6. Errors

| Code | Meaning | Do |
|---|---|---|
| `NOT_REGISTERED` | no valid seat for this process | stop write commands; tell the captain/user |
| `STALE_GENERATION` | your seat was taken over or restarted | stop; run `gear` |
| `FORBIDDEN` | your role or state may not do this | do not retry; ask |
| `BAD_FORMAT` | arguments or text malformed (detail says where) | fix and resend |
| `STALE_VERSION` | the plan changed since your draft's base | export a new draft, reapply |
| `UNCONFIRMED` | outcome unknown | rerun with the same `--request-id` |
| `NO_TARGET` | the task, message or seat named does not exist | check the id |
| `BAD_TRANSITION` | that state change is not allowed from the current state | run `gear`; do not force |
| `REQUEST_CONFLICT` | this `--request-id` was used with different arguments | resend the original exactly, or use a new id for new content |
| `QUOTA` | no capacity for that (lanes/temps) | wait; the captain decides |
| `ENGINE_DOWN` | the engine is not running | your write is stored; tell the user if it persists |
| `LOCKED` | the store is busy | retry shortly |
| `SCHEMA_TOO_NEW` | the store is newer than this binary | do not retry; tell the user to upgrade GEAR (the store is never migrated down) |

A fuller `errors` topic ships with slice 2.
