# Agent SOP: claude

For whoever drives a Claude Code agent through Herdr (captain, user, or the GEAR engine). Every step: run the command, then check the screen shows what is listed before going on. Never type into a pane before checking who occupies it.

Verified on: Claude Code 2.1.308, Herdr 0.9.1 (protocol 22), macOS, 2026-10-23 (the GEAR captain runs here).

## Facts

| Item | Value |
|---|---|
| Herdr kind | `claude` |
| Unattended flag (mandatory) | `--dangerously-skip-permissions` — the status line then shows `⏵⏵ bypass permissions on` |
| Model | start flag `--model <name>` (an alias such as `opus`, `sonnet`, `fable`, or a full model name) |
| Effort | start flag `--effort <low\|medium\|high\|xhigh\|max>` |
| Permission mode | start flag `--permission-mode <acceptEdits\|auto\|bypassPermissions\|manual\|dontAsk\|plan>` (the unattended flag above is the one GEAR uses) |
| Herdr reports `agent_session` | yes — `agent get` → `agent_session.value` (the session uuid, e.g. `0f9dc6bd-…`) |
| Resume | `claude --resume <session-id>` |
| Busy sign | `esc to interrupt`; the spinner line above the input shows the running turn (`✻ Orbiting… (2m 10s · ↓ 1.1k tokens)`) |
| Status bar (two lines, bottom) | `Opus 5.5 (1M context) · medium /path/to/repo (dev)  ·  ctx 74%  ·  5h 16% ↻06:10  ·  7d 68% ↻Oct23` and `⏵⏵ bypass permissions on · 1 shell` |
| Exit | two `ctrl+c`: the first interrupts a running turn, the second leaves the session |

## Before anything: check the pane

```bash
herdr agent get <name-or-pane>        # agent kind + agent_status
herdr pane read <pane> --source visible --lines 30
```

- An agent is present → do not use `pane run` / `pane send-text` for shell commands; anything typed goes into the agent's input as a prompt.
- Only act on an agent whose status is `idle` or `done`.

## Start

The pane must show the shell prompt of the agent's worktree; a pane that Herdr just made is not ready for a few seconds (shell init) and `agent start` then returns `agent_pane_busy` — wait for the prompt, then start.

```bash
herdr agent start <name> --kind claude --pane <pane> --timeout 60000 -- \
  --dangerously-skip-permissions
```

Check: the reply contains `"agent_status":"idle"` (about 5 s) and the status bar shows the model and `bypass permissions on`. To pin a model or an effort, add `--model <name>` / `--effort <level>` to the same command line.

## Prompt

```bash
herdr agent prompt <name> "<text>" --wait --timeout <ms>
```

A long first turn makes `--wait` time out; that does not mean the prompt was lost — check `agent get` shows `working`.

## Find the session id

Herdr reports it: `herdr agent get <name>` → `agent_session.value`. Save it when the agent goes idle after its first turn; a fresh start does not report the session until then. `claude --resume` without an id opens an interactive picker — do not use that to resume.

## Exit

Only when the agent is `idle`/`done`:

```bash
herdr agent send-keys <name> ctrl+c     # once, then a second time to leave
herdr pane read <pane> --source recent-unwrapped --lines 8
```

Check: the screen shows the session summary and the shell prompt, and `herdr agent get <name>` returns `agent_not_found`. On a working agent the first `ctrl+c` only interrupts the turn; the second leaves the session. Do not keep pressing `ctrl+c` after the agent is gone — the keys would hit the shell.

## Resume (re-review, new session in the same seat)

```bash
herdr agent start <name> --kind claude --pane <pane> --timeout 60000 -- \
  --dangerously-skip-permissions --resume <session-uuid>
```

Check: `agent_started` and the status bar shows the old context (`ctx N%`) — that proves the old conversation is loaded. Right after a resume `agent get` may not report `agent_session` yet; take the id from the pane or from the previous `agent get`, not from the resume.

## Switch model or effort

The start flags are the reliable way (`--model`, `--effort`): a resume with different flags changes the model of the resumed session. Inside a running session Claude Code has `/model` and `/effort` commands; they are **not verified in Herdr** on this machine (2026-10-23), so treat them as unproven and prefer starting with the flag.

## Pitfalls

- **Permissions.** Without the unattended flag Claude Code stops on a permission question; Herdr then reports the agent as blocked, and the turn waits for a human. Start it with `--dangerously-skip-permissions` (or `--permission-mode bypassPermissions`) and check the status bar says `bypass permissions on`.
- **The first prompt right after `agent start` can be lost.** `agent start` returns once Herdr sees the TUI, but the input may not take a prompt yet: `agent prompt --wait` then reports a stalled prompt. Wait until the status bar is drawn, then prompt again; a prompt counts as delivered only when the agent is seen `working`.
- **A rate limit looks like a finished turn.** The status bar carries the limits (`5h 16% ↻06:10`, `7d 68% ↻Oct23`): the percentage and the arrow give the reset. These percentages are usage, not remaining capacity: `5h 1%` means 1% used; `5h 100%` is exhausted. The engine reads only the actual footer region, never quoted quota words or bars in the conversation. During a captain outage, current work/reviews/merges continue and new tasks pause; after the reset wake, run `gear` to resume assignments. Read the screen before treating a stop as completion.
- **One `ctrl+c` is not an exit.** On a working turn it only interrupts; a second one leaves the session.
- **A session belongs to its directory.** `claude --resume <id>` must be started in the directory the session was created in; the status line shows that directory and the branch in parentheses.
- **Queued input.** `ctrl+enter` sends the queued message immediately; the input box shows `Press up to edit queued messages`. Read the screen before typing again.
