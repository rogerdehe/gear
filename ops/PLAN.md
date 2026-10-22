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
| gear-runtime | `gear-runtime` (w8) / w8:p1 | `/Volumes/Data/.herdr-worktrees/gear/home-runtime` | `home/runtime` |

## Models (this project's chains, 17 §0)

| Role | Chain (primary → fallbacks) | Now |
|---|---|---|
| lanes | MiniMax M3.1 Flash → DeepSeek V4.1 Flash 1017 | DeepSeek V4.1 Flash 1017 (MiniMax quota, 2026-10-22) |
| reviewers | DeepSeek V4.1 Flash 1017 → DeepSeek V4 Pro (first ≠ writer's current model) | per job |

## Board

Single writer: the captain (`ops/CAPTAIN.md`). Rows are each lane's queue in order; lanes take the first `ready` row (`ops/bin/inbox.sh`). States: `draft → ready → running → verifying → done`, `rework`, `waiting`, `cancelled`.

| Task | Lane | Branch | Content | State | Depends on |
|---|---|---|---|---|---|
| T-1 | data | `data/t1-store` | gear-store implementation | done | — |
| T-7 | data | `data/t7-config` | project config: parse + validate (pure) | done | — |
| T-13 | data | `data/t13-identity` | app: caller identity (observe, require_seat, classify) | done | — |
| T-16 | data | `data/t16-select-batch` | app: inbox batch selection | done | — |
| T-14 | data | `data/t14-send` | app: gear send | done | — |
| T-15 | data | `data/t15-reply` | app: gear reply | done | — |
| T-17 | data | `data/t17-home` | app: home page + view record | verifying | — |
| T-3 | data | — | split into T-13..T-17 | cancelled | — |
| T-24 | data | `data/t24-send-followups` | send: `--to` means instruction; pin thread ids (from R-11) | done | — |
| T-26 | data | `data/t26-reply-followups` | reply: dedupe ids, answer every thread, wait needs a time (from R-15) | verifying | — |
| T-10 | data | `data/t10-notarget` | store: raw state writes refuse a missing target (from R-1) | ready | — |
| T-22 | data | `data/t22-agentless-pane` | identity: agent-less seat pane rejected (from R-6) | ready | — |
| T-11 | data | `data/t11-config-trim` | config: trim before comparing (from R-2) | done | — |
| T-2 | runtime | `runtime/t2-herdr` | gear-herdr: profile, outcome mapping, SocketHerdr incl. worktree calls | done | — |
| T-8 | runtime | `runtime/t8-doorbell` | engine doorbell decision (pure) | done | — |
| T-9 | runtime | `runtime/t9-skill` | `gear --skill` printing the embedded skill | done | — |
| T-12 | runtime | `runtime/t12-herdr-followups` | herdr adapter: refusal codes, strict not-found, reply-mapping tests (from R-3) | done | — |
| T-18 | runtime | `runtime/t18-intend-recover` | engine: intend doorbells, recover on start | done | — |
| T-19 | runtime | `runtime/t19-execute` | engine: execute intended ops | done | — |
| T-20 | runtime | `runtime/t20-confirm` | engine: confirm doorbells from views | done | — |
| T-21 | runtime | `runtime/t21-reconcile` | engine: observe and reconcile seats | verifying | — |
| T-23 | runtime | `runtime/t23-more-refusals` | herdr: two more refusal codes (from R-8) | verifying | — |
| T-25 | runtime | `runtime/t25-confirm-evidence` | confirm: link evidence; Unknown path test (from R-13) | ready | — |
| T-27 | data | `data/t27-config-models` | config: `[models]` chains + validation (01, 17 §0) | draft | — |
| T-28 | runtime | `runtime/t28-cli-plumbing` | cli: env, store, request id | ready | — |
| T-4 | runtime | `runtime/t4-tick` | engine: tick wiring; slice1_messaging passes; `recover_on_start` returns project-qualified ids (R-10 question) | draft | T-14, T-15, T-17, T-18, T-19, T-20, T-21 |
| T-5 | runtime | `runtime/t5-cli` | gear-cli: slice-1 commands, `gear run` loop | draft | T-14, T-15, T-17, T-4 |
| R-1 | review | `review/t1` | review T-1 (branch `data/t1-store`) | done | — |
| R-2 | review | `review/t7` | review T-7 (branch `data/t7-config`) | done | — |
| R-3 | review | `review/t2` | review T-2 (branch `runtime/t2-herdr`) | done | — |
| R-4 | review | `review/t8` | review T-8 (branch `runtime/t8-doorbell`) | done | — |
| R-5 | r9 | `review/t9` | review T-9 (branch `runtime/t9-skill`) — temp reviewer gear-r9 (session ses_e9cf4091fffejJRMFPr6pkDgum) | done | — |
| R-6 | r13 | `review/t13` | review T-13 (branch `data/t13-identity`) — temp reviewer gear-r13 (session ses_e9cf4091fffejJRMFPr6pkDgum) | done | — |
| R-7 | r11 | `review/t11` | review T-11 (branch `data/t11-config-trim`) — temp reviewer gear-r11 (session ses_e9cefccd8ffezPlP9TL2p98Ctf) | done | — |
| R-8 | r12 | `review/t12` | review T-12 (branch `runtime/t12-herdr-followups`) — temp reviewer gear-r12 (session ses_e9cef8ed2ffe4Rh4t71sju79cH) | done | — |
| R-9 | r16 | `review/t16` | review T-16 (branch `data/t16-select-batch`) — temp reviewer gear-r16 on seat (session ses_e9cd8571bffe30IBlLqfDXjHB4), 2 rounds | done | — |
| R-10 | r18 | `review/t18` | review T-18 (branch `runtime/t18-intend-recover`) — temp reviewer gear-r18 on seat (session ses_e9cd3146cffepZeSJm2b17P1uR) | done | — |
| R-11 | r14 | `review/t14` | review T-14 (branch `data/t14-send`) — temp reviewer gear-r14 on seat (session ses_e9cd1de4dffeo36eWpI5Gd6LmK) | done | — |
| R-12 | r19 | `review/t19` | review T-19 (branch `runtime/t19-execute`) — temp reviewer gear-r19 on seat (session ses_e9ccac231ffebIFJA0Dk6TAIsf), 2 rounds | done | — |
| R-13 | r20 | `review/t20` | review T-20 (branch `runtime/t20-confirm`) — temp reviewer gear-r20 on seat (session ses_e9cc2b643ffejLx5AURY4CHrZ2) | done | — |
| R-14 | r24 | `review/t24` | review T-24 (branch `data/t24-send-followups`) — temp reviewer gear-r24 on seat (session ses_e9cc1a9dcffer4EBceKwV1scT6) | done | — |
| R-15 | r15 | `review/t15` | review T-15 (branch `data/t15-reply`) — temp reviewer gear-r15 on seat (session ses_e9cbd29c7ffeOo3aMwpvkQjTBf) | done | — |
| R-16 | r17 | `review/t17` | review T-17 (branch `data/t17-home`) — temp reviewer gear-r17 on seat (session ses_e9cb776fdffe7xhq2Jh4IWaxRZ) | running | — |
| R-17 | r26 | `review/t26` | review T-26 (branch `data/t26-reply-followups`) — temp reviewer gear-r26 on seat | running | — |
| R-18 | r21 | `review/t21` | review T-21 (branch `runtime/t21-reconcile`) — temp reviewer gear-r21 on seat | running | — |
| R-19 | r23 | `review/t23` | review T-23 (branch `runtime/t23-more-refusals`) — temp reviewer gear-r23 on seat | running | — |
| T-6 | captain | — | skills/gear/SKILL.md v1 (includes 17-conventions); fix `--skill` help text to served topics (R-5 question) | draft | — |

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
- 2026-10-22 practice finding: a long-lived review lane must be cleared before every review, which an async lane cannot do → retired gear-review (exited, worktree removed via Herdr); reviews are temp workers from now on (17 §8, §8a; ops/REVIEW.md requirements-only). T-9 handed in → R-5 by temp reviewer gear-r9.
- 2026-10-22 T-13 handed in → R-6 by temp reviewer gear-r13 (started with ops/bin/start-reviewer.sh).
- 2026-10-22 R-5 pass (no findings; first temp reviewer). Merged T-9 + review; reviewer exited, worktree removed, session recorded. Help-text question → T-6.
- 2026-10-22 R-7 pass (T-11), R-8 pass (T-12), R-6 fail overridden → pass: its only failed fact was T-11's contract test, red on the branch base before dev-green (L-19), not T-13's work. Merged T-11, T-12, T-13 + reviews; T-11 ignore removed; slice-1 acceptance tests ignored until T-4 (L-23). T-14, T-15 → ready. Follow-ups: T-22 (R-6 question + P3: agent-less pane rejected), T-23 (R-8 question: two refusal codes). Reviews sat undecided because the watcher was off after a compaction; data lane idle at the cap → L-22 (heartbeat, `capped` event).
- 2026-10-22 T-18 handed in with a captain question (engine cannot enumerate projects). Decision (b): captain added `Tx::project_ids()` + contract test on dev; T-18 → rework (M-1) to use it and drop its private meta-table registry. No review started on the first hand-in (it would review code about to change).
- 2026-10-22 T-16 handed in → R-9 by gear-r16, first job on a warm reviewer seat (ops/bin/start-reviewer.sh).
- 2026-10-22 R-9 fail (P2 `max_threads = 0` returns a thread; test P3). T-16 → rework (data M-1); per_sender rule and T-17 limits decided there. T-18 re-handed in after M-1 → R-10 on the warm seat.
- 2026-10-22 T-14 handed in → R-11.
- 2026-10-22 R-10 pass (P3 stale diff count only). Merged T-18 + review; workspace green. R-10 question (bare op ids are ambiguous across projects): T-4 changes `recover_on_start` to return (project, id) pairs — noted on the T-4 row for its packet.
- 2026-10-22 R-11 pass (P3s). Merged T-14 + review; thread doc amended to `<seq>`. Decisions: `--to` ⇒ Instruction only, `needs_ack` false in slice 1 → T-24. T-16 rework will meet T-14 in mail.rs imports; the lane resolves when merging origin/dev (17 §8). Data lane ended its turn after a hand-in instead of re-running its inbox → L-26.
- 2026-10-22 T-19 handed in → R-12.
- 2026-10-22 R-12 fail: T-19 conflicts with dev in ops.rs imports (T-18 merged after the cut). T-19 → rework (runtime M-2): merge origin/dev, resolve; doc comment says doorbell ops only (decision: other kinds later). New lane rule: check `merge-tree` before every hand-in (L-27). T-16 rework handed in → R-9 round 2 (resume gear-r16's session).
- 2026-10-22 R-9 round 2 pass. Merged T-16 + review; workspace green. T-17 → ready (T-13, T-16 merged). The round-2 `done` did not ring (event keyed without the round) → watcher keys review events on the round.
- 2026-10-22 T-20 handed in → R-13. start-reviewer.sh now refuses a branch that does not merge into dev (L-27).
- 2026-10-22 T-24 handed in → R-14.
- 2026-10-22 R-13 pass (P3s). Merged T-20 + review; workspace green. Decisions: evidence counts from intent time; confirmation records the view → T-25.
- 2026-10-22 R-14 pass (P3s). Merged T-24 + review. Question (store has no prefix/max thread query): the workaround stands for slice 1. Root cause is the packet: it asked for "one query" that its allowed changes could not reach → L-28.
- 2026-10-22 T-15 handed in → R-15.
- 2026-10-22 R-15 pass (P3s). Merged T-15 + review; workspace green. Decisions → T-26: dedupe ids; one answer per (sender, thread); Wait requires wait_until; downgrade/next_check are slice 2.
- 2026-10-22 T-17 handed in → R-16. T-19 rework handed in → R-12 round 2 (resume gear-r19).
- 2026-10-22 R-12 round 2 pass (P3s). Merged T-19 + review; workspace green.
- 2026-10-22 User: when MiniMax quota runs out, switch the long-lived lanes temporarily to DeepSeek V4.1 Flash. Mechanism: watcher `quota` event (screen markers), ops/bin/switch-model.sh, start-reviewer picks an alternate reviewer model when the writer runs the default reviewer model (independence).
- 2026-10-22 MiniMax quota hit on both lanes → switched gear-data and gear-runtime to DeepSeek V4.1 Flash (switch-model.sh); switch back after the reset.
- 2026-10-22 Correction: the first switch picked the free OpenCode tier (prefix match), which hit its limit; lanes now on DeepSeek V4.1 Flash 1017 (L-29).
- 2026-10-22 R-16 fail (P1 same-second message lost; P2 instruction shown twice). T-17 → rework (data M-2); line cap out of T-17.
- 2026-10-22 User: GEAR is also for one requirement iterated inside an existing repo, and every project starts with a kickoff aligned with the user — installed agents/models inventory, lanes, per-role model chains with fallbacks. Written into 17 §0, 01 `[models]`, 00 §0; T-27 (config `[models]`) drafted.
- 2026-10-22 Handed in: T-26 → R-17, T-21 → R-18, T-17 rework → R-16 round 2, T-23 → R-19 (queued: 3 seats busy).
- 2026-10-22 runtime lowready → T-28 (CLI plumbing, independent). Next for runtime after T-17 lands: T-4 packet; CLI send/reply wiring needs a captain decision on how CLI tests get a Herdr fingerprint (T-22 rejects an agent-less caller).
- 2026-10-22 Both lanes capped (2 in review each) while 3 seats were busy → 4th seat; review throughput is the bottleneck now. start-reviewer waits for the UI before the model check and confirms the prompt was picked up.
