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
