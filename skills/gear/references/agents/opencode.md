# Agent SOP: opencode

For whoever drives an OpenCode agent through Herdr (captain, user, or the GEAR engine). Every step: run the command, then check the screen shows what is listed before going on. Never type into a pane before checking who occupies it.

Verified on: OpenCode v2.1.2, Herdr 0.9.1 (protocol 22), macOS, 2026-10-22.

## Facts

| Item | Value |
|---|---|
| Herdr kind | `opencode` |
| Unattended flag (mandatory) | `--auto` — without it the agent stops on permission dialogs (Herdr status `blocked`) |
| Resume flag | `-s <session-id>` |
| Herdr reports `agent_session` | no — find the id yourself (below) |
| Exit | one `ctrl+c` |
| Model / effort | switched inside the agent: `/model`, `/effort`; not a start flag |
| Status bar (bottom, above the input) | `Build auto · <model> <provider> · <effort>` — `auto` confirms the unattended flag |
| Busy sign | `esc interrupt` at the bottom-left while a turn runs |

## Before anything: check the pane

```bash
herdr agent get <name-or-pane>        # agent kind + agent_status
herdr pane read <pane> --source visible --lines 30
```

- An agent is present → do not use `pane run` / `pane send-text` for shell commands; anything typed goes into the agent's input as a prompt.
- Only act on an agent whose status is `idle` or `done`.

## Start

Pane must be an empty shell prompt in the agent's worktree.

```bash
herdr agent start <name> --kind opencode --pane <pane> --timeout 60000 -- --auto
```

Check: reply `"type":"agent_started"`; status bar shows `Build auto · …`.

## Find the session id

Herdr does not report it. Sessions are listed per repository (all worktrees of a repo share one list), so match on the directory:

```bash
cd <worktree> && opencode session list                  # id, title, time (newest first)
opencode session export <id> | python3 -c 'import json,sys; d=json.load(sys.stdin)["info"]; print(d["location"]["directory"], d["model"])'
```

Take the id whose `location.directory` equals the agent's worktree. On exit OpenCode also prints it: `Continue  opencode -s <id>`.

## Exit

```bash
herdr agent send-keys <name> ctrl+c     # once; it may take a moment
herdr pane read <pane> --source recent-unwrapped --lines 6
```

Check: the screen shows `Session <title>` / `Continue opencode -s <id>` and the shell prompt; `herdr agent get <name>` returns `agent_not_found`. Do not press `ctrl+c` again — the agent is already gone and the key would hit the shell.

## Resume

```bash
herdr agent start <name> --kind opencode --pane <pane> --timeout 60000 -- --auto -s <session-id>
```

Check: `agent_started`; status bar `Build auto · …`; the context counter at the bottom right (e.g. `15.1K (1%)`) matches what it was before exit — that proves the old context is loaded.

## Switch model

Agent must be `idle`.

```bash
herdr pane send-text <pane> "/model" && herdr pane send-keys <pane> enter
herdr pane read <pane> --source visible          # a picker with "Search", "Recent", provider groups
herdr pane send-text <pane> "<model name>"       # filters the list, e.g. MiniMax-M3.1-Flash
herdr pane read <pane> --source visible          # exactly one row left, with the provider you want
herdr pane send-keys <pane> enter
herdr pane read <pane> --source visible          # status bar: Build auto · <model> <provider>
```

- Use the name as the picker shows it. List names with `opencode models` (`<provider>/<model>`, e.g. `minimax-cn/MiniMax-M3.1-Flash`); the picker shows `MiniMax-M3.1-Flash  MiniMax (minimax.cn)`.
- The same model may exist under several providers (own account vs. `… Free  OpenCode`). Filter until only the intended provider's row remains before pressing enter.
- Use `pane send-text` / `pane send-keys`, not `agent prompt`: this is UI input, not a task.

## Switch effort

Unverified. `/effort` exists; after switching to MiniMax-M3.1-Flash the status bar no longer shows an effort level, so the model may have none. Verify before relying on it.

## Pitfalls

- Typing a shell command into a pane that hosts OpenCode submits it as a prompt and the agent starts working on it. Check the occupant first; if it happened, `herdr agent send-keys <name> esc` stops the turn.
- `herdr agent prompt … --wait` returns when the agent settles; a settled agent is not necessarily finished with the task — read its task packet / screen.
