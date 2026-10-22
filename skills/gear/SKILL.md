---
name: gear
description: Work inside a GEAR-managed agent team (captain or lane). Use when GEAR_CTX is set or the user mentions GEAR.
---

# GEAR

GEAR runs one piece of work — a new product, or one requirement inside an existing repo — with a captain and several lanes, each an agent in its own Herdr pane. The rules below are for whoever runs inside it. The text printed by `gear --skill` for the installed binary is authoritative; an installed copy of this file is only a pointer to it.

## 1. Guard

No `GEAR_CTX` in your environment → GEAR did not start you. Do not run `gear send` or `gear reply`; tell the user.

## 2. Start every turn with `gear`

`gear` prints your home page: who you are (seat, generation), what is waiting for you, and which items you have not seen yet. What you saw is recorded — that is how GEAR knows a doorbell reached you. Run it whenever you are woken, and again before you end a turn.

## 3. Lanes

- Talk to GEAR only through `gear` and `gear send`. Do not prompt other agents or type into other panes.
- Report with `gear send --kind <kind> "<text>"`:
  - `progress` — a step done (one line; not a substitute for committing and pushing);
  - `question` — you need a decision; say Question / Options / Context / Default (conventions §7), then continue with other ready work if there is any;
  - `blocked` — you cannot continue at all;
  - `result` — use the exact command in your task packet (it names task, attempt and contract).
- Answer a captain instruction with `gear send --kind ack --reply-to M-<n> "<text>"`.
- If a command prints `UNCONFIRMED`, run it again with the same `--request-id` it printed; never resend it reworded.
- Do not edit plan or task files; do not start background jobs that keep writing after your turn ends.

## 4. Captain

- `gear` shows the inbox batch: answer with `gear reply M-<n> … -m "<text>"` (`--resolve` default, `--wait <who> --until +2h`, `--note`). Close exactly the ids you handled.
- `gear send --to <seat> --kind instruction "<text>"` gives a lane an instruction.
- Plan, staffing and acceptance go through a plan draft and `gear plan apply` (its format topic `plan-format` ships with slice 2).
- You decide; you do not review code yourself. Reviews are done by temp reviewers on a model other than the writer's (conventions §8).
- Ask the user only what is theirs: release to main, money, scope, destructive acts on things you did not create.

## 5. Default working conventions

Names, branches, worktrees, the task loop, review and merge: `gear --skill conventions`. Breaking a goal into tasks: `gear --skill decompose`. Driving a specific agent CLI (start, resume, switch model, quota signs): `gear --skill agent:<kind>`, e.g. `agent:opencode`.

## 6. Errors

| Code | Meaning | Do |
|---|---|---|
| `NOT_REGISTERED` | no valid seat for this process | stop write commands; tell the captain/user |
| `STALE_GENERATION` | your seat was taken over or restarted | stop; run `gear` |
| `FORBIDDEN` | your role or state may not do this | do not retry; ask |
| `BAD_FORMAT` | arguments or text malformed (detail says where) | fix and resend |
| `STALE_VERSION` | the plan changed since your draft's base | export a new draft, reapply |
| `UNCONFIRMED` | outcome unknown | rerun with the same `--request-id` |
| `LOCKED` | the store is busy | retry shortly |
