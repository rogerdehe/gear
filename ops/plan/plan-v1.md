<!-- gear-plan project=gear base=0 -->
# Goal
GEAR develops itself: the parallel develop–review–merge loop runs inside the engine, the plan board opens on a Herdr hotkey, and kickoff/quota policy come from the project config (PRD v0.9 §0, §8a, §10, §15, §16). Done when the gear project runs on GEAR with no ops/ stopgap scripts (ops/STOPGAPS.md empty).

# Milestone M1: Self-hosting loop
Review loop in the engine, model per agent, plan board, plugin install. Out of scope: dependency graph, multi-host, Codex/Claude lanes.

# Decisions
- 2026-10-22 Kickoff with the user: lanes gear-runtime / gear-data / gear-board (OpenCode --auto), 3 review temps; dev MiniMax-M3.1-Flash -> DeepSeek flash; review MiniMax-M3 -> DeepSeek V4 Pro, never the writer's model; on quota: fallback, then wait for the 5 h reset.
- 2026-10-22 Until T-3 lands the captain starts reviewers by hand (ops/bin/start-reviewer.sh) and merges with ops/bin/merge-task.sh.
- 2026-10-22 Skeleton rule: smallest change that makes the loop run; no robustness polishing; P3 findings are logged, not turned into tasks.

# Queue
## NEW [must] lane=runtime milestone=M1 required
goal: An agent GEAR starts runs on the model GEAR chose. OpenCode has no --model flag; find the supported way (try env OPENCODE_CONFIG_CONTENT='{"model":"<provider/model>"}' exported in the pane before `opencode --auto`, or a per-worktree opencode.json) and verify it live on one pane. Then: `onboard_seat` (crates/gear-engine/src/onboard.rs) puts the first model of the lane's chain (`[models].lanes`, or the lane's own `models`) into the environment line. Record what you verified in skills/gear/references/agents/opencode.md (one short section).
accept: cargo test --workspace green; a unit test shows the env line carries the model; the reference file states the verified mechanism with the OpenCode version.
deps: -

## NEW [must] lane=data milestone=M1 required
goal: A reviewer can hand in a verdict. Add `gear send --kind review --task T-n --attempt <a> --verdict pass|fail "<findings path or text>"`, allowed only for a temp seat bound to that task (role Temp). It stores the verdict on the attempt's review record (new store table or columns; your choice, keep it small) and puts an action item in the captain's inbox. It does not settle the task (the engine does that in a later task). Update skills/gear/SKILL.md §3 with one line for reviewers.
accept: cargo test --workspace green; tests: a temp seat's review is stored and visible to the captain; a lane seat or a wrong task is FORBIDDEN/NO_TARGET.
deps: -

## NEW [must] lane=board milestone=M1 required
goal: `gear board` — a read-only full-screen text view of the plan, redrawn every 2 s, `q` quits. New crate crates/gear-board (library, called from gear-cli; add only the `board` subcommand line in crates/gear-cli/src/main.rs). Content (PRD §15): totals (tasks, done, in progress, waiting); per lane: its tasks in queue order with state (done / in review / in progress / queued / waiting); items waiting for the captain (inbox count, oldest age); temp slots in use / quota; one warning line only if the engine lock is free (engine down). No agent working/idle, no heartbeat.
accept: cargo test --workspace green; a test renders the board from an in-memory store with 2 lanes and 4 tasks and checks the lines; running `gear board` with GEAR_DB set shows the gear project.
deps: -

## NEW [must] lane=board milestone=M1 required
goal: Install as a Herdr plugin. herdr-plugin.toml at the repo root (id herdr-gear): a `[[build]]` that runs `cargo build --release`, a `[[panes]]` entry `board` running the gear binary with `board`, and two `[[actions]]`: open the board in a split and in a tab (follow the herdr-file-viewer plugin's manifest: ~/.config/herdr/plugins/github/*/herdr-plugin.toml). A `bin/gear` wrapper so agents find `gear` on PATH. README section: install with `herdr plugin link`, bind a key with `[[keys.command]]` type plugin_action.
accept: `herdr plugin link <repo>` succeeds on this machine and `herdr plugin action invoke` of the board action opens the board (report the exact commands you ran); cargo test --workspace green.
deps: -

## NEW [must] lane=runtime milestone=M1 required
goal: The engine starts a reviewer when a lane hands in a result. When an attempt is Delivered (task Verifying) and no review temp is bound to it: if a temp slot is free (`[quota].temps`), create a temp seat for the task, `worktree_create` branch `review/t<n>` based on the candidate commit, start the `[temps]` agent with the first `[models].review` model that differs from the writer's model, and deliver a review packet (task goal + accept, candidate, the diff range `<integration>..<candidate>`, ops/REVIEW.md rules, and the exact `gear send --kind review ...` command). Slot full → queued (shown to the captain). The lane is not blocked: it continues with its next task.
accept: cargo test --workspace green; an engine test with FakeHerdr: result in -> one temp seat, one worktree_create on review/t<n>, one agent_start, one delivery; second tick does not start another; no free slot -> no start.
deps: T-1, T-2
