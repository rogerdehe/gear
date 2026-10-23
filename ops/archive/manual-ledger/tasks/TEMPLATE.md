# T-<n> <title>

Task: T-<n> · Lane: gear-<lane> · Branch: `<lane>/t<n>-<slug>` (you cut it from latest `origin/dev`[, after T-<m> is merged]) · Depends on: <none | T-<m> (why)> · Size: small (≤ 30 min)
Status: ready   <!-- ready → running → review | blocked | question; lane sets it, captain's PLAN.md is authoritative -->

## Goal

<what, exactly; point at the doc comment / contract test that defines it; captain decisions it carries>

## Read first

- `ops/CONVENTIONS.md`, <design docs>
- <source files>; <contract test file> (<k> tests, ignored on dev until this merges)

## Allowed changes

- `ops/tasks/T-<n>.md` (this file): only the `Status:` line, the checkboxes, and the Lane record.
- `<file>`: <which items>.
- If a test you did not write needs changing, ask (Question in Progress notes).

Not allowed: contract tests, any `pub` type or signature, other crates, Cargo.toml.

## Tasks

- [ ] 1. <test names first, then the change>

## Acceptance

```bash
cargo build -p <crate> --all-targets                                   # zero warnings
cargo test -p <crate>                                                  # all pass
cargo test -p <crate> --test <contract file> -- --include-ignored      # your <k> pass
```

## Report

Follow `ops/CONVENTIONS.md` → "Your loop": when done set `Status: review`, push, and take your next task; when stuck ask (step 6).

## Lane record

Only the lane edits this section, the checkboxes above, and the `Status:` line. Tick a box only after its tests pass and the build is warning-free.

### Progress notes

### Files

### Result

## Review follow-ups
