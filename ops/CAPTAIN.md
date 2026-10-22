# Captain loop (manual scaffold)

While GEAR cannot run itself, the captain performs by hand what the engine will do. Every step names the GEAR piece that replaces it, so this file is the automation spec.

Borrowed from BMAD: one status board with a single writer (sprint-status.yaml → `ops/PLAN.md`); a task file that carries its own checklist and the agent's record (story file → task packet); "tick only when tests pass"; review findings go back into the task file as follow-up items; one story per agent at a time. Borrowed from bmad-loop: the agent never writes the board — it writes its own file, the orchestrator verifies and then advances the board; completion is detected by the agent stopping, then the result is read from disk and re-verified.

## State

| Thing | Where | Writer | GEAR replacement |
|---|---|---|---|
| Task board (state, lane, order) | `ops/PLAN.md` | captain only | `tasks` table + `gear plan apply` |
| Task contract | `ops/tasks/T-<n>.md` top part | captain | task packet (06) |
| Lane progress | packet `Status:`, checkboxes, Lane record — in the lane's worktree | lane only | `gear send --kind progress/result` + work sampling (12) |
| Lane's task list | its rows in `ops/PLAN.md` | captain | home view "your tasks" |
| Lane's dev list | packet `## Tasks` checklist | captain writes, lane ticks | same (packet) |
| Event log | `ops/PLAN.md` → Log | captain | `audit` table |

Board states use GEAR's `TaskState`: `ready → running → verifying → done`, plus `waiting` (question/blocked), `failed`, `cancelled`. Packet `Status:` is the lane's claim; the board changes only after the captain checks it.

## Loop

1. **Dispatch** (GEAR: assign + deliver op, 09)
   - board: task `ready → running`, log it;
   - make sure the lane's worktree is on the task branch cut from current `dev` (`git -C <worktree> switch -c <lane>/t<n>-<slug> dev`);
   - deliver a one-line pointer and arm the doorbell in one background command:
     `herdr agent prompt <agent> "Task T-<n>: read ops/tasks/T-<n>.md and ops/CONVENTIONS.md, then work it." --wait`
     run with `run_in_background`; it returns when the lane settles (idle/done/blocked) after starting work.
2. **Doorbell** (GEAR: engine observe + doorbell, 08) — the background command finishing re-invokes the captain. Fallback: on any wake, run `ops/bin/board.sh` and treat every lane whose agent is idle/done as rung (idempotent: a lane already handled has no new packet status).
3. **Read** (GEAR: `gear` home) — `ops/bin/board.sh`, then the packet in the lane's worktree. If the agent is `blocked` (approval dialog) or the packet status is still `running`, read the screen: `herdr agent read <agent> --source recent-unwrapped --lines 80` (rate limit, quota, crash, or a turn that ended early).
4. **Answer** (GEAR: `gear reply` → deliver) — for `question`/`blocked`: board `running → waiting`, log; reply with `herdr agent prompt <agent> "<answer>" --wait` in the background (same doorbell); board back to `running`.
5. **Nudge** (GEAR: continue, 09) — lane stopped with unticked boxes and no question: one prompt "Continue T-<n> from the first unticked box." Same failure twice → read the screen and decide; never loop blindly.
6. **Review** (GEAR: verdict, 11/17 §8) — packet `review`: board `running → verifying`; run the merge checklist (17 §8): brief complete, diff only in allowed files, acceptance re-run by the captain in the lane's worktree, zero warnings, branch current with `dev`.
   - fail: add `- [ ] [Review][P<n>] …` items under the packet's "Review follow-ups" (committed on the task branch), board `verifying → running`, prompt "Rework T-<n>: see Review follow-ups." in the background.
   - pass: merge into `dev` (`git merge --no-ff <branch>` from the main checkout on `dev`), verify, push `dev`, delete the task branch, switch the lane worktree back to `home/<lane>` or straight to its next task branch; board `verifying → done`, log.
7. **Next** — pick the lane's next `ready` row in board order and go to 1. No ready row → the lane is on standby; say so in the log, send nothing.

## Rules the captain holds itself to

- One open task per lane. Never prompt a lane that is working.
- Never resend a pointer whose delivery is uncertain; read the screen first.
- Writer ≠ reviewer: lanes run DeepSeek; the captain (Claude) reviews.
- Every board change gets a log line with the time.
