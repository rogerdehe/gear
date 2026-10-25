# Agent SOP: opencode

For whoever drives an OpenCode agent through Herdr (captain, user or the GEAR engine). After every step, read the screen and check it shows what is listed. Never type into a pane before checking who occupies it.

Tested with OpenCode 2.1, Herdr 0.9.1, macOS.

## Facts

| Item | Value |
|---|---|
| Herdr kind | `opencode` |
| Unattended flag (required) | `--auto` — without it the agent stops on permission dialogs (Herdr: `blocked`) |
| Background service | the TUI attaches to a shared local service; do **not** use `--standalone` for lanes (private servers exit on their own) |
| Model and effort (variant) | belong to the **session** on the service; set them through `opencode api` (below) |
| Session id | not reported by Herdr; GEAR creates the session and records its id |
| Resume | `--session <id>` (or `-s <id>`) |
| Status bar | `Build auto · <model> <provider> · <variant>` above the input — `auto` confirms the unattended flag |
| Busy sign | `esc interrupt` at the bottom left while a turn runs |
| Exit | one `ctrl+c` |

## Check the pane first

```bash
herdr agent get <name-or-pane>                    # kind + status
herdr pane read <pane> --source visible --lines 30
```

An agent is present → anything typed goes into its input as a prompt. Only act on `idle` or `done`.

## Start on a chosen model

```bash
cd <worktree>
opencode api session.create -d '{"location":{"directory":"<worktree>"},"agent":"build","model":{"providerID":"<provider>","id":"<model>"}}'
# → the new session id
herdr agent start <name> --kind opencode --pane <pane> --timeout 60000 -- --auto --session <id>
```

Check: the status bar shows the model. List models and their variants with `opencode api model.list` or `opencode models`. A project-wide default model can also go into the repo's `opencode.json` (`{"model":"<provider>/<model>"}`); the environment variable `OPENCODE_CONFIG_CONTENT` is ignored by the shared service.

## Switch model or effort in a running session

```bash
opencode api session.switchModel --param sessionID=<id> -d '{"model":{"providerID":"<provider>","id":"<model>","variant":"<variant>"}}'
opencode api GET /api/session/<id>                # check .model
```

The running TUI follows; check its status bar. The variant must be one of the model's variant ids (from `model.list`) or it is dropped. This changes only that session, never the machine-wide default.

Fallback by keys (agent `idle`): **type** `/models` (`pane send-text`), press `enter`, type a filter specific enough to leave exactly one row (`<model> <provider>`; the same model exists under several providers, including free tiers), press `enter`, answer a "Select variant" dialog if one opens, then check the status bar. A pasted slash command (`agent prompt`, `pane run`) is sent to the model as a chat message — type it. `/effort` opens the variant dialog the same way. Setting a variant by keys changes that model's default for every new session on the machine.

## Exit and resume

```bash
herdr agent send-keys <name> ctrl+c              # once
herdr agent start <name> --kind opencode --pane <pane> --timeout 60000 -- --auto --session <id>
```

After the exit the screen shows `Continue  opencode -s <id>` and the shell prompt. A resumed session shows its old context size at the bottom right. Resume only from the directory the session was created in.

## Quota

A provider limit looks like a finished turn: Herdr says `done`. The screen shows `Session failed` and an `Error:` line from the provider (MiniMax: `Token Plan … (2056)`; OpenCode free tier: `Free usage limit reached`). Treat it as quota — switch the model or wait for the reset — never as completion.

## Pitfalls

- **The first prompt right after a start can be lost.** Wait until the status bar is drawn; a prompt counts as delivered only when the agent is seen `working`.
- **Dialogs are drawn mid-screen.** Read the whole visible screen; anything typed while a dialog is open goes into its search box (`esc` closes it).
- **A shell command typed into an OpenCode pane** is submitted as a prompt; `herdr agent send-keys <name> esc` stops the turn.
- **Environment exported in the pane** before the start reaches the agent's commands, and stays in the shell after the agent exits.
