# GEAR

GEAR keeps a fleet of AI agents working under one captain: lanes take tasks
from a living plan, hand in results, and the captain decides. The user looks
at one thing — the plan.

This repository is GEAR itself: the `gear` command, the engine that runs in
the background, and the store they share. The design is in `docs/` (the
product plan is `docs/PRD.md`, the working conventions are
`docs/design/17-conventions.md`).

## Install as a Herdr plugin

The plugin adds the plan board to Herdr: a pane that shows the plan of the
project, and a key that opens it. The board is a view, not a process to keep
open — `q` leaves it.

### 1. Build the binary

```bash
cargo build --release
```

Herdr builds a plugin it *installs* from GitHub, but not one you *link*: for
a linked plugin the working tree is built by you. The binary lands in
`target/release/gear`.

### 2. Link the plugin

```bash
herdr plugin link /path/to/gear
```

The plugin id is `herdr-gear`. Check that Herdr sees it:

```bash
herdr plugin list
herdr plugin action list --plugin herdr-gear
```

### 3. Bind a key

Add this to `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+b"
type = "plugin_action"
command = "herdr-gear.open-board"
description = "open the GEAR board in a split"

[[keys.command]]
key = "prefix+shift+b"
type = "plugin_action"
command = "herdr-gear.open-board-tab"
description = "open the GEAR board in a tab"
```

Then reload the config and press the key:

```bash
herdr server reload-config
```

### 4. Open the board without a key

The two actions are the ones the keys are bound to:

```bash
herdr plugin action invoke herdr-gear.open-board        # a split beside the current work
herdr plugin action invoke herdr-gear.open-board-tab    # its own tab
```

### 5. Put `gear` on PATH for the agents

The agents call `gear` by name, so the plugin's wrapper has to be reachable:

```bash
export PATH="/path/to/gear/bin:$PATH"
```

(or link `bin/gear` into a directory that is already on PATH). The wrapper
runs the release binary from any working directory and finds the store:
`GEAR_DB` when it is set, otherwise `~/.local/state/gear/gear.db`.

## The commands

| Command | Who | What it does |
|---|---|---|
| `gear` | every seat | the home page: what waits for you, what you have not seen |
| `gear send` | lanes, the captain | report progress, a question, a block, a result |
| `gear reply` | the captain | answer exact message ids |
| `gear plan apply` | the captain | apply a plan draft |
| `gear board` | the user | the plan, full screen; `q` leaves it |
| `gear run` | the user | the engine in the foreground (normally started for you) |
| `gear admin …` | the user | project, seat and captain administration |

Every `gear` call starts the engine in the background when it is not running
already, so nobody has to watch it. The engine writes `engine.log` next to
the database.
