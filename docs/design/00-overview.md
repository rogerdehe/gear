# GEAR MVP Technical Design: Overview

The implementation is the Rust workspace in [crates/](../../crates/).
[PRD](../PRD.md) defines the product; [README](../../README.md) explains the
installed commands. The remaining module notes describe design intent;
current interfaces and table shapes live in the linked Rust sources.

## 0. Scope (2026-10-22, user)

GEAR runs one piece of work with several agents: a new product, **or one requirement iterated inside an existing large repo** (named after the requirement, on its own integration branch). Every project starts with a kickoff aligned with the user — installed agents and models, lanes, a per-role model chain with fallbacks — see 17 §0.

## 1. Two confirmed decisions (2026-10-21, user)

1. **Temp worker resume**: must be supported by the mechanism. **New work opens a new session; when the same piece of work needs the worker back midway, bring back the original temp worker (resume the original session).** When resume is unavailable, fall back to a new session that reads the task packet, explicitly marked as "new-session recovery". See [10-staff.md](10-staff.md).
2. **Captain replacement is only switched manually by the user.** The MVP does no automatic switching and no automatic activation of a standby captain; when the captain is unavailable, GEAR only freezes, displays, and notifies. For the manual switch flow see [03-identity-seats.md](03-identity-seats.md).

## 2. Implementation

- `gear-model`: state transitions, identities and errors.
- `gear-store`: SQLite schema, transactions and request replay.
- `gear-herdr`: Herdr transport, verified CLI profiles and the test fake.
- `gear-app`: configuration, commands, plans, tasks and messages.
- `gear-engine`: observation, dispatch, review, merge and lifecycle actions.
- `gear-board`: the plan board.
- `gear-cli`: command parsing and engine process entry point.

Commands make short database transactions; the engine executes Herdr side
effects. Identity queries happen before write transactions. The plugin manifest
is [herdr-plugin.toml](../../herdr-plugin.toml); [bin/gear](../../bin/gear)
executes the built Rust binary. The board and background engine are separate.

## 3. Common conventions

- **Time**: always UTC epoch seconds (int) in the database; converted to local time for display.
- **IDs**: task `T-<n>`, message `M-<n>`, temp worker `R-<n>`, review item `V-<n>`, operation `O-<n>`; incremented per project, never reused.
- **Transactions**: all writes go through `Store::write` (`BEGIN IMMEDIATE`, busy_timeout 5 seconds). Inside the transaction, re-read and check identity, generation, and version.
- **request_id**: auto-generated for every write command or passed in by the caller; same ID with same arguments returns the original result, same ID with different arguments is an error.
- **Errors**: commands exit non-zero and print one line `ERROR <code>: <reason>`; `<code>` is a fixed enum (see 02). When the outcome is unknown, exit code 3 and output `UNCONFIRMED`. Every write command prints the request_id to stderr before committing, so a lost receipt can be recovered by rerunning.
- **Identity**: managed Agents are all started and onboarded by GEAR and inherit the fixed call context `GEAR_CTX` (03); the latest generation is never looked up by pane.
- **Output**: short human-readable text by default; `--json` gives structured results (also usable by Agents).
- **Invariants**: see PRD §17; each module file marks which ones it is responsible for upholding.

## 4. Data flow (one complete round)

0. The user registers the project with `gear admin init`; the engine starts and onboards the captain and long-lived lanes per config (03).
1. The captain edits `gear/voyages/<milestone>.md` → `gear plan apply` (05) → a new plan version is stored, tasks become ready, and pending staff actions are written to operations.
2. Engine loop (08) observes Herdr → continuation judgement (09) → delivers a task pointer to idle lanes (ops).
3. The lane runs `gear` to read the task packet (13) → works → `gear send --kind progress/question/result` (07).
4. The engine finds the captain idle with pending items → doorbell (07).
5. The captain runs `gear` to see a batch of threads → handles each with `gear reply`; if the plan needs changing, back to 1.
6. Engine timers: review triggers (11), output sampling (12), temp worker release/resume and standby expiry (10).

## 5. Review order

Suggested bottom-up by dependency: 02 → 03 → 04 → 06 → 05 → 07 → 08 → 09 → 10 → 11 → 12 → 13 → 01 → 14 → 15.
