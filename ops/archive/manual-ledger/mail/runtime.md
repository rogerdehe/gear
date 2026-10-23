# Mail to lane runtime

The captain appends messages as `### M-<n> [open] T-<n>: <subject>` followed by the body, and marks them `[closed]` once handled. Lanes only read this file (via `ops/bin/inbox.sh`).

### M-1 [closed] T-18: rework — use `Tx::project_ids()`, drop the private registry

Your question (projects cannot be enumerated): option (b). `Tx::project_ids()` is now on `dev` (ascending ids from the `projects` table, contract test `project_ids_lists_every_project_in_order`). Rework T-18: rebase/merge `origin/dev` into your branch, make `recover_on_start` iterate `project_ids()`, and remove `remember_project` / `known_projects` and their meta-table keys. T-4's tick uses the same method. Then set `Status: review` again. This was decided before review, so no reviewer was started for the first hand-in.

### M-2 [closed] T-19: rework — merge origin/dev (R-12 P1)

Read `origin/review/t19:ops/reviews/T-19.md`. P1: the branch conflicts with dev in `crates/gear-engine/src/ops.rs` imports (T-18 landed after your cut). `git merge origin/dev` into `runtime/t19-execute` (no rebase), keep dev's imports plus yours, re-run acceptance, correct the progress note. Captain decision on the drain question: `execute_intended` handles doorbell ops only for now — make its doc comment say so (other kinds come with their own packets). The P3s are optional; the void-reason test is welcome if it fits in a few minutes. New rule for every hand-in: `ops/CONVENTIONS.md` → Acceptance (merge-tree check).

### M-3 [closed] T-21: rework — use `SeatState::expects_agent()` (R-18 P1)

Read `origin/review/t21:ops/reviews/T-21.md`. Delete the private `observe::expects_agent` and call `state.expects_agent()` from gear-model (Retired/Missing/NeedsCheck/Unknown seats must be `NotExpected`); add a test with a Retired seat whose pane hosts an agent → `NotExpected`. Merge `origin/dev` first, then hand in.

### M-4 [closed] T-29: rework — reject `--need` too (R-25 P2)

Read `origin/review/t29:ops/reviews/T-29.md`. Captain answer: yes, `--need` is a slice-1-unsupported flag — reject it with `BadFormat` naming the flag, like the other four (my packet missed it). Extend `unsupported_flag_is_bad_format` to cover it. Merge `origin/dev` first.

### M-5 [closed] T-30: rework — merge origin/dev and make it compile (R-29 P1)

Read `origin/review/t30:ops/reviews/T-30.md`. `git merge origin/dev` into `runtime/t30-cli-home`: the merged `mod tests` defines `T0`/`CAP_PANE`/`LANE_PANE` twice (yours and T-29's) — keep one set; use T-29's `open_herdr()` / `now_ts()` in `cmd_home` (P3). Run acceptance on the merged result. Hand-in rule changed: always merge dev and build before `Status: review` (ops/CONVENTIONS.md, L-33).

### M-6 [closed] T-48: rework — skip kinds without an evidence rule (R-46 P2)

Read `origin/review/t48:ops/reviews/T-48.md`. In `confirm_pending`, ops of kinds other than Doorbell/Deliver must be skipped entirely (no budget bump, no flag) as before; add a test that an Onboard op in Submitted is left untouched. Merge `origin/dev` and build before hand-in.

### M-7 [closed] T-52: answer — option (a)

`skill_contract::not_yet_commands_fail_cleanly` now uses `gear admin init x` (on dev). Merge `origin/dev` into your branch, re-run `cargo test -p gear-cli`, set `Status: review`. Good question — exactly the right way to raise it.

### M-8 [closed] T-58: merge origin/dev (conflict with the captain's onboard_queued stub)

`runtime/t58-onboard-plan` conflicts with dev in `crates/gear-engine/src/onboard.rs` because I added the `onboard_queued` stub there afterwards. Merge `origin/dev` into the T-58 branch (keep your `plan_onboard` body and the new stub), re-run acceptance, hand it in again — before continuing T-62.

### M-9 [closed] gear-model now allows Queued → Starting/NeedsCheck for lanes (and Queued → Starting/Active for the captain)

Pushed to dev (found by gear-runtime). Merge `origin/dev` into your branch and use `SeatState::can_transition` for those steps — no raw state write needed.
