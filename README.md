# GEAR

**GEAR** — **G**oals **E**xecuted through **A**gent **R**elay (goals carried out by a relay of agents).

GEAR runs a team of AI coding agents in [Herdr](https://github.com/herdrdev/herdr) under one captain. Lanes (long-lived developer agents) take tasks from a living plan, hand in results, and keep going; reviewers on another model check each hand-in; the captain decides the verdict and the engine merges what passes. The captain re-plans while the work runs; you watch one thing — the plan board.

**Use it for** big, long-running work with several agents and a plan that changes as you go. For anything smaller, talk to an agent directly.

> **Status:** v0.1.0 is being prepared. The prebuilt release is published with it — until the v0.1.0 release appears under [Releases](https://github.com/rogerdehe/gear/releases), the steps below do not work yet.

## Requirements

- Herdr 0.9.1 or later, on macOS (Apple silicon or Intel) or Linux x86_64
- at least one agent CLI: OpenCode, Codex, Claude Code or Grok
- git; your project in a git repository with an integration branch (e.g. `dev`)

## Install

```bash
herdr plugin install rogerdehe/gear
```

*Not verifiable here:* there is no v0.1.0 release yet (this machine links the checkout with `herdr plugin link`). The plugin downloads a prebuilt `gear` and puts it in its own directory.

Put the `gear` wrapper on PATH — agents call it by name. The wrapper is in the plugin's `bin`; `herdr plugin list --json` prints its `plugin_root` (`~/.config/herdr/plugins/github/herdr-gear-<hash>` for a released plugin):

```bash
export PATH="$(herdr plugin list --json | sed -n 's/.*"plugin_id":"herdr-gear","plugin_root":"\([^"]*\)".*/\1/p')/bin:$PATH"
```

*Verified (macOS, Herdr 0.9.1):* the command prints the plugin root on this machine (here `/Volumes/Data/myai/gear`, a local link); if it prints nothing, take `plugin_root` from `herdr plugin list --json` and use `<plugin_root>/bin`.

Bind a key for the plan board in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+g"
type = "plugin_action"
command = "herdr-gear.open-board"
description = "GEAR plan board"
```

*Verified:* this machine's config has exactly this block, and `herdr plugin list` shows the `herdr-gear.open-board` action.

## Start a project

1. **Kickoff** — see what this machine offers (agent CLIs and their models) and get a config skeleton with the TODOs to fill in:

   ```bash
   gear admin kickoff | sed -n '/^# gear.toml/,$p' > gear.toml
   ```

   *Verified:* the kickoff prints the inventory and then a `gear.toml` skeleton; the extracted skeleton is 36 lines of TOML. Fill in the project, the captain's pane, the lanes (names, duty, owned paths), the review slots, the models (developers on a fast model, reviewers on a different, stronger one) and the quota policy (switch to the next model, or wait for the reset). Until the TODOs are filled, `gear admin init` names what is missing.

2. **Register it** (in your own shell, not inside an agent):

   ```bash
   GEAR_ADMIN=1 gear admin init gear.toml
   ```

   *Verified:* with a filled config it registers the project and its seats. The engine starts by itself in the background and brings the lanes up in their own Herdr worktrees.

3. **Quality bar** — agree one with your captain at kickoff; it goes into the milestone's `bar:` and is never raised mid-voyage:

   | Bar | For | What blocks | Rework |
   |---|---|---|---|
   | **FASTEST** | an MVP or a skeleton, iterations of a few hours | only what breaks the chain or loses data; the rest is logged | at most one round |
   | **DELIVERABLE** | a mature project | errors in normal use are fixed; edge cases are logged | capped; the gate is the project's own build/test command |
   | **STRICT** | e.g. finance | severe issues must be fixed; medium ones are the captain's call | more rounds, each approved by the captain; more time and cost agreed up front |

4. **Give the captain its instructions** — install the GEAR skill where its CLI loads skills:

   ```bash
   gear admin install-skill --agent claude     # or codex, opencode, grok
   ```

   *Verified:* it writes `SKILL.md` and the references into `~/.claude/skills/gear` (Codex: `~/.codex/skills/gear`, OpenCode: `~/.config/opencode/skills/gear`, Grok: `~/.grok/skills/gear`) and prints every path. `gear --skill` prints the same text from the binary.

5. **Plan** — the captain writes a plan draft and applies it:

   ```bash
   gear plan apply plan.md
   ```

   *Verified:* a plan written from the captain guide applies on a fresh project (it creates the tasks and the milestone). The lanes start working at once.

## Day to day

- `prefix+g` opens the plan board; `q` closes it. (*Verified:* the key is bound and the action is registered; the key press itself is not testable from here.)
- The captain is rung when something needs it; it runs `gear`, answers with `gear reply`, and changes the plan with `gear plan apply`.
- Reviews run in their own sessions on another model; the captain decides each verdict and the engine merges what passes.
- Stop the engine (upgrade, uninstall): `gear admin stop`. Any later `gear` call starts it again. (*Not verifiable here:* it would stop this machine's live engine.)
- Something wrong? Read `engine.log` next to the store (`~/.local/state/gear/` by default). (*Verified:* the file is there.)

## Commands

| Command | Who | What it does |
|---|---|---|
| `gear` | everyone | your home page: what waits for you |
| `gear send` | lanes, captain | report progress, a question, a block, a result |
| `gear reply` | captain | answer messages |
| `gear plan apply` | captain | apply a plan draft |
| `gear board` | you | the plan, full screen |
| `gear admin kickoff / init / install-skill / stop` | you | set up, register, install the skill, stop |

## Developing GEAR

GEAR is developed with GEAR. Building from source needs Rust 1.89+: `cargo build --release`, then `herdr plugin link <checkout>`. The product plan is `docs/PRD.md`; the working conventions are `docs/design/17-conventions.md`.
