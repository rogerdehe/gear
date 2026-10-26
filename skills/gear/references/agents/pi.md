# Agent SOP: pi

For whoever drives a pi agent through Herdr (captain, user or the GEAR engine). After every step, read the screen and check it shows what is listed. Never type into a pane before checking who occupies it.

Tested with pi 1.4.0, Herdr 0.9.1, macOS.

## Facts

| Item | Value |
|---|---|
| Herdr kind | `pi` |
| Unattended flag (required) | `--approve` — pi's own project-trust override; without it a lane in a fresh worktree waits on a trust prompt |
| Model | start flag `--model <provider>/<id>` (e.g. `opencode-go/deepseek-v4.1-flash`) |
| Thinking level | start flag `--thinking <off\|minimal\|low\|medium\|high\|xhigh\|max>` (the chain entry's args) |
| Session | Herdr reports the session **file** (`agent_session.value` is a path under `~/.pi/agent/sessions/`) |
| Resume | `--session <path\|id>` — the path Herdr reports works; pi's own hint after an exit names the session id |
| Startup line | ` ▀▀█  v1.4.0` and `escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more` |
| Status line | `<provider> <model> • <thinking>` at the right, `↑… ↓… R… CH… $…` at the left |
| Busy sign | ` Working` — the spinner line (`── ⠋ Working ────`; the glyph changes, the word does not) |
| Exit | the `/quit` command; `ctrl+d` exits only while the editor is empty |

## Start

```bash
herdr agent start <name> --kind pi --pane <pane> --timeout 60000 -- \
  --model <provider>/<id> --approve
```

Check: `"agent_status":"idle"`, the status line shows the model, `agent get` shows the session file.

## Prompt, exit, resume

```bash
herdr agent prompt <name> "<text>" --wait --timeout <ms>
herdr agent prompt <name> "/quit"
herdr agent start <name> --kind pi --pane <pane> --timeout 60000 -- \
  --model <provider>/<id> --approve --session <session-file>
```

After `/quit` the agent is gone, the shell prompt is back and the screen shows `To resume this session: pi --session <uuid>`.

## Provider signs

Taken from pi's own output (1.4.0); the engine matches them in the last 16 lines:

| Sign | On screen |
|---|---|
| Quota spent | `Error: 429: {"message":"You exceeded your current quota, …","type":"insufficient_quota",…}` |
| Rate limited | `Error: 429: {"message":"Rate limit reached for …","type":"rate_limit_error",…}` |
| No credentials | `Error: No API key found for <provider>.` |
| Provider back-off | `── ⠙ Retrying (3/3) in 7s... (escape to cancel) ────` — pi retries on its own, so this is not a stall |

## Guardrails

`@aliou/pi-guardrails` refuses a dangerous command or a path outside the workspace, and the agent goes on. The two deny lines are:

- `Command auto-denied: <why>` (the `permission-gate` extension)
- `Access to <path> is blocked (outside working directory).` (the `path-access` extension)

The GEAR engine reads them and shows them to the captain: a lane that was told to do something it may not do is worth knowing about.

## Pitfalls

- **The first prompt right after a start can be lost**; a prompt counts as delivered only when the agent is seen `working`.
- **`/quit` is a command, not a key**: send it as a prompt, never as typed text — typing `/` opens pi's command menu.
- **The model is per process**: switching models means a restart with another `--model`; pi has no model picker GEAR drives.
