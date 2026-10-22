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
id = "engine"
title = "GEAR"
placement = "tab"
command = ["bin/gear", "run"]

[[actions]]
id = "open"
title = "Open GEAR"
contexts = ["workspace"]
command = ["herdr", "plugin", "pane", "open", "--plugin", "herdr-gear", "--entrypoint", "engine"]
```

- No `startup`, no `events` (MVP); no nohup background process.
- The package ships `skills/gear/` (16); install instructions include `npx skills add … --skill gear -g`.
- The engine runs in the plugin tab; when opened again, the second instance cannot get the lock, prints the location of the existing instance and exits (08).
- `bin/gear`: `exec python3 -m gear "$@"`, and adds the plugin root to `PYTHONPATH`. The `gear` invoked by Agents is made available by linking `bin/gear` onto PATH at install time (one step in the install instructions), or by writing the absolute path in the task packet.
- State: the engine uses `HERDR_PLUGIN_STATE_DIR`; onboarded Agents find the same database through `GEAR_DB`, set at onboarding (03). User ops commands locate it via `~/.config/gear/targets.toml` (execution host/session → database path), recorded at install time, and check that the database's `meta.engine_session` matches the current Herdr session, rejecting on mismatch. In the MVP, each execution host binds one Herdr session to one database; it does not guess from UI focus.

## Lifecycle boundaries

- Switching tabs, hiding, local remote client disconnecting: the engine keeps running.
- Closing the plugin tab: the engine stops; `gear send` can still write to the database; after reopening, recovery checks run (08).
- Upgrade: stop the engine first (close the tab), replace the code, reopen; the schema only migrates up, never down.

## Pending probes

Single-instance and remote behavior of the plugin pane on 0.9.1; display of `placement="tab"`; PATH of `gear` inside Agent panes.
