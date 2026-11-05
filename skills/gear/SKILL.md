---
name: gear
description: Work inside a GEAR-managed agent team (captain, lane or reviewer). Use when GEAR_CTX is set, the user mentions GEAR, or the user asks you to take over a project's captain.
---

# GEAR

GEAR runs one piece of work — a new product, or one requirement inside an existing repo — with a captain, several lanes and temp reviewers, each an agent in its own Herdr pane. The text `gear --skill` prints for the installed binary is authoritative; an installed copy of this file only points to it.

## 1. Guard

Run `gear` once when you start. If it names no seat of yours, GEAR did not start you: run no write command (`gear send`, `gear reply`, `gear plan apply`, `gear admin …`); tell the user. If it names your seat, that seat is yours.

## 2. Start every turn with `gear`

Run `gear` commands exactly as GEAR's messages to you write them. A CLI that runs its commands outside its own pane (OpenCode, Codex) gets them as `gear --session <session> --pane <your pane> …`, since only those two arguments tell GEAR who it is; keep them on that seat's `gear` commands and never use another seat's pane. Every other CLI runs plain `gear`. Nothing else you run (git, grep, builds, tests) takes these arguments or a `gear` call in front of it.

`gear` prints your home page: who you are, your task, what is waiting for you. Running it records that you saw it — that is how GEAR knows a message reached you. Run it whenever you are woken.

## 3. Lanes

- Talk to GEAR only through `gear` and `gear send`. Never prompt other agents or type into other panes.
- Report with `gear send --kind <kind> "<text>"`:
  - `progress` — a step done (one line; not a substitute for committing and pushing);
  - `question` — you need a decision (`gear --skill conventions` §6), then continue with other ready work if there is any;
  - `result` — the exact command in your task packet. **Then end your turn.** Your next task is delivered as soon as you are idle; never wait for the review, never investigate GEAR to see why nothing came yet.
- Answer a captain instruction with `gear send --kind progress --reply-to M-<n> "<text>"`.
- Answer an estimate question with `gear send --kind progress --task T-<n> --attempt <a> --contract <c> --left <minutes> --blocker "<what stops you, or none>" "<done; left>"`, then keep working. When another task must merge first, send `gear send --kind blocked --task T-<n> --attempt <a> --contract <c> --on T-<m> "<done; what waits>"`; the clock pauses until it merges.
- For a result or review, add `--friction "<what cost time the captain could fix>"` when a permission, slow build or test, unclear contract, missing tool or fixture, or other step cost time. Say so too when the checks the captain asked for took a fifth of the round or more: name them and their minutes. The flag is required when the round exceeds its budget.
- If a command prints `UNCONFIRMED`, run it again with the same `--request-id`; never resend it reworded.
- When your agent is switched to another CLI, the packet starts with a handover: read the previous session it names, check the worktree it describes, and do not start over on work you have not inspected. The template ships as `templates/handover.md`.
- Do not edit the plan or the packets GEAR renders; do not start background jobs that keep writing after your turn ends.

## 4. Reviewers

A temp reviewer judges one task and hands in with `gear send --kind review --task T-<n> --attempt <a> --verdict pass|fail "<findings path or text>"`, adding `--friction "<what cost time>"` when over budget. What to deliver: `gear --skill review`.

## 5. Captain

To become a project's captain, run `gear --session <session> --pane <your pane> captain take <project>` (the user or the handing-over captain gives you the session and pane). Your pane becomes the captain seat at a new generation.

The captain's guide: `gear --skill captain`. Holding the course: `gear --skill course`. Cutting work into tasks: `gear --skill decompose`.

## 6. Topics

| Topic | For |
|---|---|
| `conventions` | names, branches, worktrees, the lane's task loop, questions, review and merge |
| `captain` | kickoff, config, the plan file, the daily loop |
| `departure` | the departure sheet the user confirms before every voyage departs |
| `course` | keeping the work aimed at the goal: findings, eta, signals, landing |
| `decompose` | cutting work into tasks |
| `review` | what a temp reviewer delivers |
| `adopt` | taking over a project that already runs: its instruction layer, exemplars, gate and working agreement |
| `github` | issues and pull requests on GitHub: the mode, what the engine writes and when, issues into tasks |
| `agent:claude`, `agent:codex`, `agent:opencode`, `agent:grok`, `agent:pi` | driving one agent CLI through Herdr: start, model, resume, exit, quota signs |

The user installs this skill into an agent CLI with `gear admin install-skill --agent <claude|codex|opencode|grok|pi|omp|agy>`.

## 7. Errors

| Code | Meaning | Do |
|---|---|---|
| `NOT_REGISTERED` | no valid seat for this process | run the command as GEAR's messages write it (OpenCode and Codex: with `--session` and `--pane`); if it still fails, stop write commands and tell the captain/user |
| `STALE_GENERATION` | your seat was restarted or taken over | stop; run `gear` |
| `FORBIDDEN` | your role or state may not do this | do not retry; ask |
| `BAD_FORMAT` | arguments or text malformed (the detail says where) | fix and resend |
| `STALE_VERSION` | the plan changed since your draft's base | re-read the plan, reapply |
| `UNCONFIRMED` | outcome unknown | rerun with the same `--request-id` |
| `NO_TARGET` | the task, message or seat does not exist | check the id |
| `BAD_TRANSITION` | that state change is not allowed now | run `gear`; do not force |
| `REQUEST_CONFLICT` | this `--request-id` was used with other arguments | resend the original exactly, or use a new id |
| `QUOTA` | no capacity for that (lanes/temps) | wait; the captain decides |
| `ENGINE_DOWN` | the engine is not running | your write is stored; tell the user if it persists |
| `LOCKED` | the store is busy | retry shortly |
| `SCHEMA_TOO_NEW` | the store is newer than this binary | tell the user to upgrade GEAR |
