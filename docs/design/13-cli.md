# 13 Command Surface, Home Page, Board

Files: `gear/__main__.py`, `gear/cli/*.py`, `gear/engine/board.py`. Budget 200–250 lines.

## Everyday commands (used by Agents)

| Command | Who | Arguments | Returns immediately |
|---|---|---|---|
| `gear` | All seats | `--show <ID/thread/queue/draft>` (captain `--show draft` exports a new baseline draft to a new file), `--diff`, `--json` | Role home page (below) |
| `gear send` | Lanes, temp workers, captain | `--kind progress\|question\|blocked\|result\|ack\|instruction`; `--task/--attempt/--contract` (required for result, blocked; ack is validated against the item being replied to, handover ACK does not need them; the task packet gives the full command); `--about T-x`, `--need`, `--to <seat>` (captain only), `--reply-to M-x`, `--urgent`, `--candidate`, `--preempted T-x --saved <ref>`, `--request-id`; body (argument or stdin) | 07's delivery result |
| `gear reply` | Captain | `M-x …`, `--resolve` (default; when only `--downgrade` is given the default is note)/`--wait <whom> [--until <time>]`/`--note`, `--downgrade`, `--next-check <time>`, `--request-id`, body | Handling result for each ID |
| `gear --skill [captain\|plan-format\|errors]` | Everyone | — | Prints the skill matching the current version (16), read-only |
| `gear plan apply` | Captain | `[draft path]` (defaults to the project's `plan.draft.md`), `--keep V-n --next <time> "reason"` (shortcut: submit only that review item's conclusion, linked to the current plan version), `--request-id` | 05's apply result |

Lanes only need to remember `gear` and `gear send`; task packets give complete commands that can be copied directly. All write commands print the request_id to stderr before committing, so it can be used to re-run when the receipt is lost. A new captain confirms a handover with `gear send --kind ack --reply-to <handover item>`; no separate command.

## Ops commands (used by the user, not given to Agents)

| Command | Purpose |
|---|---|
| `gear run` | Start the engine (invoked by the plugin pane) |
| `gear admin init <project.toml>` | Register/update the project config (01) |
| `gear admin seat add/confirm/remove …` | Register a lane (name, duty, owns, kind; no pane or path — the engine creates its worktree and pane via Herdr, 17 §4); confirm write resources are safe; remove |
| `gear admin captain replace --project P --pane X --kind K` | Replace the captain manually (03) |
| `gear --all` | Read-only overview of all projects for the user |

Ops commands must be run in the user's own shell (no `GEAR_CTX`), with `GEAR_ADMIN=1` or interactive confirmation; any Agent invocation is rejected. `seat add` only registers the seat; the engine starts onboarding (03).

## Home page content

**Lane**: identity line (seat, generation) → current task in one line + task packet path → unread replies/instructions for me (original text) → contract update notices → suggested next command.

**Captain**:
1. Handover/anomaly banner (if any);
2. **Due reviews** and snapshot (11);
3. One line per lane: state, current task, standby/dwell time, most recent output time; temp worker quota;
4. Inbox batch (07): thread, original text, ID;
5. Other: count of unmentioned tasks, items needing check, engine heartbeat.

Total length limit ~120 lines; overflow is written as "N more, `gear --show …`". Reading the home page changes no message/task state; it only records one "view record": seat, generation, time, and the task/attempt/contract versions, message IDs, and related operation IDs **actually shown** this time. The record is written only after output succeeds; if output and recording are interrupted, it counts as not shown. The record only proves the content was fetched; it does not prove the items were handled or the contract adopted. 08 uses it to confirm deliveries/doorbells.

## Board (plugin pane, `board.py`)

Text refreshed every tick: one block per project — captain state, each lane's state and task, temp workers (in use/quota/queued/resumable), pending count and oldest wait, due reviews, engine heartbeat, observation failures. Read-only, accepts no keyboard input (MVP). Does not update the pane title every tick (only when state changes).

## Test points

Argument validation and error codes for each command; home page is read-only; `--json` structure is stable; lanes invoking captain commands are rejected.
