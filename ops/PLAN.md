# GEAR development ledger (manual stand-in for GEAR's PLAN view)

Goal: GEAR MVP. Current milestone: **M1 skeleton + slice 1 (messaging loop)**.
Done when: `cargo test --workspace` passes, including `crates/gear-engine/tests/slice1_messaging.rs`.

## Decisions

- 2026-10-22 Rust; outside-in slices; the captain builds the skeleton (gear-model fully implemented; store/herdr/app/engine/cli as signatures + contract tests); lanes implement.
- 2026-10-22 Herdr adapter talks to the socket via vendored board-herdr (MIT, from herdr-board).
- 2026-10-22 Repo `rogerdehe/gear` (private). Branches per 17-conventions: `main` (release), `dev` (integration), task branches `<lane>/t<n>-<slug>` cut by the lane from `dev`, pushed, deleted after merge; `staging` not used yet.
- 2026-10-22 Worktrees only through `herdr worktree create`; lanes hold one long-lived worktree each; temp workers get their own. No slot pool, no hand-written paths.
- 2026-10-22 Lanes named by duty: `gear-data` (storage + business rules), `gear-runtime` (Herdr adapter + engine).
- 2026-10-22 All repo files in English.

## Lanes

| Lane | Herdr workspace / pane | Worktree | Anchor branch |
|---|---|---|---|
| gear-data | `gear-data` (w7) / w7:p1 | `/Volumes/Data/.herdr-worktrees/gear/home-data` | `home/data` |
| gear-review | `gear-review` / (set at onboarding) | `/Volumes/Data/.herdr-worktrees/gear/home-review` | `home/review` |
| gear-runtime | `gear-runtime` (w8) / w8:p1 | `/Volumes/Data/.herdr-worktrees/gear/home-runtime` | `home/runtime` |

## Board

Single writer: the captain (`ops/CAPTAIN.md`). Rows are each lane's queue in order; lanes take the first `ready` row (`ops/bin/inbox.sh`). States: `draft → ready → running → verifying → done`, `rework`, `waiting`, `cancelled`.

| Task | Lane | Branch | Content | State | Depends on |
|---|---|---|---|---|---|
| T-1 | data | `data/t1-store` | gear-store implementation | done | — |
| T-7 | data | `data/t7-config` | project config: parse + validate (pure) | done | — |
| T-13 | data | `data/t13-identity` | app: caller identity (observe, require_seat, classify) | ready | — |
| T-16 | data | `data/t16-select-batch` | app: inbox batch selection | ready | — |
| T-14 | data | `data/t14-send` | app: gear send | draft | T-13 |
| T-15 | data | `data/t15-reply` | app: gear reply | draft | T-13 |
| T-17 | data | `data/t17-home` | app: home page + view record | draft | T-13, T-16 |
| T-3 | data | — | split into T-13..T-17 | cancelled | — |
| T-10 | data | `data/t10-notarget` | store: raw state writes refuse a missing target (from R-1) | ready | — |
| T-11 | data | `data/t11-config-trim` | config: trim before comparing (from R-2) | ready | — |
| T-2 | runtime | `runtime/t2-herdr` | gear-herdr: profile, outcome mapping, SocketHerdr incl. worktree calls | done | — |
| T-8 | runtime | `runtime/t8-doorbell` | engine doorbell decision (pure) | done | — |
| T-9 | runtime | `runtime/t9-skill` | `gear --skill` printing the embedded skill | ready | — |
| T-12 | runtime | `runtime/t12-herdr-followups` | herdr adapter: refusal codes, strict not-found, reply-mapping tests (from R-3) | ready | — |
| T-18 | runtime | `runtime/t18-intend-recover` | engine: intend doorbells, recover on start | ready | — |
| T-19 | runtime | `runtime/t19-execute` | engine: execute intended ops | ready | — |
| T-20 | runtime | `runtime/t20-confirm` | engine: confirm doorbells from views | ready | — |
| T-21 | runtime | `runtime/t21-reconcile` | engine: observe and reconcile seats | ready | — |
| T-4 | runtime | `runtime/t4-tick` | engine: tick wiring; slice1_messaging passes | draft | T-14, T-15, T-17, T-18, T-19, T-20, T-21 |
| T-5 | runtime | `runtime/t5-cli` | gear-cli: slice-1 commands, `gear run` loop | draft | T-14, T-15, T-17, T-4 |
| R-1 | review | `review/t1` | review T-1 (branch `data/t1-store`) | done | — |
| R-2 | review | `review/t7` | review T-7 (branch `data/t7-config`) | done | — |
| R-3 | review | `review/t2` | review T-2 (branch `runtime/t2-herdr`) | done | — |
| R-4 | review | `review/t8` | review T-8 (branch `runtime/t8-doorbell`) | done | — |
| T-6 | captain | — | skills/gear/SKILL.md v1 (includes 17-conventions) | draft | — |

## Done

- M1-0 skeleton (captain): workspace, gear-model (8 tests), FakeHerdr (4 tests incl. worktrees), signatures and contract tests for every layer.
- 2026-10-22 docs translated to English; 17-conventions written; slot pool removed from design and schema.

## Log

- 2026-10-22 manual scaffold set up: board (this file), packets with checklists + lane record, `ops/CAPTAIN.md`, `ops/bin/board.sh`.
- 2026-10-22 lanes gear-data (w7:p1) and gear-runtime (w8:p1) running OpenCode, DeepSeek V4.1 Flash / high. Incident: captain typed `export …` into panes that already hosted agents (submitted as a prompt); interrupted, no changes. Restarted both with `--auto -s <session>`; context kept.
- 2026-10-22 rule: every agent GEAR starts carries the profile's unattended flags (opencode `--auto`); model/effort switched in-agent via `/model` `/effort`.
- 2026-10-22 both lanes switched to MiniMax-M3.1-Flash (minimax.cn) via /model; /effort verified on DeepSeek (MiniMax has no variants); both /clear-ed (stale context from the export incident removed).
- 2026-10-22 practice findings: (1) lanes must not idle waiting for review → non-blocking loop (17 §5–§6), rework at the next checklist boundary; (2) task branches are pushed for review and deleted after merge; (3) the M1 queues forced waiting (T-1→T-3, T-2→T-4→T-5) → added contract-first independent tasks T-7 (config), T-8 (doorbell), T-9 (`--skill`), each with captain-written contract tests validated against a throwaway implementation; (4) decomposition rules in 17 §6a + `gear --skill decompose` (method-neutral; BMAD and superpowers mapped).
- 2026-10-22 kickoff: both lanes hit the MiniMax Token Plan limit (error 2056) within ~90 s, before starting any task; Herdr reported `done`. Waiting for the reset; quota signs added to the opencode SOP.
- 2026-10-22 practice finding: the captain doing acceptance + code review itself (~10+ min per task) would block it all day. Added a review lane `gear-review` (OpenCode + DeepSeek, different model from the MiniMax writers): mechanical acceptance + code review into `ops/reviews/T-<n>.md` on `review/t<n>`; the captain only decides. T-1 and T-7 handed in → R-1, R-2 queued.
- 2026-10-22 T-2 handed in → R-3 queued. gear-data stopped: T-1/T-7 in review, T-3 waits for T-1 (planning gap: data lane had only two independent tasks).
- 2026-10-22 R-1 pass (P3 doc wording; question: raw state writes succeed silently on a missing row). Captain: merged T-1 + review into dev (acceptance re-run on dev), branches deleted; T-3 → ready. Silent-miss question → follow-up task T-10 (data).
- 2026-10-22 practice finding: gear-data sat idle ~15 min — the watcher rang once and the captain's attention went elsewhere. Fix: idle lanes re-ring until handled (watch.sh).
- 2026-10-22 T-8 handed in → R-4 queued.
- 2026-10-22 R-2 pass (P3: owns/branch names compared without trimming). Merged T-7 + review into dev; acceptance re-run on dev. P3 → follow-up T-11 (data).
- 2026-10-22 restocked gear-data: T-10, T-11 (small, contract tests written and validated against a throwaway fix).
- 2026-10-22 caught: T-3 was marked ready without a packet and is too big for the size rule → back to draft; captain splits it next.
- 2026-10-22 R-3 pass (2× P3, 3 questions) and R-4 pass (no findings). Merged T-2, T-8 + reviews; acceptance re-run on dev. Decisions on R-3 questions → T-12 (runtime): busy/unavailable/name-taken are refusals; generic not_found is an observation failure; pin the worktree reply mapper. `pane.send_input` key semantics left to the captain's live probe.
- 2026-10-22 T-3 split into T-13 identity, T-16 select_batch (both ready, independent), T-14 send, T-15 reply (after T-13), T-17 home (after T-13, T-16). Lane writes the named tests first (intra-lane behaviour; public signatures already fixed).
- 2026-10-22 T-4 split: T-18..T-21 (independent, store + herdr already merged) and a thin T-4 for the tick + slice-1 acceptance. Captain fixed `observe::reconcile` to take `profiles` (screen flags need them).
