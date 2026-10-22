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
| Model / effort / context | switched inside the agent: `/model`, `/effort`, `/clear`; never start flags |
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

## Switch effort (model variant)

Agent must be `idle`. Only models with variants have it; the status bar shows the current one at the end (`… DeepSeek · high`). No variant shown (e.g. MiniMax-M3.1-Flash) → the model has none and `/effort` opens nothing.

```bash
herdr pane send-text <pane> "/effort" && herdr pane send-keys <pane> enter
herdr pane read <pane> --source visible          # a "Select variant" dialog in the MIDDLE of the screen:
                                                 # Search / Default / none / low / high / max (● = current)
herdr pane send-text <pane> "<variant>"          # filter, e.g. low
herdr pane send-keys <pane> enter
herdr pane read <pane> --source visible          # status bar ends with · <variant>
```

- Read the whole visible screen, not just the last lines: dialogs are drawn mid-screen.
- Anything typed while a dialog is open goes into the dialog's search box (typing `/effort` again shows "No results found"). Clear it with `backspace`, or close the dialog with `esc`.
- Verified on DeepSeek V4.1 Flash: high → low → high.

## Clear context

Agent must be `idle`.

```bash
herdr pane send-text <pane> "/clear" && herdr pane send-keys <pane> enter
herdr pane read <pane> --source visible          # OpenCode start screen, "Ask anything…"
```

- Keeps the model, variant and `auto`; the old session stays in `opencode session list`.
- No new session id exists until the first message is sent — look the id up again after the next prompt before relying on it for resume.

## Pitfalls

- **Provider quota / plan limit looks like a finished turn.** Herdr reports `done`, not `blocked`. The screen shows a "Session failed" toast and an `Error:` line from the provider, e.g. MiniMax: `Error: <message in Chinese saying the Token Plan usage limit is reached> (2056)` (seen 2026-10-22). Always read the screen when an agent stops; treat this as quota (wait for the reset or `/model` to another provider), never as completion. Profile quota markers: `Session failed`, `Token Plan`, `(2056)`, and OpenCode's free tier: `Free usage limit reached`.
- **The first prompt right after `agent start` can be lost.** `agent start` returns once Herdr sees the TUI, but the input box may not take a prompt yet: `agent prompt --wait` then reports `agent_prompt_stalled` and the screen shows the empty welcome box. Wait ~2 s (or until the status bar `Build auto …` is drawn) and prompt again; a prompt counts as delivered only when the agent is seen `working` (verified 2026-10-22).
- **Environment set in the pane reaches the agent's commands.** `pane run <pane> "export GEAR_CTX=…"` before `agent start` → the agent's shell tool sees it (`printenv GEAR_CTX` from inside OpenCode printed the value; verified 2026-10-22). The shell keeps it after the agent exits — `unset` it before the pane is reused.
- **Auto-compaction ends the turn.** When the context fills, OpenCode writes a summary ("Work State / Next Move") and stops; Herdr shows `idle`/`done`. It is not completion: prompt the agent to continue (run its inbox).
- **A session belongs to its directory.** `opencode -s <id>` started elsewhere still works in the directory the session was created in (`opencode session export <id>` → `"directory"`). Resume a session only from that directory.
- **A model with variants opens a second dialog.** After choosing e.g. `DeepSeek V4 Pro` in `/model`, a mid-screen "Select variant" list (Default/none/low/high/max) appears; anything typed next — including a prompt — goes into its search box. Pick the variant (type it, Enter) and check the status bar (`… · high`) before prompting.
- **Same display name, different provider.** The `/model` picker lists e.g. `DeepSeek V4.1 Flash 1017` (DeepSeek) and `DeepSeek V4.1 Flash Free` (OpenCode free tier). Filter with and verify the full label; a prefix can pick the wrong one.

- Typing a shell command into a pane that hosts OpenCode submits it as a prompt and the agent starts working on it. Check the occupant first; if it happened, `herdr agent send-keys <name> esc` stops the turn.
- `herdr agent prompt … --wait` returns when the agent settles; a settled agent is not necessarily finished with the task — read its task packet / screen.
