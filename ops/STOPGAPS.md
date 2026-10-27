# Stopgaps (remove when their trigger is met)

Things that exist only because GEAR cannot run this project itself yet. Each has a trigger and an owner task; none may become permanent. Adding a stopgap means adding a row here in the same commit; the captain reviews this list at every milestone.

| Stopgap | Why it exists | Remove when | Removed by |
|---|---|---|---|
| `~/.local/bin/gear` symlink to `target/release/gear` | lanes started by `gear run` need `gear` on PATH | GEAR has an install step (design/14 plugin) | captain |
| The captain's lane watch (`scratchpad/watch.sh`, every 5 min: no commit for 20 min on a task, a working lane whose session is 10 min old, an idle lane with a Started attempt, `find /` orphans, load; wakes the captain, re-armed after each wake) | the engine does not yet ask a lane that has not pushed, nor watch the machine | T-194, T-206 and T-209 run in the engine; the machine watch in M5 | captain |
| Plan park ended by hand: `- hold <lane>`, apply, `- release <lane>`, apply | a plan park has no exit (`until=` never expires, `release` refuses Parked) | T-190 merged | captain |

## Removed

- 2026-10-26 Delivered attempts started by hand in the store (T-157, T-156): the engine rebuilt at 19:54 runs T-150's late confirm.

- 2026-10-26 home-deck/home-hull worktrees removed by hand: T-125 (a re-registered lane reuses its worktree).
- 2026-10-26 captain instructions typed into lane panes: T-130 (messages reach a working lane).
- 2026-10-26 held lanes moved to needs_check by hand: T-133 (`- release <lane>`).

- 2026-10-23 captain switches a lane from MiniMax (OpenCode) to Codex gpt-6.1-sol medium by hand on a quota stop: the engine closes the agent and starts codex in the same pane and worktree (T-35, the cross-CLI fallback).
- 2026-10-23 `merge-task.sh`, `switch-model.sh`, `timer.sh`: the engine merges (T-12), switches models in place (T-33) and waits for quota (T-15).
- 2026-10-23 GEAR_DB exported by hand: `gear` defaults to `~/.local/state/gear/gear.db`.

- 2026-10-22 Stop hook + `watch-guard.sh`, pause switch, `watch.sh`: the engine's captain doorbell is verified (it rang the captain for every hand-in today).
- 2026-10-23 `ops/bin/codex-review.sh`: the engine starts reviewers (T-5).
- 2026-10-23 manual release of review temps: the engine releases them (T-17).
- 2026-10-23 `inbox.sh`, `board.sh`, `ops/mail`, `ops/PLAN.md`, `ops/tasks` packets: archived/removed — GEAR's store, home page and board replaced them.
- 2026-10-23 `start-reviewer.sh`, `models.conf`, warm seats `home-review-<k>`: the engine starts reviewers (T-5) on the config's `[models].review`; warm seats return with T-18.
- 2026-10-25 `[profiles.codex]` override in `gear/gear.toml` (update check off): T-57 put the flag in the built-in codex profile.
