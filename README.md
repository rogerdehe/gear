# GEAR

**GEAR** — **G**oals **E**xecuted through **A**gent **R**elay (goals carried out by a relay of agents).

GEAR runs a team of AI coding agents in [Herdr](https://github.com/herdrdev/herdr) under one captain. Lanes (long-lived developer agents) take tasks from a living plan, hand in results, and keep going; reviewers on another model check each hand-in; the captain decides the verdict and the engine merges what passes. The captain re-plans while the work runs; you watch one thing — the plan board.

**Use it for** big, long-running work with several agents and a plan that changes as you go. For anything smaller, talk to an agent directly.

> **Status:** v0.2.0 is released — prebuilt `gear` for macOS (Apple silicon, Intel) and Linux x86_64 under [Releases](https://github.com/rogerdehe/gear/releases).

## Requirements

- Herdr 0.9.1 or later, on macOS (Apple silicon or Intel) or Linux x86_64
- at least one agent CLI: OpenCode, Codex, Claude Code or Grok
- git; your project in a git repository with an integration branch (e.g. `dev`)

## Install

```bash
herdr plugin install rogerdehe/gear --ref v0.1.0
```

The plugin downloads the release binary, verifies its SHA-256, and puts it in its own directory. Rust is needed only if a matching prebuilt cannot be downloaded and the installer falls back to a source build. In a noninteractive shell, add `--yes` to approve the plugin install preview.

Put the `gear` wrapper on PATH — agents call it by name. The wrapper is in the plugin's `bin`; `herdr plugin list --json` prints its `plugin_root` (`~/.config/herdr/plugins/github/herdr-gear-<hash>` for a released plugin):

```bash
export PATH="$(herdr plugin list --json | sed -n 's/.*"plugin_id":"herdr-gear","plugin_root":"\([^"]*\)".*/\1/p')/bin:$PATH"
```

Run `command -v gear` to check that it resolves to the installed plugin’s `bin/gear`. If the extraction prints nothing, read `plugin_root` from `herdr plugin list --json` and add that directory’s `bin` to PATH. Keep this PATH in the shells that start your agents, for example by adding the export to your shell startup file.

Bind a key for the plan board in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+g"
type = "plugin_action"
command = "herdr-gear.open-board"
description = "GEAR plan board"
```

Reload Herdr’s configuration with `herdr server reload-config` after saving the binding.

## Start a project

1. **Kickoff** — see what this machine offers (agent CLIs and their models) and get a config skeleton with the TODOs to fill in. GEAR keeps its files in `gear/` in your project: the config at `gear/gear.toml`, one plan per voyage at `gear/voyages/<milestone>.md`:

   ```bash
   mkdir -p gear
   gear admin kickoff | tee gear/kickoff.txt
   sed -n '/^# gear.toml/,$p' gear/kickoff.txt > gear/gear.toml
   ```

   Kickoff prints the inventory and then a `gear.toml` skeleton; the second command extracts the TOML. Fill in the project, the captain's pane, the lanes (names, duty, owned paths), the review slots, the models (developers on a fast model, reviewers on a different, stronger one) and the quota policy (switch to the next model, or wait for the reset). Until the TODOs are filled, `gear admin init` names what is missing.

   An agent kind GEAR does not know needs a profile: `[profiles.<kind>]` sets its kind, the flags that keep it unattended, the screen markers and the exit keys — `gear admin kickoff` prints the skeleton for a kind it detects without a built-in profile.

2. **Install the captain’s instructions**, before starting its CLI:

   ```bash
   gear admin install-skill --agent claude     # or codex, opencode, grok
   ```

   This writes `SKILL.md` and its references into `~/.claude/skills/gear` (Codex: `~/.codex/skills/gear`, OpenCode: `~/.config/opencode/skills/gear`, Grok: `~/.grok/skills/gear`) and prints each path. `gear --skill captain` prints the guide embedded in the installed binary, including a complete first plan. Use that version-matched guide when running a released binary.

3. **Register it** (in your own shell inside the intended Herdr session, not inside an agent):

   ```bash
   GEAR_ADMIN=1 gear admin init gear/gear.toml
   ```

   With a filled config it registers the project and its seats. After registration, the next `gear` call (the captain's first one, below) starts the engine in the background and brings the lanes up in their own Herdr worktrees.

4. **Start the captain** — in the Herdr pane you named in `gear/gear.toml`, in your project directory, export the captain's context and start the agent you want as captain (the same pane and kind as in the config):

   ```bash
   export GEAR_CTX=acme:captain:1        # <project.id>:captain:<generation>
   claude                                 # or codex, opencode, grok
   ```

   `init` bound that pane and generation for you; the export is what makes the agent's `gear` calls carry the captain's identity. Keep `GEAR_DB` the same as in the shell that ran `init` if you changed it. Then tell the captain to read its GEAR skill (it says how to plan) and to write and apply the first plan.

   Complete your agent CLI’s first-run setup and authentication if needed. Keep it running in the configured pane: GEAR checks the live agent kind as well as the context and pane. Exporting `GEAR_CTX` in an ordinary shell alone is insufficient; plan apply reports `NOT_REGISTERED` if the pane hosts no agent.

5. **Quality bar** — agree one with your captain at kickoff; it goes into the milestone's `bar:` and is never raised mid-voyage:

   | Bar | For | What blocks | Rework |
   |---|---|---|---|
   | **FASTEST** | an MVP or a skeleton, iterations of a few hours | only what breaks the chain or loses data; the rest is logged | at most one round |
   | **DELIVERABLE** | a mature project | errors in normal use are fixed; edge cases are logged | capped; the gate is the project's own checks |
   | **STRICT** | e.g. finance | severe issues must be fixed; medium ones are the captain's call | more rounds, each approved by the captain; more time and cost agreed up front |

6. **Plan** — the captain writes the voyage's plan file and applies it:

   ```bash
   gear plan apply gear/voyages/M1.md
   ```

   After a successful apply GEAR rewrites that same file in place (`base=` becomes the applied version, each `## NEW` block gets its id, the `# Verdicts` section goes), so the file is always the current plan and the captain keeps editing it. A landed voyage's file is kept as its record; the next voyage starts a new `gear/voyages/<milestone>.md`. The lanes start working at once.

   Success prints `plan version 1` and the created task ids. The plan format is described by `gear --skill captain`.

## Day to day

- `prefix+g` opens the plan board; `q` closes it.
- The captain is rung when something needs it; it runs `gear`, answers with `gear reply`, and changes the plan with `gear plan apply`.
- Reviews run in their own sessions on another model; the captain decides each verdict and the engine merges what passes.
- Stop the engine (upgrade, uninstall): `gear admin stop`. Any later `gear` call starts it again.
- Something wrong? Read `engine.log` next to the store (`~/.local/state/gear/` by default).

## Commands

| Command | Who | What it does |
|---|---|---|
| `gear` | everyone | your home page: what waits for you |
| `gear send` | lanes, captain | report progress, a question, a block, a result |
| `gear reply` | captain | answer messages |
| `gear plan apply` | captain | apply a plan draft |
| `gear report time` | captain | where the time went: per task and per voyage |
| `gear board` | you | the plan, full screen |
| `gear admin kickoff / init / install-skill / stop` | you | set up, register, install the skill, stop |
| `gear --version` | everyone | which version of GEAR this is |

The v0.1.0 install and first plan apply were exercised on macOS arm64 with Herdr 0.9.1, in an isolated configuration without Rust on PATH. The step outputs and limits are recorded in [the install report](ops/reports/T-48.md).

## Developing GEAR

GEAR is developed with GEAR. Building from source needs Rust 1.89+: `cargo build --release`, then `herdr plugin link <checkout>`. The product plan is `docs/PRD.md`; the working conventions are `docs/design/17-conventions.md`.
