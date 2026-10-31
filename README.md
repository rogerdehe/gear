# GEAR

**GEAR** — **G**oals **E**xecuted through **A**gent **R**elay.

GEAR runs a team of AI coding agents in [Herdr](https://github.com/herdrdev/herdr) under one captain. Lanes (long-lived developer agents) take tasks from a living plan, hand in results and keep going; reviewers on another model check each hand-in; the engine merges what passes, and the captain judges what fails and keeps the work aimed at the goal. You agree the goal once at kickoff, then watch one thing — the plan board.

**Use it for** big, long-running work with several agents and a plan that changes as you go. For anything smaller, talk to an agent directly.

> **Status:** v0.6.3, an early release for trial use — what changed: [docs/releases/v0.6.3.md](docs/releases/v0.6.3.md). Feedback is welcome — see [Feedback](#feedback).

## Requirements

- Herdr 0.9.1 or later, on macOS (Apple silicon or Intel) or Linux x86_64
- at least one agent CLI, installed and logged in: Claude Code, Codex, OpenCode or Grok (two different models are best: one to write code, a stronger one to review)
- git; your project in one git repository, or a folder of several, each with an integration branch (e.g. `dev`) pushed to a remote

## Install

1. **The plugin** (downloads a prebuilt `gear` and checks its SHA-256; no Rust needed):

   ```bash
   herdr plugin install rogerdehe/gear --ref v0.6.3
   ```

   Add `--yes` in a non-interactive shell.

2. **Put `gear` on PATH** — the agents call it by name. Add this to your shell startup file so every pane has it:

   ```bash
   export PATH="$(herdr plugin list --json | sed -n 's/.*"plugin_id":"herdr-gear","plugin_root":"\([^"]*\)".*/\1/p')/bin:$PATH"
   ```

   Check: `gear --version` prints `gear 0.6.3`. If the line above prints nothing, take `plugin_root` from `herdr plugin list --json` and add its `bin` to PATH.

3. **A key for the plan board**, in `~/.config/herdr/config.toml`, then `herdr server reload-config`:

   ```toml
   [[keys.command]]
   key = "prefix+g"
   type = "plugin_action"
   command = "herdr-gear.open-board"
   description = "GEAR plan board"
   ```

4. **The GEAR skill for every CLI your project uses** — the captain's, and every lane's and reviewer's: an agent without the skill does not know the loop (trial G06).

   ```bash
   gear admin install-skill --agent claude      # or codex, opencode, grok, pi
   gear admin install-skill --agent opencode    # once per CLI the project runs
   ```

**Upgrading from 0.2.x or 0.3.0?** The [v0.3.0 upgrade guide](docs/releases/v0.3.0.md#upgrading) has the three cases — a fresh install, an upgrade with no voyage running, an upgrade during a voyage — with the commands to copy.

**From a source checkout?** A checkout has no `gear` on PATH. Run `make install`: it builds `gear-cli` in release mode and links `$PREFIX/bin/gear` to `target/release/gear`. It is a link, not a copy, so every later `cargo build --release` is the binary the agents run. `PREFIX` defaults to `~/.local`, and `$PREFIX/bin` must be on PATH.

## Start a project

1. **Pick a short project name** (`[a-z][a-z0-9-]*`, e.g. `acme`). In a Herdr pane, in your project's repository, start the agent that will be your captain with GEAR's context and your consent:

   ```bash
   export GEAR_CTX=acme:captain:1
   export GEAR_ADMIN=1                          # lets the captain re-apply a config change
   claude                                       # or codex, opencode, grok, pi
   ```

2. **Ask the captain to kick off**, in your own words, for example:

   > Use the GEAR skill (`gear --skill captain`) to kick off a project named acme for: <your goal>.

   The captain runs `gear admin kickoff` (which agent CLIs and models this machine has), proposes lanes, models and a plan, and asks you twice: first the quality bar (fastest / deliverable / strict), then the whole kickoff sheet — answer "confirm", or type what to change. It writes `gear/gear.toml` and the first plan `gear/voyages/M1/plan.md`.

3. **The captain registers the project** from the repository, with `GEAR_ADMIN=1 gear admin init gear/gear.toml`: your confirmed kickoff sheet is the consent, and you type nothing. It checks the config first with `gear admin init --check gear/gear.toml`, which starts nothing; `init` lists every problem at once (for example a missing `gate`: your own build + test command, run before every merge). A lane or reviewer never registers a project.

4. **Tell the captain to apply the plan.** The engine starts in the background, brings up the lanes in their own Herdr worktrees and delivers their first tasks. From here on nobody asks you anything until the voyage lands.

## Day to day

- `prefix+g` opens the plan board; `q` closes it.
- The captain is rung when something needs it; the lanes, reviews and merges run by themselves.
- When a voyage lands, the captain reports it and asks whether to reclaim the lanes. Merging into your release branch is always your decision.
- If the work lives on GitHub, the captain adds a `[github]` mode to the project: issues become tasks (`gear plan import-issues`), and each task's claim, hand-in, verdict and landing are commented on its pull request. Agents never write with `gh`; the engine does. `gear --skill github` has the details — landing *through* a pull request is not in this release.
- Stop the engine (upgrade, uninstall): `gear admin stop`. Any later `gear` call starts it again.
- The engine's log: `~/.local/state/gear/engine.log`.

## Commands

| Command | Who | What it does |
|---|---|---|
| `gear` | everyone | the home page: what waits for you |
| `gear send` | lanes, reviewers | report progress, a question, a result, a review |
| `gear reply` | captain | answer an item |
| `gear plan apply` | captain | apply the voyage's plan file |
| `gear plan import-issues` | captain | print task blocks for open GitHub issues the plan does not carry yet |
| `gear report time` | captain | where the time went, per task and per voyage |
| `gear --skill [topic]` | everyone | the version-matched instructions |
| `gear board` | you | the plan, full screen |
| `gear admin kickoff / init / install-skill / stop` | you | inventory, register, install the skill, stop the engine |
| `gear --version` | everyone | the installed version |

## Feedback

Please send one feedback file per trial, written from [`FEEDBACK.md`](FEEDBACK.md) (copy it, fill it in). Each problem gets its own block: what you did, what you expected, what happened (with the exact output), and the version and environment lines the template asks for. The more exact the output, the faster it is fixed.

## Developing GEAR

GEAR is developed with GEAR. Building from source needs Rust 1.89+: `cargo build --release`, then `herdr plugin link <checkout>`. The skill texts are in `skills/gear/`.

### Tests

Run the whole suite with `cargo test --workspace` (or one crate: `cargo test -p gear-app`). The tests are offline and push nothing, by construction:

- every repository a test uses is a local bare repository in the temp dir, and every merge a test makes moves a ref on the remote's side;
- `.cargo/config.toml` names no `gh`: `GEAR_GH` points at a path that does not exist, so a test that forgets its fake fails with `no gh in tests` instead of calling GitHub;
- it allows git's file transport only (`GIT_ALLOW_PROTOCOL=file`), so a clone or fetch over https or ssh is refused;
- it keeps every git a test starts away from your global and system git config (hooks, credentials, URL rewrites), and runs with no seat identity (`GEAR_CTX` empty).

To prove it, run the same suite with outbound network denied — `sandbox-exec` on macOS, `unshare -rn` on Linux:

```bash
sh scripts/test-offline.sh
```

It builds first (that step may fetch crates), then runs the suite `--offline`, and fails if any test fails or reaches out.

A test that truly needs the network is `#[ignore]`d and runs only when asked: `GEAR_LIVE=1 cargo test -- --ignored`. There is none today.

`.cargo/config.toml` applies to `cargo run` too, and its `GEAR_GH` cannot be overridden from the shell (it is forced). To use the workspace's binary for real work, run it directly — `target/debug/gear …` — since the file only affects the processes cargo starts.
