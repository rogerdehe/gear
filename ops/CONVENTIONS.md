# Lane conventions (every lane reads this first)

This repo follows GEAR's own default working conventions: `docs/design/17-conventions.md`. Read it once. This file adds what applies while GEAR is not yet running itself (you report by editing your task packet instead of `gear send`).

## Who you are

- You are a long-lived lane with one duty (below). The captain (Claude) splits work, fixes interfaces, reviews and merges.
- Each task has a packet: `ops/tasks/T-<n>.md`. Read all of it before touching code.
- You work only in your own worktree, on the task branch the captain created for you. Do not switch branches; do not touch other worktrees or the main checkout.

| Lane | Agent name | Duty | Owns | Worktree |
|---|---|---|---|---|
| data | `gear-data` | storage and business rules | `crates/gear-store`, `crates/gear-app` (later: plan, tasks) | `/Volumes/Data/.herdr-worktrees/gear/home-data` |
| runtime | `gear-runtime` | Herdr adapter and engine | `crates/gear-herdr`, `crates/gear-engine`, `gear run` | `/Volumes/Data/.herdr-worktrees/gear/home-runtime` |

## Hard rules

1. **English only** in every file you write: code, comments, docs, reports, commit messages.
2. **Edit only the files the packet lists under "Allowed changes".** Contract tests (`crates/*/tests/*.rs`), `gear-model`, and public signatures are off limits. If a signature or test looks wrong, say so in your report and stop.
3. **No public API changes**: no added/removed `pub` items, no signature changes. Helpers are private.
4. **No new dependencies** (Cargo.toml untouched) unless the packet allows it.
5. **No external side effects**: no `git push`, no merge/rebase, no branch deletion, no Herdr commands (`herdr agent …`, `herdr pane …`, `herdr worktree …`), no deleting directories, no background processes.
6. **Small commits**: commit each compiling step. Subject `<type>(T-<n>): <slug>` (≤ 70 chars), body says why and what verification ran.
7. Same failure three times → stop and report `blocked`. Do not grind.

## Acceptance (the packet names the crate)

```bash
cargo build -p <crate> --all-targets 2>&1 | grep -E '^(warning|error)' && echo "NOT CLEAN" || echo "clean"
cargo test -p <crate>
```

Zero warnings, zero errors, every listed test passes.

## Working a task

- Your task list is the rows for your lane in `ops/PLAN.md` (in order); you only ever work the one the captain sent you.
- Your dev list is the `## Tasks` checklist in the packet. Work it top to bottom; if `## Review follow-ups` has open items, do those first.
- Tick a box only when its tests pass and the build is warning-free. After each box: add one line under "Progress notes", update "Files", commit.
- When you start, set `Status: running` and commit.

## Reporting

You never message the captain. You report by editing your packet and ending your turn — the captain is woken when you stop.

- **Done:** every box ticked, acceptance passes → fill "Result" (format below), set `Status: review`, commit, end your turn.
- **Stuck / need a decision:** write the question under "Progress notes" in the 17 §7 shape (Question / Options / Context / Default), set `Status: blocked` or `Status: question`, commit, end your turn.

Result format (exact headings; see 17 §6):

```
Scope: T-<n> — <what shipped, one line>
Branch: <branch> @ <short sha>
Files: <path> — <what/why>, one line each
Verification: <command> → <result>, one line each
Open questions: <each> | none
Out of scope: <noticed, not touched> | none
Diff: <+/- lines across N files>
```
