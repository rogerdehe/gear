<!-- gear-plan project=gear base=2 -->
# Decisions
- 2026-10-22 User, mid-run: reviewers run on Codex `gpt-6.1-sol` (reasoning high) instead of OpenCode. `ops/gear.toml` `[temps]` = codex, `[models].review` = ["gpt-6.1-sol"]. Codex takes the model per process (`-m`), so writer and reviewer models never collide. SOP: `gear --skill agent:codex`. Until the store accepts a config update (T-6) the registered config still says opencode; T-5 reads the review model from the config after T-6.

# Queue
## T-5 [must] lane=runtime milestone=M1 required
goal: The engine starts a reviewer when a lane hands in a result. When an attempt is Delivered (task Verifying) and no review temp is bound to it: if a temp slot is free (`[quota].temps`), create a temp seat for the task, `worktree_create` branch `review/t<n>` based on the candidate commit, start the `[temps]` agent (Codex: add a built-in `codex` profile in crates/gear-herdr/src/profile.rs from `gear --skill agent:codex` — unattended `--dangerously-bypass-approvals-and-sandbox`, `session_ref` true, resume `resume {session}`) with `-m <first [models].review model that differs from the writer's>` and `-c model_reasoning_effort=high`, and deliver a review packet (task goal + accept, candidate, the diff range `<integration>..<candidate>`, ops/REVIEW.md rules, and the exact `gear send --kind review ...` command). A fresh worktree's shell needs a few seconds: `agent_pane_busy` right after `worktree_create` is retried on the next tick, not a failure. Slot full -> queued (shown to the captain). The lane is not blocked: it continues with its next task.
accept: cargo test --workspace green; an engine test with FakeHerdr: result in -> one temp seat, one worktree_create on review/t<n>, one agent_start with the codex args, one delivery; second tick does not start another; no free slot -> no start.
deps: T-1, T-2, T-6

## NEW [must] lane=data milestone=M1 required
goal: Change the project config mid-run. `gear admin init <config>` on an already registered project stores the new config as the next config version (today: BAD_FORMAT "already registered") when it is valid and every registered seat is still in it; new lanes become queued seats (the engine onboards them); a lane removed from the config -> BAD_FORMAT naming it (retiring is a later task). The engine reads the latest config version.
accept: cargo test --workspace green; tests: re-init with a changed `[models]` bumps the config version and the store returns the new models; re-init adding a lane creates a queued seat; re-init dropping a lane is rejected.
deps: T-2
