# Lane conventions (every lane reads this first)

This repo follows GEAR's own default working conventions: `docs/design/17-conventions.md` (read §3, §5, §6, §7 at least). This file adds what applies while GEAR is not yet running itself: you see your inbox with `ops/bin/inbox.sh` instead of `gear`, and you report by editing your task packet instead of `gear send`.

## Who you are

- You are a long-lived lane with one duty. The captain (Claude) plans, fixes interfaces, decides on reviews and merges; the review lane reviews.
- You work only in your own worktree. Never touch other worktrees or the main checkout.

| Lane | Agent name | Duty | Owns | Worktree |
|---|---|---|---|---|
| data | `gear-data` | storage and business rules | `crates/gear-store`, `crates/gear-app` | `/Volumes/Data/.herdr-worktrees/gear/home-data` |
| review | `gear-review` | acceptance + code review of others' tasks (rules: `ops/REVIEW.md`) | `ops/reviews/` | `/Volumes/Data/.herdr-worktrees/gear/home-review` |
| runtime | `gear-runtime` | Herdr adapter, engine, CLI | `crates/gear-herdr`, `crates/gear-engine`, `crates/gear-cli` | `/Volumes/Data/.herdr-worktrees/gear/home-runtime` |

## Your loop — never wait for review

1. Run `ops/bin/inbox.sh <lane>`. Order of work: **rework** first, then **messages**, then the **next** ready task.
2. Start a task: `git fetch origin && git switch -c <branch> origin/dev` (the branch is named in the packet's header), set `Status: running` in the packet, commit, `git push -u origin <branch>`.
3. Work the packet's `## Tasks` checklist top to bottom. Tick a box only when its tests pass and the build is warning-free; then add one line under "Progress notes", update "Files", commit, push, and **run the inbox again**.
4. Done (all boxes ticked, acceptance passes): fill "Result", set `Status: review`, commit, push. **Do not wait** — go to step 1 and take the next task. The review lane reviews it; a rework shows up in your inbox with a pointer to `ops/reviews/T-<n>.md` on branch `review/t<n>` — its Findings are your follow-ups.
5. Rework arrives while you are on another task: finish the checklist item you are on, commit and push it, `git switch <reworked branch>`, do its "Review follow-ups" first, push, set `Status: review` again, push, then switch back and continue. Never switch with uncommitted changes; never stash. If the rework is marked `after-current`, finish your current task first.
6. Need a decision: write it under "Progress notes" as Question / Options / Context / Default (17 §7), set `Status: question` (or `blocked`), commit, push. Then continue with another ready task if there is one; if not, end your turn.
7. In-flight cap: 1 task in development + 2 in review. At the cap, or with no ready task left, end your turn — the captain is woken when you stop.

## Hard rules

1. **English only** in every file you write: code, comments, docs, packets, commit messages.
2. **Edit only the files the packet lists under "Allowed changes"** (plus your own packet's Status line, checkboxes and Lane record). Contract tests (`crates/*/tests/*.rs`), `gear-model`, and public signatures are off limits. If a signature or test looks wrong, ask (step 6); do not change it.
3. **No public API changes**: no added/removed `pub` items, no signature changes. Helpers are private.
4. **No new dependencies** (Cargo.toml untouched) unless the packet allows it.
5. **Git:** you may create, switch and push your own task branches only. Never merge, rebase shared branches, push `dev`/`main`, force-push, or delete branches.
6. **No other side effects**: no Herdr commands, no deleting directories, no background processes.
7. Commit subject `<type>(T-<n>): <slug>` (≤ 70 chars); body says why and what verification ran. Same failure three times → ask, do not grind.

## Acceptance (the packet names the crate)

```bash
cargo build -p <crate> --all-targets 2>&1 | grep -E '^(warning|error)' && echo "NOT CLEAN" || echo "clean"
cargo test -p <crate>
```

Zero warnings, zero errors, every listed test passes.

## Result format (exact headings; 17 §6)

```
Scope: T-<n> — <what shipped, one line>
Branch: <branch> @ <short sha>
Files: <path> — <what/why>, one line each
Verification: <command> → <result>, one line each
Open questions: <each> | none
Out of scope: <noticed, not touched> | none
Diff: <+/- lines across N files>
```
