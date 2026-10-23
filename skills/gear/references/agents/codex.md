# Agent SOP: codex

For whoever drives a Codex agent through Herdr (captain, user, or the GEAR engine). Every step: run the command, then check the screen shows what is listed before going on. Never type into a pane before checking who occupies it.

Verified on: codex-cli 0.163.0, Herdr 0.9.1 (protocol 22), macOS, 2026-10-22 (first GEAR reviewer on Codex).

## Facts

| Item | Value |
|---|---|
| Herdr kind | `codex` |
| Unattended flag (mandatory) | `--dangerously-bypass-approvals-and-sandbox` — the screen header then says `permissions: YOLO mode` |
| Shared service | the TUI attaches to a shared local `codex app-server daemon` by default (`codex agents` lists its sessions); `--no-daemon` runs without it (the counterpart of OpenCode's `--standalone`). GEAR's reviewers use the daemon (default) |
| Model | start flag `-m <slug>`, applied per session even on the shared daemon (verified with `/status` on every reviewer). Slugs: `~/.codex/models_cache.json` (`slug`), e.g. `gpt-6.1-sol` |
| Reasoning effort | start flag `-c model_reasoning_effort=high` (low / medium / high / xhigh) |
| Herdr reports `agent_session` | yes — `agent get` → `agent_session.value` (UUID) |
| Resume | `codex resume <session-uuid>` — a subcommand, so pass `resume <uuid>` plus the same flags after `--` |
| Busy sign | `• Working (<time> • esc to interrupt)` above the input |
| Status bar (bottom) | `<Model> <effort> · <thread name> · <dir> · Context N% used` |

## Before anything: check the pane

```bash
herdr agent get <name-or-pane>        # agent kind + agent_status
herdr pane read <pane> --source visible --lines 30
```

Only act on an agent whose status is `idle` or `done`. Anything typed into a pane with an agent goes into its input.

## Start

The pane must show the shell prompt of the agent's worktree. A pane that a fresh `herdr worktree create` just made is not ready for a few seconds (shell init); `agent start` then returns `agent_pane_busy` — wait for the prompt, then start.

```bash
herdr agent start <name> --kind codex --pane <pane> --timeout 60000 -- \
  -m <slug> -c model_reasoning_effort=<effort> --dangerously-bypass-approvals-and-sandbox
```

Check: reply contains `"agent_status":"idle"` (about 5 s); header `permissions: YOLO mode`. Right after start the status bar may show `<Model> default`; the effort is applied — confirm with `/status` (`Model: <Model> (reasoning <effort>, …)`) or after the first turn the bar shows the effort.

Startup warnings (`⚠ N warnings · f2 to view`) about ignored keys in the user's `~/.codex/config.toml` are harmless; `f2` opens them, `esc` closes.

## Prompt

```bash
herdr agent prompt <name> "<text>" --wait --timeout <ms>
```

A long first turn makes `--wait` time out; that does not mean the prompt was lost — check `agent get` shows `working`.

## Exit

Only when the agent is `idle`/`done`:

```bash
herdr agent send-keys <name> ctrl+c     # once
herdr pane read <pane> --source recent-unwrapped --lines 8
```

Check: the screen shows `Token usage: …` and `To continue this session, run: codex resume <uuid>`, then the shell prompt; `herdr agent get <name>` returns `agent_not_found`. Do not press `ctrl+c` again (it would hit the shell). On a working agent the first `ctrl+c` only interrupts the turn.

## Resume (re-review)

```bash
herdr agent start <name> --kind codex --pane <pane> --timeout 60000 -- \
  resume <session-uuid> -m <slug> -c model_reasoning_effort=<effort> --dangerously-bypass-approvals-and-sandbox
```

Verified: `agent start` returns `idle` while the screen still says `Resuming session…`; wait until the status bar shows `Context N% used` (the old context, a few seconds) before prompting. Right after a resume `agent get` does not report `agent_session` yet — and right after a fresh start not either, so do not save it then (an empty id makes `codex resume` open its picker, and the next prompt lands in the picker's search box). Get the id from the exit screen, `agent get` after the first turn, or the session file: `grep -l <worktree> ~/.codex/sessions/*/*/*/*.jsonl` (first line `session_meta` has `id` and `cwd`).

## Quota

To be recorded when first seen (the five-hour and weekly limits show on the status bar as `5h …` / `weekly N% left`).
