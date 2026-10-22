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
| T-17 | data | `data/t17-home` | app: home page + view record | done | — |
| T-31 | data | `data/t31-draft-parse` | slice 2: plan draft parser (pure; types + contract by captain) | done | — |
| T-33 | data | `data/t33-store-tasks` | slice 2: store plans, tasks, contracts (types + contract by captain) | done | — |
| T-34 | data | `data/t34-store-attempts` | slice 2: store attempts, verdicts (types + contract by captain) | done | — |
| T-36 | data | `data/t36-plan-apply-queue` | slice 2: `plan apply` queue part (stub + contract by captain) | done | — |
| T-37 | data | `data/t37-draft-strict` | draft parser: two strictness gaps (from R-27) | done | — |
| T-40 | data | `data/t40-open-attempts` | slice 2: store open attempts of a seat (stub + contract by captain) | done | — |
| T-42 | data | `data/t42-attempt-lifecycle` | slice 2: start attempt, confirm delivery, submit result (stubs + contract by captain) | done | — |
| T-43 | data | `data/t43-settle` | slice 2: settle verdicts | done | — |
| T-45 | data | `data/t45-send-result` | slice 2: `gear send --kind result` submits the attempt (contract by captain) | done | — |
| T-50 | data | `data/t50-plan-verdicts` | slice 2: `plan apply` settles `# Verdicts` (contract by captain) | done | — |
| T-51 | data | `data/t51-home-task` | slice 2: home shows and records the current task (contract by captain) | done | — |
| T-53 | data | `data/t53-task-packet` | slice 2: render the task packet (`gear --show T-n`; contract by captain) | verifying | — |
| T-55 | data | `data/t55-plan-staff` | slice 2: `plan apply` `# Staff` park/hold/retire (contract by captain) | ready | — |
| T-3 | data | — | split into T-13..T-17 | cancelled | — |
| T-24 | data | `data/t24-send-followups` | send: `--to` means instruction; pin thread ids (from R-11) | done | — |
| T-26 | data | `data/t26-reply-followups` | reply: dedupe ids, answer every thread, wait needs a time (from R-15) | done | — |
| T-10 | data | `data/t10-notarget` | store: raw state writes refuse a missing target (from R-1) | done | — |
| T-22 | data | `data/t22-agentless-pane` | identity: agent-less seat pane rejected (from R-6) | done | — |
| T-11 | data | `data/t11-config-trim` | config: trim before comparing (from R-2) | done | — |
| T-2 | runtime | `runtime/t2-herdr` | gear-herdr: profile, outcome mapping, SocketHerdr incl. worktree calls | done | — |
| T-8 | runtime | `runtime/t8-doorbell` | engine doorbell decision (pure) | done | — |
| T-9 | runtime | `runtime/t9-skill` | `gear --skill` printing the embedded skill | done | — |
| T-12 | runtime | `runtime/t12-herdr-followups` | herdr adapter: refusal codes, strict not-found, reply-mapping tests (from R-3) | done | — |
| T-18 | runtime | `runtime/t18-intend-recover` | engine: intend doorbells, recover on start | done | — |
| T-19 | runtime | `runtime/t19-execute` | engine: execute intended ops | done | — |
| T-20 | runtime | `runtime/t20-confirm` | engine: confirm doorbells from views | done | — |
| T-21 | runtime | `runtime/t21-reconcile` | engine: observe and reconcile seats | done | — |
| T-23 | runtime | `runtime/t23-more-refusals` | herdr: two more refusal codes (from R-8) | done | — |
| T-25 | runtime | `runtime/t25-confirm-evidence` | confirm: link evidence; Unknown path test (from R-13) | done | — |
| T-27 | data | `data/t27-config-models` | config: validate `[models]` (types + contract test by captain) | done | — |
| T-28 | runtime | `runtime/t28-cli-plumbing` | cli: env, store, request id | done | — |
| T-4 | runtime | `runtime/t4-tick` | engine: tick wiring; slice1_messaging passes; `recover_on_start` returns project-qualified ids (R-10 question) | done | — |
| T-29 | runtime | `runtime/t29-cli-send-reply` | cli: gear send / gear reply wiring | done | — |
| T-30 | runtime | `runtime/t30-cli-home` | cli: `gear` home page wiring | done | — |
| T-32 | runtime | `runtime/t32-gear-run` | cli: `gear run` engine loop (lock, recover, tick) | done | — |
| T-35 | runtime | `runtime/t35-dispatch` | slice 2: engine dispatch decision (pure; types + contract by captain) | done | — |
| T-38 | runtime | `runtime/t38-dispatch-delivered` | dispatch: continue a Delivered attempt (from R-31) | done | — |
| T-39 | runtime | `runtime/t39-deliveries` | slice 2: delivery texts, intend, execute (stubs + contract by captain) | done | — |
| T-41 | runtime | `runtime/t41-lane-view` | slice 2: build the lane view (stub + contract by captain) | done | — |
| T-44 | runtime | `runtime/t44-stale-delivery` | execute: void a stale delivery (from R-35) | done | — |
| T-46 | runtime | `runtime/t46-dispatch-tick` | slice 2: plan + intend dispatch (stubs + contract by captain) | done | — |
| T-47 | runtime | `runtime/t47-missing-attempt` | execute: void a delivery for a missing attempt (from R-41) | done | — |
| T-48 | runtime | `runtime/t48-confirm-delivery` | slice 2: confirm deliveries from views (contract by captain) | done | — |
| T-49 | runtime | `runtime/t49-tick-dispatch` | slice 2: dispatch in the tick; **slice-2 acceptance** (`slice2_dispatch.rs` by captain) | verifying | — |
| T-52 | runtime | `runtime/t52-cli-plan-result` | cli: `gear plan apply`, `--candidate` | done | — |
| T-54 | runtime | `runtime/t54-cli-show` | cli: `gear --show T-n` | draft | T-53 |
| T-56 | runtime | `runtime/t56-builtin-profiles` | built-in opencode profile; `gear run` uses it (contract by captain) | ready | — |
| T-5 | runtime | — | split into T-28, T-29, T-30, T-32 | cancelled | — |
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
| R-16 | r17 | `review/t17` | review T-17 (branch `data/t17-home`) — temp reviewer gear-r17 on seat (session ses_e9cb776fdffe7xhq2Jh4IWaxRZ), 2 rounds | done | — |
| R-17 | r26 | `review/t26` | review T-26 (branch `data/t26-reply-followups`) — temp reviewer gear-r26 on seat (session ses_e9cad4a35ffec27oj5H7Z4AMDZ) | done | — |
| R-18 | r21 | `review/t21` | review T-21 (branch `runtime/t21-reconcile`) — temp reviewer gear-r21 (round 1 ses_e9cacb72bffe85APn13cVdEnng; round 2 new-session recovery ses_e9c9f4c20ffeR4s83ApKF3UHMP) | done | — |
| R-19 | r23 | `review/t23` | review T-23 (branch `runtime/t23-more-refusals`) — temp reviewer gear-r23 on seat (session ses_e9ca9da55ffetpXMr32sRFk5gM) | done | — |
| R-20 | r10 | `review/t10` | review T-10 (branch `data/t10-notarget`) — temp reviewer gear-r10 on seat (session ses_e9ca14555ffeTLHJUTXWF1Xhhb) | done | — |
| R-21 | r22 | `review/t22` | review T-22 (branch `data/t22-agentless-pane`) — temp reviewer gear-r22 on seat (session ses_e9ca11681ffeE6PFvTNI7zruxo) | done | — |
| R-22 | r25 | `review/t25` | review T-25 (branch `runtime/t25-confirm-evidence`) — temp reviewer gear-r25 on seat (session ses_e9ca0e4f2ffeZl3Ym8V2aTt2I7) | done | — |
| R-23 | r28 | `review/t28` | review T-28 (branch `runtime/t28-cli-plumbing`) — temp reviewer gear-r28 on seat (session ses_e9c9ccc9dffeZjGrdNQ5Br72Qz) | done | — |
| R-24 | r27 | `review/t27` | review T-27 (branch `data/t27-config-models`) — temp reviewer gear-r27 on seat (session ses_e9c950e3affeO983U45knVsJzf) | done | — |
| R-25 | r29 | `review/t29` | review T-29 (branch `runtime/t29-cli-send-reply`) — temp reviewer gear-r29 on seat (session ses_e9c94d900ffeYli6RP1JMvQLPH), 2 rounds | done | — |
| R-26 | r4 | `review/t4` | review T-4 (branch `runtime/t4-tick`) — temp reviewer gear-r4 on seat (session ses_e9c924489ffeZdxVO157xCH2ue) | done | — |
| R-27 | r31 | `review/t31` | review T-31 (branch `data/t31-draft-parse`) — temp reviewer gear-r31 on seat (session ses_e9c8a6b70ffeh5sqWBqyBMu4L0) | done | — |
| R-28 | r33 | `review/t33` | review T-33 (branch `data/t33-store-tasks`) — temp reviewer gear-r33 on seat (session ses_e9c877257ffeehrl85pVJ8vmd7) | done | — |
| R-29 | r30 | `review/t30` | review T-30 (branch `runtime/t30-cli-home`) — temp reviewer gear-r30 on seat (session ses_e9c873476ffe0YBhVayZdcwiD9), 2 rounds | done | — |
| R-30 | r32 | `review/t32` | review T-32 (branch `runtime/t32-gear-run`) — temp reviewer gear-r32 on seat (session ses_e9c7f3958ffeScASFffzXSnlxp) | done | — |
| R-31 | r35 | `review/t35` | review T-35 (branch `runtime/t35-dispatch`) — temp reviewer gear-r35 on seat (session ses_e9c7e586cffeX07HoHKoX2EVLQ) | done | — |
| R-32 | r34 | `review/t34` | review T-34 (branch `data/t34-store-attempts`) — temp reviewer gear-r34 on seat (session ses_e9c7a020dffe41ZXCI10PJb0Oe) | done | — |
| R-33 | r36 | `review/t36` | review T-36 (branch `data/t36-plan-apply-queue`) — temp reviewer gear-r36 on seat (session ses_e9c71de3cffeXE6Bju5tFuahIF), round 2 new-session recovery ses_e9c5fd1a3ffetvlHvhqagW5tNe | done | — |
| R-34 | r38 | `review/t38` | review T-38 (branch `runtime/t38-dispatch-delivered`) — temp reviewer gear-r38 on seat (session ses_e9c71adadffeNqEPO3RJKDwFHB) | done | — |
| R-35 | r39 | `review/t39` | review T-39 (branch `runtime/t39-deliveries`) — temp reviewer gear-r39 on seat (session ses_e9c6ebc9effef9Fb7NcakyILcA) | done | — |
| R-36 | r37 | `review/t37` | review T-37 (branch `data/t37-draft-strict`) — temp reviewer gear-r37 on seat (session ses_e9c6aeaedffebAeYJcEaEZ1Api) | done | — |
| R-37 | r6 | `review/t6` | review T-6 (branch `captain/t6-skill`) — temp reviewer gear-r6 on seat (session ses_e9c625c7cffe83nTPnqq88jCPX), 2 rounds | done | — |
| R-38 | r40 | `review/t40` | review T-40 (branch `data/t40-open-attempts`) — temp reviewer gear-r40 on seat (session ses_e9c5c5744ffeS3on5K6XAPD49g) | done | — |
| R-39 | r42 | `review/t42` | review T-42 (branch `data/t42-attempt-lifecycle`) — temp reviewer gear-r42 on seat (session ses_e9c51cd97ffeTfg58BlmtujMiP) | done | — |
| R-40 | r41 | `review/t41` | review T-41 (branch `runtime/t41-lane-view`) — temp reviewer gear-r41 on seat (session ses_e9c4db501ffel4I1Yxj7jRdydK) | done | — |
| R-41 | r44 | `review/t44` | review T-44 (branch `runtime/t44-stale-delivery`) — temp reviewer gear-r44 on seat (session ses_e9c4d4795ffelOAIJwNJzCrEMX) | done | — |
| R-42 | r43 | `review/t43` | review T-43 (branch `data/t43-settle`) — temp reviewer gear-r43 on seat (session ses_e9c42c05dffee9BSIW8chVNNlY) | done | — |
| R-43 | r47 | `review/t47` | review T-47 (branch `runtime/t47-missing-attempt`) — temp reviewer gear-r47 on seat (session ses_e9c3ec62bffe7DLXPgCT5qkfRW) | done | — |
| R-44 | r46 | `review/t46` | review T-46 (branch `runtime/t46-dispatch-tick`) — temp reviewer gear-r46 on seat (session ses_e9c3e8e6affe2zdySh07ehBlAW) | done | — |
| R-45 | r45 | `review/t45` | review T-45 (branch `data/t45-send-result`) — temp reviewer gear-r45 on seat (session ses_e9c37bdc7ffeT9sHr3t67ud0bt) | done | — |
| R-46 | r48 | `review/t48` | review T-48 (branch `runtime/t48-confirm-delivery`) — temp reviewer gear-r48 on seat (session ses_e9c32a21dffe4jKmnxeVGcynpz), round 2 new-session recovery ses_e9c26a7eaffegviPNb0J1QzM41 | done | — |
| R-47 | r50 | `review/t50` | review T-50 (branch `data/t50-plan-verdicts`) — temp reviewer gear-r50 on seat (session ses_e9c2d132fffeCvFXhxwoQcT0qp) | done | — |
| R-48 | r51 | `review/t51` | review T-51 (branch `data/t51-home-task`) — temp reviewer gear-r51 on seat (session ses_e9c27071fffeuSvSJr474MQ22u) | done | — |
| R-49 | r52 | `review/t52` | review T-52 (branch `runtime/t52-cli-plan-result`) — temp reviewer gear-r52 on seat (session ses_e9c2579abffexrPYbsZYHQ00xt) | done | — |
| R-50 | r53 | `review/t53` | review T-53 (branch `data/t53-task-packet`) — temp reviewer gear-r53 on seat | running | — |
| R-51 | r49 | `review/t49` | review T-49 (branch `runtime/t49-tick-dispatch`) — temp reviewer gear-r49 on seat | running | — |
| T-6 | captain | `captain/t6-skill` | skills/gear/SKILL.md v1 (includes 17-conventions); fix `--skill` help text to served topics (R-5 question) | done | — |

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
- 2026-10-22 R-17 pass (two P3 doc lines, fixed by the captain at merge). Merged T-26 + review; workspace green.
- 2026-10-22 R-16 round 2 pass, R-19 pass. Merged T-17, T-23 + reviews; workspace green. slice1_messaging now fails only at `engine::tick` (T-4), which waits on T-21 alone.
- 2026-10-22 R-18 fail (P1: private expects_agent with the wrong state set). T-21 → rework (runtime M-3). Re-reviews resume on the session's own seat (L-31).
- 2026-10-22 data lowready → captain added `[models]` types (parse) + contract test `models_rules` (ignored for T-27); T-27 ready.
- 2026-10-22 T-10 → R-20, T-22 → R-21, T-25 → R-22.
- 2026-10-22 runtime lowready → T-29 (CLI send/reply). Decision: CLI logic in private run_* helpers with injected store/env/herdr, tested with FakeHerdr; cmd_* is glue only.
- 2026-10-22 T-21 rework handed in → R-18 round 2; its session's seat is busy → new-session recovery on a free seat (no waiting on the critical path to T-4).
- 2026-10-22 runtime lowready → T-30 (CLI home).
- 2026-10-22 T-28 → R-23 on a 5th seat (V4 Pro reviews are slower; review throughput is the bottleneck).
- 2026-10-22 R-20 pass (P3 wording). T-10 ignore removed; merged T-10 + review; workspace green.
- 2026-10-22 R-18 round 2 (new-session recovery), R-21, R-22 pass. Merged T-21, T-22, T-25 + reviews; T-21 doc P3 fixed at merge; workspace green. T-4 → ready (all deps merged). Recurring stale-Result P3 removed at the source (L-32).
- 2026-10-22 R-23 pass (P3). Merged T-28 + review; workspace green.
- 2026-10-22 Slice 1 data work done → slice 2 starts: captain added `gear_app::draft` types + contract `draft_contract.rs` (5, ignored); T-31 ready. Next slice-2 contracts (store task/attempt accessors, plan apply, settle) follow.
- 2026-10-22 T-27 → R-24, T-29 → R-25.
- 2026-10-22 runtime lowready → T-32 (`gear run`); T-5 is now only the slice-1 end-to-end CLI acceptance after T-4/T-29/T-30/T-32.
- 2026-10-22 T-4 → R-26.
- 2026-10-22 Slice-2 store contract on dev: row types, method signatures (todo!), `slice2_contract.rs` (5 ignored). T-33, T-34 ready (independent bodies).
- 2026-10-22 R-24 pass (no findings). T-27 ignore removed; merged T-27 + review; workspace green.
- 2026-10-22 R-25 fail (P2 `--need` silently ignored; my packet's unsupported-flag list missed it). T-29 → rework (runtime M-4).
- 2026-10-22 T-31 → R-27; T-29 rework → R-25 round 2.
- 2026-10-22 **Milestone: slice-1 messaging acceptance passes on dev** (slice1_messaging 7/7, no ignores). R-26 pass; merged T-4 + review; slice-1 ignores removed; engine crate's skeleton allow removed (no warnings). TickReport.intended bare ids: noted, qualify when a multi-project consumer needs it.
- 2026-10-22 R-25 round 2 pass; merged T-29 + review. T-33 → R-28, T-30 → R-29.
- 2026-10-22 Slice-2 dispatch contract on dev (`dispatch.rs` types + `decide` stub, `dispatch_contract.rs` 5 ignored). T-35 ready.
- 2026-10-22 Slice-2 `plan apply` (queue cut) contract on dev: `plan.rs` stub + `plan_apply_contract.rs` (5 ignored). T-36 draft until T-33 merges.
- 2026-10-22 R-27 pass (2 P3 strictness gaps → T-37). T-31 ignores removed; merged T-31 + review; workspace green.
- 2026-10-22 R-28 pass: merged T-33 + review, its slice2 tests un-ignored; T-36 ready. R-29 fail: T-30 merges textually but does not compile with T-29 (duplicate test consts) → rework (runtime M-5); hand-in rule now merges dev and builds (L-33). T-32 → R-30.
- 2026-10-22 T-35 → R-31.
- 2026-10-22 T-30 rework → R-29 round 2.
- 2026-10-22 R-30 pass (P3s). Merged T-32 + review; MSRV bumped to 1.89 (File::try_lock; R-30 question). T-34 → R-32.
- 2026-10-22 R-29 r2 pass, R-31 pass. Merged T-30, T-35 + reviews; T-35 ignores removed; workspace green. **Slice-1 CLI complete** (send, reply, home, run). R-31 decisions: continue-preconditions (task Waiting/held/verifying) are enforced when building the LaneView (attempt = None then); a Delivered attempt on an idle seat past cooldown gets Continue like Started — both in the next runtime task.
- 2026-10-22 runtime lowready → slice-2 delivery contract on dev (`deliver_contract.rs`, 3 ignored); T-39 ready.
- 2026-10-22 R-32 pass (P3 format fixed at merge). Merged T-34 + review; its tests un-ignored; T-40 ready. T-36 → R-33, T-38 → R-34.
- 2026-10-22 T-39 → R-35. Lane-view contract on dev (`lane_view_contract.rs`, 3 ignored); T-41 drafted (after T-40).
- 2026-10-22 Slice-2 tasks contract on dev (`gear_app::tasks`, `tasks_contract.rs` 5 ignored). T-42 ready, T-43 after T-42.
- 2026-10-22 R-34 pass; merged T-38 + review. T-37 → R-36.
- 2026-10-22 R-35 pass: T-39 merged via merge-task.sh (first use). R-33 fail: T-36 rework (data M-3: every contract field opens a version; base check first). R-35 question → T-44 (stale delivery voided).
- 2026-10-22 R-36 pass; T-37 merged (merge-task.sh).
- 2026-10-22 runtime starved (T-41, T-44 wait on T-40) → data M-4: T-40 before the T-36 rework.
- 2026-10-22 T-6 (captain's own task) handed in → R-37: the captain's work is reviewed like any lane's.
- 2026-10-22 T-36 rework → R-33 round 2 (data did the rework before reading M-4; T-40 next).
- 2026-10-22 R-37 fail (2 P2 in SKILL.md: lane write rule vs 17; `blocked` arguments). Captain reworked T-6 → round 2.
- 2026-10-22 T-40 → R-38 (critical path: unblocks runtime T-41, T-44).
- 2026-10-22 R-37 round 2 pass; T-6 merged (SKILL.md v1, `--skill conventions`).
- 2026-10-22 R-33 round 2 pass; T-36 merged (`plan apply` queue part; its contract un-ignored).
- 2026-10-22 R-38 pass; T-40 merged; T-41, T-44 ready (runtime unblocked).
- 2026-10-22 R-33 r2 questions: evidence fields absent = none, goal/accept absent = keep (documented in plan.rs); packet acceptance narrowed to own ignored tests (L-36, T-41/42/43 packets fixed). Store skeleton allow removed.
- 2026-10-22 T-42 → R-39 (data's next tasks all depend on it).
- 2026-10-22 Captain: `SendArgs.candidate` added (all constructors updated); result-send contract `result_contract.rs` (2 ignored); T-45 drafted (after T-42). CLI `--candidate` mapping is a runtime follow-up once T-45 lands.
- 2026-10-22 T-41 → R-40.
- 2026-10-22 T-44 → R-41.
- 2026-10-22 Dispatch-tick contract on dev (`dispatch_tick_contract.rs`, 3 ignored; packet_ref/task_branch by captain). T-46 drafted (after T-41, T-42).
- 2026-10-22 R-39 pass; T-42 merged; T-43, T-45 ready.
- 2026-10-22 R-41 pass; T-44 merged. Question → T-47 (missing attempt voided; captain adjusted deliver_contract to record the attempt).
- 2026-10-22 R-40 pass (P3: a blocked view also clears pending_op — harmless, decide returns None without attempt or ready tasks). T-41 merged; T-46 ready.
- 2026-10-22 T-43 → R-42.
- 2026-10-22 Captain: `ApplyResult.settled` + verdicts contract (`plan_verdicts_contract.rs`, 2 ignored); T-50 drafted (after T-43).
- 2026-10-22 T-47 → R-43, T-46 → R-44.
- 2026-10-22 Delivery confirmation designed: a Deliver op is confirmed by a view whose shown_task/attempt match (ViewRecord fields already exist); home records them. Contracts on dev; T-48 (runtime), T-51 (data) ready.
- 2026-10-22 R-42, R-44 pass (no findings). Merged T-43 (settle), T-46 (plan/intend dispatch); T-50 ready. T-45 → R-45.
- 2026-10-22 Slice-2 acceptance written: `slice2_dispatch.rs::full_work_loop` (plan → assign → no resend → view → confirm → result → captain rung → verdict pass → quiet), ignored for T-49.
- 2026-10-22 R-43 pass; T-47 merged.
- 2026-10-22 T-48 → R-46.
- 2026-10-22 Deliveries point at `gear --show T-n` → packet rendering contract (`packet_contract.rs`, 2 ignored); T-53 ready (data); CLI `--show` follows.
- 2026-10-22 R-45 pass; T-45 merged (result submits the attempt); T-52 ready.
- 2026-10-22 T-50 → R-47.
- 2026-10-22 R-46 fail (P2: other op kinds would be budget-bumped and flag seats). T-48 → rework (runtime M-6).
- 2026-10-22 T-52 question (a captain-owned test it invalidates) → test moved to `admin init` on dev; answered M-7.
- 2026-10-22 T-51 → R-48; T-48 rework → R-46 round 2.
- 2026-10-22 T-52 → R-49.
- 2026-10-22 R-47 pass (stale doc P3s fixed at merge), R-46 round 2 pass. Merged T-50 (verdicts), T-48 (delivery confirmation). T-49 waits only on T-51.
- 2026-10-22 R-48, R-49 pass. Merged T-51 (home shows current task), T-52 (cli plan apply, --candidate). T-49 ready — the last piece of the slice-2 acceptance.
- 2026-10-22 T-53 → R-50. merge-task.sh now also parks the reviewer seat.
- 2026-10-22 Staff (park/hold/retire) contract on dev (`plan_staff_contract.rs`, ignored; `ApplyResult.staffed`); T-55 ready. Temp workers and hold release come later.
- 2026-10-22 T-49 (slice-2 acceptance) → R-51.
- 2026-10-22 Found: `gear run` used an empty profile map → every seat fail-closed, nothing dispatched in a real run. T-56 (builtin profiles) ready. Packet template made generic.
