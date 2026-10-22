# GEAR development ledger (manual stand-in for GEAR's PLAN view)

Goal: GEAR MVP. Current milestone: **M1 skeleton + slice 1 (messaging loop)**.
Done when: `cargo test --workspace` passes, including `crates/gear-engine/tests/slice1_messaging.rs`.

## Decisions

- 2026-10-22 Rust; outside-in slices; the captain builds the skeleton (gear-model fully implemented; store/herdr/app/engine/cli as signatures + contract tests); lanes implement.
- 2026-10-22 Herdr adapter talks to the socket via vendored board-herdr (MIT, from herdr-board).
- 2026-10-22 Repo `rogerdehe/gear` (private). Branches per 17-conventions: `main` (release), `dev` (integration), task branches `<lane>/t<n>-<slug>` from `dev`; `staging` not used yet.
- 2026-10-22 Worktrees only through `herdr worktree create`; lanes hold one long-lived worktree each; temp workers get their own. No slot pool, no hand-written paths.
- 2026-10-22 Lanes named by duty: `gear-data` (storage + business rules), `gear-runtime` (Herdr adapter + engine).
- 2026-10-22 All repo files in English.

## Lanes

| Lane | Herdr workspace / pane | Worktree | Anchor branch |
|---|---|---|---|
| gear-data | `gear-data` (w7) / w7:p1 | `/Volumes/Data/.herdr-worktrees/gear/home-data` | `home/data` |
| gear-runtime | `gear-runtime` (w8) / w8:p1 | `/Volumes/Data/.herdr-worktrees/gear/home-runtime` | `home/runtime` |

## Board

Single writer: the captain (see `ops/CAPTAIN.md`). States follow GEAR `TaskState`. Rows are in dispatch order per lane.

| Task | Lane | Branch | Content | State | Depends on |
|---|---|---|---|---|---|
| T-1 | data | `data/t1-store` | gear-store implementation | ready | — |
| T-2 | runtime | `runtime/t2-herdr` | gear-herdr: profile, outcome mapping, SocketHerdr incl. worktree calls | ready | — |
| T-3 | data | `data/t3-app` | gear-app: identity + mail (send/select_batch/reply) + home | draft | T-1 |
| T-4 | runtime | `runtime/t4-engine` | gear-engine: observe + doorbell + ops + tick; slice1 tests pass | draft | T-1, T-2, T-3 |
| T-5 | runtime | `runtime/t5-cli` | gear-cli: slice-1 commands, `gear run` loop, `--skill` | draft | T-3, T-4 |
| T-6 | captain | — | skills/gear/SKILL.md v1 (includes 17-conventions) | draft | — |

## Done

- M1-0 skeleton (captain): workspace, gear-model (8 tests), FakeHerdr (4 tests incl. worktrees), signatures and contract tests for every layer.
- 2026-10-22 docs translated to English; 17-conventions written; slot pool removed from design and schema.

## Log

- 2026-10-22 manual scaffold set up: board (this file), packets with checklists + lane record, `ops/CAPTAIN.md`, `ops/bin/board.sh`.
- 2026-10-22 lanes gear-data (w7:p1) and gear-runtime (w8:p1) running OpenCode, DeepSeek V4.1 Flash / high. Incident: captain typed `export …` into panes that already hosted agents (submitted as a prompt); interrupted, no changes. Restarted both with `--auto -s <session>`; context kept.
- 2026-10-22 rule: every agent GEAR starts carries the profile's unattended flags (opencode `--auto`); model/effort switched in-agent via `/model` `/effort`.
