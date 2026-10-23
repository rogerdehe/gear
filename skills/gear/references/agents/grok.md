# Agent SOP: grok

For whoever drives a Grok agent (Grok Build) through Herdr (captain, user, or the GEAR engine). Every step: run the command, then check the screen shows what is listed before going on. Never type into a pane before checking who occupies it.

Verified on: grok 1.0.62, Herdr 0.9.1 (protocol 22), macOS, 2026-10-23.

## Facts

| Item | Value |
|---|---|
| Herdr kind | `grok` |
| Unattended flag (mandatory) | `--always-approve` — the status bar then ends with `· always-approve` |
| Model / effort | start flags `-m <model>`, `--reasoning-effort <effort>`; `grok models` lists the models |
| Session id | choose it: `-s <uuid>` starts a NEW session with that id; Herdr reports it as `agent_session` |
| Resume | `-r <session-id>` (`--resume`) |
| Busy sign | `Ctrl+c:cancel` in the bottom shortcut line while a turn runs; Herdr's working/idle is correct |
| Status bar | `Grok 4.7 │ medium │ <repo> │ <branch> │ 22k/256k │ 9% ctx` |
| Exit | the `/exit` command (sent as a prompt); `ctrl+c` and `ctrl+d` do not exit |
| Trust store | `~/.grok/trusted_folders.toml` |

## Before the first start in a directory: trust it

Grok asks "Do you trust the contents of this directory?" the first time it starts in a directory, **while Herdr reports the agent `idle`** — a prompt sent then is lost. Trust the worktree before starting:

```toml
# ~/.grok/trusted_folders.toml
[folders."/abs/path/of/the/worktree"]
trusted = true
decided_at = <unix seconds>
```

(Or answer `y` in the dialog once.)

## Start

```bash
herdr agent start <name> --kind grok --pane <pane> --timeout 60000 -- \
  --always-approve -m <model> -s <new-uuid>
```

Check: `"agent_status":"idle"`, the status bar shows `always-approve`, `agent get` shows `agent_session.value` = the uuid. The first prompt right after the start (or right after the trust dialog) can be lost: send it with `--wait` and check the session has the message (`grok export <uuid>` fails with "no conversation content" if it did not arrive).

## Prompt

```bash
herdr agent prompt <name> "<text>" --wait --timeout <ms>
```

## Exit

Only when the agent is `idle`/`done`:

```bash
herdr agent prompt <name> "/exit"
```

Check: the screen shows `Resume this session with: grok --resume <uuid>` and the shell prompt; `herdr agent get <name>` returns `agent_not_found` (it may take a second).

## Resume

```bash
herdr agent start <name> --kind grok --pane <pane> --timeout 60000 -- --always-approve -m <model> -r <uuid>
```

Not yet verified live: resume, the quota/limit messages.
