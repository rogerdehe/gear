# Stopgaps (remove when their trigger is met)

Things that exist only because GEAR cannot run this project itself yet. Each has a trigger and an owner task; none may become permanent. Adding a stopgap means adding a row here in the same commit; the captain reviews this list at every milestone.

| Stopgap | Why it exists | Remove when | Removed by |
|---|---|---|---|
| `.claude/settings.local.json` Stop hook + `ops/bin/watch-guard.sh` | the captain forgot to re-arm the watcher (L-22, L-39) | the captain seat is onboarded by `gear run` and its doorbell comes from the engine | T-60 |
| `.git/gear-watch.pause` switch | lets the user turn the guard off | same as the hook | T-60 |
| `ops/bin/watch.sh` (+ heartbeat `.git/gear-watch.alive`, seen-state `.git/gear-watch`) | manual stand-in for the engine's observe + doorbell | `gear run` runs this project | T-60 |
| `ops/bin/inbox.sh`, `ops/bin/board.sh`, `ops/mail/*.md`, `ops/PLAN.md` board, `ops/tasks/*.md` packets | manual stand-ins for `gear` home, `gear reply`, the plan store and rendered packets | the project's plan lives in GEAR's store (plan apply) | T-60 |
| `ops/bin/start-reviewer.sh`, warm reviewer seats `home-review-<k>`, `ops/models.conf` | manual temp-reviewer staffing | engine staffs temp reviewers (design/10, 11) and `[models]` is read from config | T-60 |
| `ops/bin/merge-task.sh`, `ops/bin/switch-model.sh`, `ops/bin/timer.sh` | manual captain operations | the engine performs merge gate / model fallback / quota wait | T-60 |
| `~/.local/bin/gear` symlink to `target/release/gear` | lanes started by `gear run` need `gear` on PATH | GEAR has an install step (design/14 plugin) | T-60 |
