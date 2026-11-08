# Agent SOP: claude

For whoever drives a Claude Code agent through Herdr (captain, user or the GEAR engine). After every step, read the screen and check it shows what is listed. Never type into a pane before checking who occupies it.

Tested with Claude Code 2.1, Herdr 0.9.1, macOS.

## Facts

| Item | Value |
|---|---|
| Herdr kind | `claude` |
| Unattended flag (required) | `--dangerously-skip-permissions` — the status line then shows `bypass permissions on` |
| Model | start flag `--model <full model id or alias>`; prefer the full id so it never moves |
| Effort | start flag `--effort <low\|medium\|high\|xhigh\|max>` |
| Session id | Herdr reports it (`agent get` → `agent_session.value`) after the first turn |
| Resume | `--resume <session-id>` |
| Status bar | `<Model> · <effort> <dir> (<branch>) · ctx N% · 5h N% ↻<reset> · 7d N% ↻<reset>` |
| Busy sign | `esc to interrupt`; a spinner line above the input |
| Exit | two `ctrl+c` (the first interrupts a running turn) |

## Check the pane first

```bash
herdr agent get <name-or-pane>
herdr pane read <pane> --source visible --lines 30
```

Only act on `idle` or `done`.

## Start

```bash
herdr agent start <name> --kind claude --pane <pane> --timeout 60000 -- \
  --dangerously-skip-permissions --model <model> --effort <effort>
```

Check: `"agent_status":"idle"` and the status bar shows the model and `bypass permissions on`.

## Prompt

```bash
herdr agent prompt <name> "<text>" --wait --timeout <ms>
```

## Exit and resume

```bash
herdr agent send-keys <name> ctrl+c ctrl+c
herdr agent start <name> --kind claude --pane <pane> --timeout 60000 -- \
  --dangerously-skip-permissions --resume <session-id>
```

Resume from the directory the session was created in; the resumed session shows its old `ctx N%`. Change the model or effort by restarting with other flags (a resume with new flags keeps the conversation). Never type `/model` or `/effort` into a running Claude Code: it answers 'saved as your default for new sessions' and rewrites ~/.claude/settings.json, the model of every Claude started on this machine without `--model`.

## Quota

A limit stop looks like a finished turn. Claude Code records it in its session log (`quotaLimits` with `status` `rejected` and `resetsAt`); GEAR reads that record for the captain, so no status line is needed. A status line that shows `5h N%` / `7d N%` (usage, 100% = exhausted, `↻` the reset) is read too. GEAR never reads quota words quoted in the conversation.

## Pitfalls

- **The first prompt right after a start can be lost** (the input is not ready yet, or the Enter is lost and the text stays in the box). Wait until the status bar is drawn; a prompt counts as delivered only when the agent is seen `working`.
- **Without the unattended flag** Claude Code stops on a permission question and Herdr reports it `blocked`.
- **Queued input:** the box shows `Press up to edit queued messages` — read the screen before typing again.
