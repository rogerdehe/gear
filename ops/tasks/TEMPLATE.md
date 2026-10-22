# T-<n> <title>

Task: T-<n> · Lane: gear-data · Branch: `data/t<n>-<slug>` (you cut it from latest `origin/dev`, no dependency) · Depends on: none · Size: small (≤ 30 min)
Status: ready   <!-- ready → running → review | blocked | question; lane sets it, captain's PLAN.md is authoritative -->

## Goal

<what, exactly; point at the doc comment / contract test that defines it>

## Read first

- `ops/CONVENTIONS.md`, design/05 (Staff rules), design/10 (seat states)
- `crates/gear-app/src/plan.rs`, `crates/gear-app/src/draft.rs` (`StaffItem`), `crates/gear-model/src/state.rs` (`SeatState::can_transition`)
- `crates/gear-app/tests/plan_staff_contract.rs` (2 tests, ignored on dev until this merges)

## Allowed changes

- `ops/tasks/T-<n>.md` (this file): only the `Status:` line, the checkboxes, and the Lane record.
- `crates/gear-app/src/plan.rs`: `apply`, private helpers.
- If a test you did not write needs changing, ask (Question in Progress notes).

Not allowed: other contract tests, any `pub` type or signature, other crates, Cargo.toml.

## Tasks

- [ ] 1. implement; both contract tests pass; existing plan tests still pass

## Acceptance

```bash
cargo build -p gear-app --all-targets                                            # zero warnings
cargo test -p gear-app                                                           # all pass
cargo test -p gear-app --test plan_staff_contract -- --include-ignored        # your 2 pass
```

## Report

Follow `ops/CONVENTIONS.md` → "Your loop": when done set `Status: review`, push, and take your next task; when stuck ask (step 6).

## Lane record

Only the lane edits this section, the checkboxes above, and the `Status:` line. Tick a box only after its tests pass and the build is warning-free.

### Progress notes

### Files

### Result

## Review follow-ups
