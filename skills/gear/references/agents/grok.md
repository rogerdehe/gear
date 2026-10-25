# Agent SOP: grok

For whoever drives a Grok agent (Grok Build) through Herdr (captain, user or the GEAR engine). After every step, read the screen and check it shows what is listed. Never type into a pane before checking who occupies it.

Tested with grok 1.0, Herdr 0.9.1, macOS. Resume and the quota messages are not yet tested.

## Facts

| Item | Value |
|---|---|
| Herdr kind | `grok` |
| Unattended flag (required) | `--always-approve` — the status bar then ends with `· always-approve` |
| Model / effort | start flags `-m <model>`, `--reasoning-effort <effort>`; `grok models` lists the models |
| Session id | choose it: `-s <uuid>` starts a new session with that id; Herdr reports it |
| Resume | `-r <session-id>` |
| Status bar | `Grok <version> │ <effort> │ <repo> │ <branch> │ <tokens> │ N% ctx` |
| Busy sign | `Ctrl+c:cancel` in the shortcut line |
| Exit | the `/exit` command; `ctrl+c` and `ctrl+d` do not exit (or close the pane) |

## Trust the directory before the first start

Grok asks "Do you trust the contents of this directory?" on the first start in a directory, while Herdr reports it `idle` — a prompt sent then is lost. GEAR trusts a worktree before it starts Grok there; by hand, append to `~/.grok/trusted_folders.toml`:

```toml
[folders."/abs/path/of/the/worktree"]
trusted = true
decided_at = <unix seconds>
```

## Start

```bash
herdr agent start <name> --kind grok --pane <pane> --timeout 60000 -- \
  --always-approve -m <model> -s <new-uuid>
```

Check: `"agent_status":"idle"`, the status bar shows `always-approve`, `agent get` shows the session id.

## Prompt, exit, resume

```bash
herdr agent prompt <name> "<text>" --wait --timeout <ms>
herdr agent prompt <name> "/exit"
herdr agent start <name> --kind grok --pane <pane> --timeout 60000 -- --always-approve -m <model> -r <uuid>
```

After `/exit` the screen shows `Resume this session with: grok --resume <uuid>`.

## Pitfalls

- **The first prompt right after a start (or after the trust dialog) can be lost**; a prompt counts as delivered only when the agent is seen `working`.
