# 13 Command Surface, Home Page, Board

Implementation: [main.rs](../../crates/gear-cli/src/main.rs), [lib.rs](../../crates/gear-board/src/lib.rs).

## Commands

Use `gear --help` and subcommand help for the implemented arguments.
The operating instructions are in `gear --skill`; the user's installation
and command guide is [README](../../README.md).

## Home page content

**Lane**: identity line (seat, generation) → current task in one line + task packet path → unread replies/instructions for me (original text) → contract update notices → suggested next command.

**Captain**:
1. Handover/anomaly banner (if any);
2. **Due reviews** and snapshot (11);
3. One line per lane: state, current task, standby/dwell time, most recent output time; temp worker quota;
4. Inbox batch (07): thread, original text, ID;
5. Other: count of unmentioned tasks, items needing check, engine heartbeat.

Total length limit ~120 lines; overflow is written as "N more, `gear --show …`". Reading the home page changes no message/task state; it only records one "view record": seat, generation, time, and the task/attempt/contract versions, message IDs, and related operation IDs **actually shown** this time. The record is written only after output succeeds; if output and recording are interrupted, it counts as not shown. The record only proves the content was fetched; it does not prove the items were handled or the contract adopted. 08 uses it to confirm deliveries/doorbells.

## Board

The board shows the plan as described in PRD §15. Its manifest and key binding
are documented in [README](../../README.md). Closing the board leaves the
background engine running.

## Test points

Argument validation and error codes for each command; home page is read-only; `--json` structure is stable; lanes invoking captain commands are rejected.
