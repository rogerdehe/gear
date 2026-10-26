# Stopgaps (remove when their trigger is met)

Things that exist only because GEAR cannot run this project itself yet. Each has a trigger and an owner task; none may become permanent. Adding a stopgap means adding a row here in the same commit; the captain reviews this list at every milestone.

| Stopgap | Why it exists | Remove when | Removed by |
|---|---|---|---|
| `~/.local/bin/gear` symlink to `target/release/gear` | lanes started by `gear run` need `gear` on PATH | GEAR has an install step (design/14 plugin) | captain |
| the old `home-deck` and `home-hull` worktrees removed by hand (`git worktree remove`) so the re-registered lanes could onboard on pi | onboarding a re-registered lane creates its worktree again and fails on "already exists" | T-125 lands (onboarding reopens an existing worktree) | captain: delete this row |
| every captain instruction to a lane is also typed into its pane with `herdr agent prompt` | the engine rings only idle lanes, so a working lane never reads mid-task instructions | T-130 lands (urgent and captain messages reach a working lane) | captain: delete this row |
| hull and deck moved from held to needs_check by hand in the store, then `- retry` | no Staff verb releases a held lane | T-133 lands (`- release <lane>`) | captain: delete this row |

## Removed

- 2026-10-23 captain switches a lane from MiniMax (OpenCode) to Codex gpt-6.1-sol medium by hand on a quota stop: the engine closes the agent and starts codex in the same pane and worktree (T-35, the cross-CLI fallback).
- 2026-10-23 `merge-task.sh`, `switch-model.sh`, `timer.sh`: the engine merges (T-12), switches models in place (T-33) and waits for quota (T-15).
- 2026-10-23 GEAR_DB exported by hand: `gear` defaults to `~/.local/state/gear/gear.db`.

- 2026-10-22 Stop hook + `watch-guard.sh`, pause switch, `watch.sh`: the engine's captain doorbell is verified (it rang the captain for every hand-in today).
- 2026-10-23 `ops/bin/codex-review.sh`: the engine starts reviewers (T-5).
- 2026-10-23 manual release of review temps: the engine releases them (T-17).
- 2026-10-23 `inbox.sh`, `board.sh`, `ops/mail`, `ops/PLAN.md`, `ops/tasks` packets: archived/removed — GEAR's store, home page and board replaced them.
- 2026-10-23 `start-reviewer.sh`, `models.conf`, warm seats `home-review-<k>`: the engine starts reviewers (T-5) on the config's `[models].review`; warm seats return with T-18.
- 2026-10-25 `[profiles.codex]` override in `gear/gear.toml` (update check off): T-57 put the flag in the built-in codex profile.
