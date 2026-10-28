# M1 stopgaps

Workarounds added or removed during M1. Every stopgap has a removal trigger; one still open at the landing moves to the next voyage's backlog.

- 2026-10-22 Stop hook + `watch-guard.sh`, pause switch, `watch.sh`: removed — the engine's captain doorbell is verified (it rang the captain for every hand-in).
- 2026-10-23 `merge-task.sh`, `switch-model.sh`, `timer.sh`: removed — the engine merges (T-12), switches models in place (T-33) and waits for quota (T-15).
- 2026-10-23 GEAR_DB exported by hand: removed — `gear` defaults to `~/.local/state/gear/gear.db`.
- 2026-10-23 `ops/bin/codex-review.sh`: removed — the engine starts reviewers (T-5).
- 2026-10-23 manual release of review temps: removed — the engine releases them (T-17).
- 2026-10-23 `inbox.sh`, `board.sh`, `ops/mail`, `ops/PLAN.md`, `ops/tasks` packets: archived/removed — GEAR's store, home page and board replaced them.
- 2026-10-23 `start-reviewer.sh`, `models.conf`, warm seats `home-review-<k>`: removed — the engine starts reviewers (T-5) on the config's `[models].review`.
- `~/.local/bin/gear` symlink to `target/release/gear` (open): lanes started by `gear run` need `gear` on PATH. Remove when GEAR has an install step (design/14 plugin). Still open at M4.2's landing; carried to M5's backlog.
