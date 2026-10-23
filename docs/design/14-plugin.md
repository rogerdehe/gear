# 14 Plugin Packaging and Runtime

Files: `herdr-plugin.toml`, `bin/gear`, `cli/admin.py` (run). Budget 50–70 lines.

## Manifest

```toml
id = "herdr-gear"
name = "GEAR"
version = "0.1.0"
min_herdr_version = "0.9.1"
description = "Keep agent fleets working under one captain"
platforms = ["linux", "macos"]

[[panes]]
id = "board"
title = "GEAR board"
placement = "tab"
command = ["bin/gear", "board"]

[[actions]]
id = "open"
title = "Open GEAR board"
contexts = ["workspace"]
command = ["herdr", "plugin", "pane", "open", "--plugin", "herdr-gear", "--entrypoint", "board"]
```

- **The engine is a background process, never a pane.** It does not interact with anyone, so the user never starts it or sees it. Every `gear` call (agent or user) checks the single-instance lock next to `GEAR_DB`; if it is free it starts `gear run` detached (own session, no terminal, stdin null). The lock file holds the engine's pid; `gear admin stop` signals it (upgrade, uninstall). Revised 2026-10-22 with the user: the earlier "engine in a plugin tab, no background process" made the user run and watch a process that has nothing to show them.
- **Engine log**: `engine.log` next to the database, every line prefixed with a UTC time — the place to look when something goes wrong.
- **The only thing the user looks at is the board**: an optional plugin tab showing the requirements/tasks and their state. Closing it changes nothing.
- No `startup`, no `events` (MVP).
- The package ships `skills/gear/` (16); install instructions include `npx skills add … --skill gear -g`.
- `bin/gear`: `exec python3 -m gear "$@"`, and adds the plugin root to `PYTHONPATH`. The `gear` invoked by Agents is made available by linking `bin/gear` onto PATH at install time (one step in the install instructions), or by writing the absolute path in the task packet.
- State: the engine uses `HERDR_PLUGIN_STATE_DIR`; onboarded Agents find the same database through `GEAR_DB`, set at onboarding (03). User ops commands locate it via `~/.config/gear/targets.toml` (execution host/session → database path), recorded at install time, and check that the database's `meta.engine_session` matches the current Herdr session, rejecting on mismatch. In the MVP, each execution host binds one Herdr session to one database; it does not guess from UI focus.

## Lifecycle boundaries

- The engine keeps running regardless of tabs, panes or clients; if it dies, the next `gear` call starts it again and the recovery checks run (08).
- Upgrade: `gear admin stop`, replace the binary; the next `gear` call starts the new one. The schema only migrates up, never down.
- Uninstall: `gear admin stop` first (Herdr does not stop processes a plugin started).

## Pending probes

Single-instance and remote behavior of the plugin pane on 0.9.1; display of `placement="tab"`; PATH of `gear` inside Agent panes.
