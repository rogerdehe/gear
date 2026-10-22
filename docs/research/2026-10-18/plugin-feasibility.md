# Herdr Plugin Mechanism Verification and Minimal Implementation Boundary

Conclusion: a plugin can host this product's entry point, terminal board, CLI, and its own SQLite; Herdr does not provide daemon auto-restart, task persistence, or automatic skill installation. The resident control loop must be implemented by the plugin, but there is no need to deploy a network service or require systemd.

**Evidence scope**

Local `herdr --version` is 0.9.1. I read the official v0.9.1 source (commit `065ef9d6a531c49fb8bee7e818ef837065b21ee9`), the 0.9.1 docs kept in the repository, a newer pinned snapshot `143bf5fd4781d164260c63fe28e7dc08054ad32f`, and pinned versions of the plugins below. I looked at test assertions but did not run these upstream tests, did not install a probe plugin, and did not connect to or modify the user's remote sessions. Earlier, opening a local File Viewer and moving a tab actually succeeded; that cannot be counted as remote/recovery acceptance for this plugin.

**What the interface actually supports**

| Requirement | Verification result and implementation constraint |
|---|---|
| Plugin entry point | The manifest declares actions, panes, events, startup, build, link_handlers; commands are argv, and the plugin chooses its own language. It is not a business module loaded inside Herdr |
| Board | A pane can open in a tab and run its own terminal program; there is no native non-terminal UI extension. The board must be written by us |
| SQLite | Herdr injects `HERDR_PLUGIN_STATE_DIR`, and the plugin creates its own database; there is no Herdr-managed storage API. Configuration uses CONFIG_DIR and must not write to the install ROOT |
| Calling Herdr | Use the injected HERDR_BIN_PATH and socket context to call the public CLI/API; the runtime cwd defaults to the plugin directory, and the project cwd is resolved from the invocation context and confirmed |
| Background residency | startup is one-time initialization, not a supervisor; linking/enabling the plugin or a client connecting does not re-run startup automatically. An action starting successfully does not mean the task is complete |
| hooks | Usable as quick notifications, not a substitute for persistent messages; the 0.9.1 runtime has a limit of at most 32 concurrent plugin commands, with no promise of automatic retry or persistent delivery |
| CLI and skill | The manifest has no general skill-installation or CLI PATH registration fields. Files can be shipped in the package and each Agent configured explicitly; the task packet provides the exact CLI path, and we do not assume every Agent auto-discovers skills |
| remote | The plugin and program are installed on the target execution host. The plugin pane runs on that host and the client displays the terminal; the database is not opened across the network. Local custom keybindings are not automatically forwarded as remote plugin actions; use entry points/keybinding config on the target host |
| Lifecycle | Install, disable, and uninstall are not the same as starting or stopping the background process. The newer install-directory cleanup mechanism also requires watching for live background processes that reference an old checkout; upgrades need an explicit stop/switch strategy |

**Is there an existing plugin that does this**

There are clear precedents for "plugin + resident process", but this must not be described as service hosting built into Herdr:

- **Herdr Board 0.19.1**: the manifest's pane runs `board tui`, and the comments and CLI implementation explicitly auto-start boardd. It provides daemon start/stop/status; the operations doc states explicitly that uninstalling does not stop the background process. build installs the CLI separately. It proves a plugin can carry its own resident engine; it does not prove Herdr manages that engine automatically.
- **Herdr Dispatch 0.10.7**: startup calls `scripts/start.sh`, which starts the background process with `nohup ... hdis daemon &`, and provides stop/restart actions. The manifest states there is no shutdown hook, so the background process may keep running after disable/uninstall.
- **Herdr Tasks**: start.sh likewise starts it via `nohup ... htask daemon &`. No web service or systemd deployment is needed.
- **File Viewer**: `[[panes]]` runs a long-lived TUI and provides an action to open it; it is a precedent for an ordinary process hosted by a Herdr pane. It is not a task supervisor.

Among the plugins checked this time, none was found with an identical business implementation of "dedicated controller tab + separate board". This approach is a design choice built on ordinary Herdr plugin panes; it must undergo hands-on lifecycle probes, and we cannot claim an existing plugin has validated the whole approach. We also do not copy nohup directly: its uninstall leftovers, surviving old versions, and the newer directory-cleanup constraints all have costs.

**This project's minimal choice (revised under the single-process constraint)**

Board and Dispatch serve only as R&D references: learn task/attempt and local state from the former, and start/stop and dedup from the latter; they are not installed as dependencies, multiple daemons are not stitched together, and their databases are not shared. The description above covers those plugins' own implementations, not this project's chosen architecture.

The product is a Herdr plugin that runs one `gear run` process in one tab, internally combining the board, scheduling, automatic nudging, mailbox, SQLite, and Unix socket request handling. CLI requests and Herdr/Git/test commands may run briefly; no second resident UI, service, or watchdog is added. The actual working Agents are not counted as extra resident processes of the plugin.

Switching away from the tab, hiding a view within the plugin, or disconnecting the local remote client should not stop the remote loop. Closing the only plugin tab does stop the process; there is no separate background process to continue. After an unexpected termination, recovery happens on the next open; we do not pretend it can restart itself. The earlier "separate controller tab + board tab" approach is withdrawn.

**Four probes before starting product code**

1. Link a minimal plugin in an isolated Herdr session: an action opens the single running tab; repeated opens only focus the same process; record the correct cwd, socket, and state dir.
2. The same process simultaneously renders a table, handles requests, and periodically writes SQLite; no keyboard input, sitting on a detail page, or a hidden view must not block the counter. Simulate slow calls; the UI and other requests must stay responsive.
3. Install the same probe on the target host and open it via local Herdr remote; after switching tabs and disconnecting/reconnecting, the counter continues, all files stay on the target host, and the process list shows only one resident plugin process.
4. Kill the plugin tab and reopen it, recording that it recovers without double-launching; check the disable/uninstall and version-change boundaries: no old engine silently running, no database deleted. Stop explicitly before uninstalling; do not assume a shutdown hook exists.

These four have not been executed yet; they verify the single-process plugin hosting mechanism. Once they pass, do the real Agent continuation loop.

**Verifiable sources**

- [Official plugin docs (0.9.1 versioned docs)](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/docs/versions/0.9.1/website/src/content/docs/plugins.mdx)
- [v0.9.1 manifest schema](https://github.com/herdrdev/herdr/blob/065ef9d6a531c49fb8bee7e818ef837065b21ee9/src/api/schema/plugins.rs), [command and startup implementation](https://github.com/herdrdev/herdr/blob/065ef9d6a531c49fb8bee7e818ef837065b21ee9/src/app/api/plugins/runtime.rs), [panes and environment](https://github.com/herdrdev/herdr/blob/065ef9d6a531c49fb8bee7e818ef837065b21ee9/src/app/api/plugins/panes.rs)
- [v0.9.1 remote keybinding tests](https://github.com/herdrdev/herdr/blob/065ef9d6a531c49fb8bee7e818ef837065b21ee9/src/client/shell/tests/keybindings_settings.rs)
- [Board manifest](https://github.com/nelsonPires5/herdr-board/blob/7a215a179acb4e098dd1c2971332497806309360/herdr-plugin.toml), [Board uninstall and background lifecycle](https://github.com/nelsonPires5/herdr-board/blob/7a215a179acb4e098dd1c2971332497806309360/docs/operations.md)
- [Dispatch manifest](https://github.com/husniadil/herdr-dispatch/blob/852f94150a5159b261ca0e932aa126fb1f24b7ff/herdr-plugin.toml), [Dispatch start script](https://github.com/husniadil/herdr-dispatch/blob/852f94150a5159b261ca0e932aa126fb1f24b7ff/scripts/start.sh)
- [Tasks start script](https://github.com/husniadil/herdr-tasks/blob/305c9ac978d3cdbafb07f15f031b9295a9a84be5/scripts/start.sh)
- [Install-directory and detached-process constraints in newer plugin docs](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/docs/versions/0.9.5/website/src/content/docs/plugins.mdx)
