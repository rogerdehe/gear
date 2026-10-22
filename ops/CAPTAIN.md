# Captain loop (manual scaffold)

While GEAR cannot run itself, the captain performs by hand what the engine will do. Every step names the GEAR piece that replaces it, so this file is the automation spec.

Borrowed from BMAD: one status board with a single writer (sprint-status.yaml → `ops/PLAN.md`); a task file that carries its own checklist and the agent's record (story file → task packet); "tick only when tests pass"; review findings go back into the task file as follow-up items. Borrowed from bmad-loop: the agent never writes the board — it writes its own file, the orchestrator verifies and then advances the board; completion is detected by the agent stopping, then the result is read from disk and re-verified.

## State

| Thing | Where | Writer | GEAR replacement |
|---|---|---|---|
| Task board (state, lane, order) | `ops/PLAN.md` | captain only | `tasks` table + `gear plan apply` |
| Task contract | `ops/tasks/T-<n>.md` top part | captain | task packet (06) |
| Lane progress | packet `Status:`, checkboxes, Lane record — on the lane's pushed task branch | lane only | `gear send --kind progress/result` + work sampling (12) |
| Lane's task list | its rows in `ops/PLAN.md` | captain | home view "your tasks" |
| Lane's dev list | packet `## Tasks` checklist | captain writes, lane ticks | same (packet) |
| Captain → lane messages | `ops/mail/<lane>.md` (`### M-<n> [open] …`; captain closes with `[closed]`) | captain | inbox + `gear reply` (07) |
| Lane inbox view | `ops/bin/inbox.sh <lane>` | — | `gear` home (13) |
| Captain doorbell | `ops/bin/watch.sh` in the background | — | engine observe + doorbell (08) |
| Event log | `ops/PLAN.md` → Log | captain | `audit` table |

Board states (GEAR `TaskState`, plus `rework` = ready-with-follow-ups shown separately): `draft → ready → running → verifying → done`; `verifying → rework → running`; `waiting` (question/blocked); `failed`, `cancelled`. Packet `Status:` is the lane's claim; the board changes only after the captain checks it.

## Loop

Lanes never wait for review (17 §5): they take their next ready task themselves. The captain keeps queues stocked, reviews asynchronously, and answers.

1. **Stock the queues** (GEAR: plan apply, 05) — every lane has ready tasks that start from current `dev` without each other (`gear --skill decompose`). Board rows in queue order; branch names fixed in the packets.
2. **Kick off** a lane once (GEAR: first deliver, 09): `herdr agent prompt <agent> "Read ops/CONVENTIONS.md, then run ops/bin/inbox.sh <lane> and work your queue."`. After that lanes pull work via the inbox; no per-task prompts.
3. **Arm the doorbell** (GEAR: engine observe/doorbell, 08) — run `ops/bin/watch.sh` with `run_in_background`. It exits on: a pushed packet turning `review`/`question`/`blocked`, or a lane agent stopping. Re-arm it after handling every event.
4. **Handle a `packet … review` event** — board `→ verifying`; queue a review row `R-<n> | review | review/t<m> | review T-<m> (branch …) | ready` for the review lane (wake it if stopped). The captain does **not** review.
4b. **Handle a `packet R-<n> … done` event** (GEAR: verdict, 11) — read `origin/review/t<m>:ops/reviews/T-<m>.md`; look at code only where the review asks the captain something; decide:
   - pass: on `dev`: `git merge --no-ff origin/<branch> -m "merge(T-n): …"`, run acceptance on `dev`, `git push origin dev`, `git push origin --delete <branch>`; board `verifying → done`; tasks whose dependencies are now merged go `draft → ready`; log.
   - fail: commit follow-ups `- [ ] [Review][P<n>] …` into the packet **on that branch** (in a scratch worktree of `origin/<branch>`, push); board `verifying → rework`; if urgent leave it, else add `after-current` in a mail message; log. The lane picks it up from its inbox.
5. **Handle `packet … question/blocked`** (GEAR: inbox + reply, 07) — board `→ waiting`; answer as `### M-<n> [open] T-<n>: <answer>` in `ops/mail/<lane>.md` on `dev` (commit, push); board `waiting → running`. If the lane has stopped, wake it with `herdr agent prompt <agent> "Run ops/bin/inbox.sh <lane>."`.
6. **Handle `stopped <agent>`** — read the screen (`herdr agent read <agent> --source recent-unwrapped --lines 80`) and `ops/bin/board.sh`: queue empty / at cap (normal — refill or review), rate limit / quota / crash (switch model per `gear --skill agent:<kind>`), permission dialog (should not happen with `--auto`), or an early turn end (prompt it to run its inbox). Never prompt an agent that is working.
7. **Re-plan** whenever a lane's queue has no startable task (17 §6a.6).

## Rules the captain holds itself to

- Never type into a pane before checking its occupant; never prompt a working agent.
- Never resend a prompt whose delivery is uncertain; read the screen first.
- Writer ≠ reviewer: lanes run DeepSeek/MiniMax; the captain (Claude) reviews.
- The board is written only by the captain, on `dev`; every change gets a log line.
