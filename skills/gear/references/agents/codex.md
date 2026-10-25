# Agent SOP: codex

For whoever drives a Codex agent through Herdr (captain, user or the GEAR engine). After every step, read the screen and check it shows what is listed. Never type into a pane before checking who occupies it.

Tested with codex-cli 0.163, Herdr 0.9.1, macOS.

## Facts

| Item | Value |
|---|---|
| Herdr kind | `codex` |
| Unattended flag (required) | `--dangerously-bypass-approvals-and-sandbox` — the header then says `permissions: YOLO mode` |
| Update check | start flag `-c check_for_update_on_startup=false` — without it Codex shows an "Update available" dialog at start and the agent sits blocked, unseen (2026-10-25, codex 0.163.0 → 0.164.0) |
| Model | start flag `-m <slug>` (e.g. `gpt-6.1-sol`); always pass it — otherwise the user's default model from `~/.codex/config.toml` is used |
| Reasoning effort | start flag `-c model_reasoning_effort=<low\|medium\|high\|xhigh>` |
| Session id | Herdr reports it (`agent get` → `agent_session.value`) after the first turn |
| Resume | `codex resume <session-uuid>` (a subcommand: pass `resume <uuid>` plus the same flags) |
| Status bar | `<Model> <effort> · <thread> · <dir> · Context N% used · 5h … · weekly N% left` |
| Busy sign | `• Working (<time> • esc to interrupt)` |
| Exit | two `ctrl+c` (the first may only interrupt) |

## Check the pane first

```bash
herdr agent get <name-or-pane>
herdr pane read <pane> --source visible --lines 30
```

Only act on `idle` or `done`.

## Start

The pane must show the shell prompt of the agent's worktree (a freshly made pane needs a few seconds; `agent start` returns `agent_pane_busy` until then).

```bash
herdr agent start <name> --kind codex --pane <pane> --timeout 60000 -- \
  -m <slug> -c model_reasoning_effort=<effort> -c check_for_update_on_startup=false --dangerously-bypass-approvals-and-sandbox
```

Check: `"agent_status":"idle"` and the header `permissions: YOLO mode`; the status bar shows the model (the effort may read `default` until the first turn — `/status` shows it). Startup warnings about the user's own `~/.codex/config.toml` (`⚠ N warnings`) are harmless.

## Prompt

```bash
herdr agent prompt <name> "<text>" --wait --timeout <ms>
```

A long turn makes `--wait` time out; that does not mean the prompt was lost — `agent get` shows `working`.

## Exit and resume

```bash
herdr agent send-keys <name> ctrl+c ctrl+c
herdr agent start <name> --kind codex --pane <pane> --timeout 60000 -- \
  resume <session-uuid> -m <slug> -c model_reasoning_effort=<effort> -c check_for_update_on_startup=false --dangerously-bypass-approvals-and-sandbox
```

After the exit the screen shows `codex resume <uuid>` and the shell prompt. After a resume wait until the status bar shows `Context N% used` before prompting. Never resume with an empty id: it opens a picker, and the next prompt lands in its search box.

## Quota

The status bar carries the limits (`5h …`, `weekly N% left`). A stop on a limit or a server error ("model at capacity") looks like a finished turn: read the screen before treating a stop as completion.

## Pitfalls

- **Switching the model needs a restart** with another `-m`; there is no in-session switch GEAR uses.
- **The first prompt right after a start can be lost**; a prompt counts as delivered only when the agent is seen `working`.
