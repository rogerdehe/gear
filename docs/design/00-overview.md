# GEAR MVP Technical Design: Overview

Basis: [PRD v0.8](../PRD.md), [TECH-MVP](../TECH-MVP.md). This directory breaks the MVP into a module skeleton; each module is reviewed before code is written. Signatures are design conventions and may be renamed during implementation, but responsibility boundaries and contracts must not drift.

## 0. Scope (2026-10-22, user)

GEAR runs one piece of work with several agents: a new product, **or one requirement iterated inside an existing large repo** (named after the requirement, on its own integration branch). Every project starts with a kickoff aligned with the user — installed agents and models, lanes, a per-role model chain with fallbacks — see 17 §0.

## 1. Two confirmed decisions (2026-10-21, user)

1. **Temp worker resume**: must be supported by the mechanism. **New work opens a new session; when the same piece of work needs the worker back midway, bring back the original temp worker (resume the original session).** When resume is unavailable, fall back to a new session that reads the task packet, explicitly marked as "new-session recovery". See [10-staff.md](10-staff.md).
2. **Captain replacement is only switched manually by the user.** The MVP does no automatic switching and no automatic activation of a standby captain; when the captain is unavailable, GEAR only freezes, displays, and notifies. For the manual switch flow see [03-identity-seats.md](03-identity-seats.md).

## 2. Directory layout

```text
herdr-gear/                      # plugin root (herdr plugin link points here)
  herdr-plugin.toml              # plugin manifest: pane=engine, action=open board
  bin/gear                       # entry script: exec python3 -m gear "$@"
  gear/
    __main__.py                  # subcommand dispatch
    config.py                    # 01 project config loading and validation
    core/
      db.py                      # 02 SQLite, schema, transactions, request replay
      model.py                   # 02 enums and data classes (state machine definitions)
      identity.py                # 03 caller resolution, seats, generations, manual captain replacement
    herdr/
      client.py                  # 04 Herdr CLI wrapper
      profiles.py                # 04 per-CLI observation signatures and resume templates
    plan/
      draft.py                   # 05 plan draft format parsing
      apply.py                   # 05 validation and transactional apply
      render.py                  # 05 PLAN.md / task packet / editable draft generation
    work/
      tasks.py                   # 06 task and attempt state machine
      evidence.py                # 12 evidence, acceptance, output sampling
    mail/
      send.py                    # 07 send contract and routing
      inbox.py                   # 07 thread selection and reply
    engine/
      loop.py                    # 08 main loop and scheduling tick
      observe.py                 # 08 Herdr snapshot and seat reconciliation
      ops.py                     # 08 side effects: intent -> execute -> result
      dispatch.py                # 09 continuation and dispatch (pure judgement)
      doorbell.py                # 07 doorbell
      staff.py                   # 10 standby/park/hold, temp workers, retirement/missing
      review.py                  # 11 review triggers and reminders
      board.py                   # 13 plugin pane text board
    cli/
      home.py                    # 13 gear
      send_cmd.py                # 13 gear send
      reply_cmd.py               # 13 gear reply
      plan_cmd.py                # 13 gear plan apply
      admin.py                   # 13 gear admin ... (user ops), gear run
  skills/gear/                   # 16 Agent skill (SKILL.md + references), released with each version
  templates/                     # lane instructions, captain instructions (generated per project), nudge/doorbell/review prompts
  tests/
```

## 3. Module list and budget

| No. | Module | Files | Product line budget |
|---|---|---|---:|
| 01 | Config and project registration | config.py, cli/admin.py (init part) | 100–130 |
| 02 | Database and transactions | core/db.py, core/model.py | 280–340 |
| 03 | Identity, seats, generations, manual captain replacement | core/identity.py | 170–210 |
| 04 | Herdr adapter | herdr/client.py, herdr/profiles.py | 220–280 |
| 05 | Plan draft, apply, views | plan/*.py | 270–330 |
| 06 | Tasks and attempts | work/tasks.py | 130–160 |
| 07 | Messages: send, inbox, doorbell | mail/*.py, engine/doorbell.py | 250–310 |
| 08 | Engine loop, observation, side effects | engine/loop.py, observe.py, ops.py | 220–280 |
| 09 | Continuation and dispatch judgement | engine/dispatch.py | 150–190 |
| 10 | Staff: standby, temp workers (incl. resume), retirement, missing | engine/staff.py | 260–320 |
| 11 | Review scheduling | engine/review.py | 90–120 |
| 12 | Output sampling and acceptance | work/evidence.py | 120–150 |
| 13 | Command surface, home page, board | cli/*.py, engine/board.py | 200–250 |
| 14 | Plugin packaging and running | herdr-plugin.toml, bin/gear, admin run | 50–70 |
| 16 | Agent skill | skills/gear/ (Markdown, not counted); `gear --skill` counted in 13 | — |
| 17 | Default working conventions (naming, branches, worktrees via Herdr, tasks and commits, review and merge) | [17-conventions.md](17-conventions.md) (spec; enforced by 01, 10) | — |
| | **Total** | | **2,510–3,140** |

After round 15 of review filled in identity fixed context, handover, delivery checking, resume boundaries, and the acceptance loop, the total is 2,510–3,140 lines, **exceeding the 3000-line budget**. This is a design estimate, not a commitment. During implementation, compress presentation first, not behavior: merge scattered files, simplify board and home page formats, plan view formatting; do not remove already-decided behavior such as same-cause merging, fair share, reminder budgets, the single delivery placeholder, or the resume normal path. If it still exceeds after compression, bring concrete numbers back to the user for a decision. Tests, roughly 1,700–2,700 lines, are counted separately; see [15-tests-probes.md](15-tests-probes.md).

## 4. Dependency direction

```text
cli/* ──▶ mail, plan, work, core, herdr(read-only) (commands only make pure data changes; Herdr queries happen outside write transactions)
engine/* ──▶ herdr, mail, plan, work, core (the only executor of Herdr side effects)
mail, plan, work ──▶ core
herdr ──▶ (subprocess only)
```

- `cli` may only use `herdr`'s read-only interface `HerdrReadOnly` (identity fingerprint checks use `agent get`) and never executes Herdr side effects.
- The judgement functions in `engine/dispatch.py` and `review.py` are pure functions: input snapshot and state, output suggested actions; side effects all go through `engine/ops.py`.

## 5. Common conventions

- **Time**: always UTC epoch seconds (int) in the database; converted to local time for display.
- **IDs**: task `T-<n>`, message `M-<n>`, temp worker `R-<n>`, review item `V-<n>`, operation `O-<n>`; incremented per project, never reused.
- **Transactions**: all writes go through `db.tx(conn)` (`BEGIN IMMEDIATE`, busy_timeout 5 seconds). Inside the transaction, re-read and check identity, generation, and version.
- **request_id**: auto-generated for every write command or passed in by the caller; same ID with same arguments returns the original result, same ID with different arguments is an error.
- **Errors**: commands exit non-zero and print one line `ERROR <code>: <reason>`; `<code>` is a fixed enum (see 02). When the outcome is unknown, exit code 3 and output `UNCONFIRMED`. Every write command prints the request_id to stderr before committing, so a lost receipt can be recovered by rerunning.
- **Identity**: managed Agents are all started and onboarded by GEAR and inherit the fixed call context `GEAR_CTX` (03); the latest generation is never looked up by pane.
- **Output**: short human-readable text by default; `--json` gives structured results (also usable by Agents).
- **Invariants**: see PRD §17; each module file marks which ones it is responsible for upholding.

## 6. Data flow (one complete round)

0. The user registers the project with `gear admin init`; the engine starts and onboards the captain and long-lived lanes per config (03).
1. The captain edits `plan.draft.md` → `gear plan apply` (05) → a new plan version is stored, tasks become ready, and pending staff actions are written to operations.
2. Engine loop (08) observes Herdr → continuation judgement (09) → delivers a task pointer to idle lanes (ops).
3. The lane runs `gear` to read the task packet (13) → works → `gear send --kind progress/question/result` (07).
4. The engine finds the captain idle with pending items → doorbell (07).
5. The captain runs `gear` to see a batch of threads → handles each with `gear reply`; if the plan needs changing, back to 1.
6. Engine timers: review triggers (11), output sampling (12), temp worker release/resume and standby expiry (10).

## 7. Review order

Suggested bottom-up by dependency: 02 → 03 → 04 → 06 → 05 → 07 → 08 → 09 → 10 → 11 → 12 → 13 → 01 → 14 → 15.
