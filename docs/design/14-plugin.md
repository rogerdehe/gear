# 14 Plugin Packaging and Runtime

The installed manifest is [herdr-plugin.toml](../../herdr-plugin.toml).
The [bin/gear](../../bin/gear) wrapper executes `target/release/gear`;
[scripts/fetch-or-build.sh](../../scripts/fetch-or-build.sh) downloads the
release binary or builds it from the Rust workspace. Installation, PATH and
board shortcuts are documented in [README](../../README.md).

- **The engine is a background process, never a pane.** It does not interact with anyone, so the user never starts it or sees it. Every `gear` call (agent or user) checks the single-instance lock next to `GEAR_DB`; if it is free it starts `gear run` detached (own session, no terminal, stdin null). The lock file holds the engine's pid; `gear admin stop` signals it (upgrade, uninstall). Revised 2026-10-22 with the user: the earlier "engine in a plugin tab, no background process" made the user run and watch a process that has nothing to show them.
- **Engine log**: `engine.log` next to the database, every line prefixed with a UTC time — the place to look when something goes wrong.
- **The only thing the user looks at is the board**: an optional plugin tab showing the requirements/tasks and their state. Closing it changes nothing.
- Skills are embedded by [skill.rs](../../crates/gear-app/src/skill.rs).
  `gear admin install-skill` writes them to the selected CLI's skill directory.
- Database location and session routing are implemented in
  [main.rs](../../crates/gear-cli/src/main.rs); `GEAR_DB` overrides the default
  `$HOME/.local/state/gear/gear.db`.

## Lifecycle boundaries

- The engine keeps running regardless of tabs, panes or clients; if it dies, the next `gear` call starts it again and the recovery checks run (08).
- Upgrade: `gear admin stop`, replace the binary; the next `gear` call starts the new one. The schema only migrates up, never down.
- Uninstall: `gear admin stop` first (Herdr does not stop processes a plugin started).
