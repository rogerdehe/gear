# GEAR

**GEAR** — **G**oals **E**xecuted through **A**gent **R**elay (goals carried out by a relay of agents).

GEAR runs a team of AI coding agents in [Herdr](https://github.com/herdrdev/herdr) under one captain. Lanes (long-lived developer agents) take tasks from a living plan, hand in results, and keep going; reviewers on another model check each hand-in; the engine merges what passes and sends back what does not. The captain re-plans while the work runs; you watch one thing — the plan board.

**Use it for** big, long-running work with several agents and a plan that changes as you go. For anything smaller, talk to an agent directly.

> **Status:** v0.1.0 is being prepared. The prebuilt release (no Rust needed) is published with it — until the v0.1.0 release appears under [Releases](https://github.com/rogerdehe/gear/releases), the steps below do not work yet.

## Requirements

- Herdr 0.9.1 or later, on macOS (Apple silicon or Intel) or Linux x86_64
- at least one agent CLI: OpenCode, Codex or Claude Code
- git; your project in a git repository with an integration branch (e.g. `dev`)

You do **not** need Rust: the plugin downloads a prebuilt `gear`.

## Install

```bash
herdr plugin install rogerdehe/gear
```

Put the `gear` command on PATH (the agents call it by name):

```bash
export PATH="$(herdr plugin config-dir herdr-gear)/../../github/rogerdehe/gear/bin:$PATH"   # adjust to where Herdr installed the plugin
```

Bind a key for the plan board in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+g"
type = "plugin_action"
command = "herdr-gear.open-board"
description = "GEAR plan board"
```

## Start a project

1. **Kickoff** — see what this machine offers and get a config draft:

   ```bash
   gear admin kickoff > gear.toml
   ```

   Edit `gear.toml`: the project, the captain's pane, the lanes (`<project>-<duty>` names, what each owns), the review slots, the models (developers on a fast model, reviewers on a different, stronger one) and what happens when a quota runs out (switch to the next model, or wait for the reset).

2. **Register it** (in your own shell, not inside an agent):

   ```bash
   GEAR_ADMIN=1 gear admin init gear.toml
   ```

   The engine starts by itself in the background and brings the lanes up in their own Herdr worktrees.

3. **Give the captain its instructions** — the captain agent reads the GEAR skill:

   ```bash
   gear --skill            # print it; install it as a skill for your captain's CLI
   ```

4. **Plan** — the captain writes a plan draft and applies it: `gear plan apply plan.md`. Lanes start working at once.

## Day to day

- `prefix+g` opens the plan board; `q` closes it.
- The captain is rung when something needs it; it runs `gear`, answers with `gear reply`, and changes the plan with `gear plan apply`.
- Reviews, verdicts and merges run by themselves.
- Stop the engine (upgrade, uninstall): `gear admin stop`. Any later `gear` call starts it again.
- Something wrong? Read `engine.log` next to the store (`~/.local/state/gear/` by default).

## Commands

| Command | Who | What it does |
|---|---|---|
| `gear` | everyone | your home page: what waits for you |
| `gear send` | lanes, captain | report progress, a question, a block, a result |
| `gear reply` | captain | answer messages |
| `gear plan apply` | captain | apply a plan draft |
| `gear board` | you | the plan, full screen |
| `gear admin kickoff / init / stop` | you | set up, register, stop |

## Developing GEAR

GEAR is developed with GEAR. Building from source needs Rust 1.89+: `cargo build --release`, then `herdr plugin link <checkout>`. The product plan is `docs/PRD.md`; the working conventions are `docs/design/17-conventions.md`.
