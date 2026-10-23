# Stopgaps (remove when their trigger is met)

Things that exist only because GEAR cannot run this project itself yet. Each has a trigger and an owner task; none may become permanent. Adding a stopgap means adding a row here in the same commit; the captain reviews this list at every milestone.

| Stopgap | Why it exists | Remove when | Removed by |
|---|---|---|---|
| `ops/bin/start-reviewer.sh`, warm reviewer seats `home-review-<k>`, `ops/models.conf` | manual temp-reviewer staffing | engine staffs temp reviewers (design/10, 11) and `[models]` is read from config | captain |
| `ops/bin/merge-task.sh`, `ops/bin/switch-model.sh`, `ops/bin/timer.sh` | manual captain operations | the engine performs merge gate / model fallback / quota wait | captain |
| `~/.local/bin/gear` symlink to `target/release/gear` | lanes started by `gear run` need `gear` on PATH | GEAR has an install step (design/14 plugin) | captain |
| GEAR state at `~/.local/state/gear/gear.db` (GEAR_DB exported by hand in the captain's commands) | no plugin install yet, so no `HERDR_PLUGIN_STATE_DIR` | the plugin install (gear T-4) | captain |

## Removed

- 2026-10-22 Stop hook + `watch-guard.sh`, pause switch, `watch.sh`: the engine's captain doorbell is verified (it rang the captain for every hand-in today).
- 2026-10-23 `ops/bin/codex-review.sh`: the engine starts reviewers (T-5).
- 2026-10-23 manual release of review temps: the engine releases them (T-17).
- 2026-10-23 `inbox.sh`, `board.sh`, `ops/mail`, `ops/PLAN.md`, `ops/tasks` packets: archived/removed — GEAR's store, home page and board replaced them.
