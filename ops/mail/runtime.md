# Mail to lane runtime

The captain appends messages as `### M-<n> [open] T-<n>: <subject>` followed by the body, and marks them `[closed]` once handled. Lanes only read this file (via `ops/bin/inbox.sh`).

### M-1 [open] T-18: rework — use `Tx::project_ids()`, drop the private registry

Your question (projects cannot be enumerated): option (b). `Tx::project_ids()` is now on `dev` (ascending ids from the `projects` table, contract test `project_ids_lists_every_project_in_order`). Rework T-18: rebase/merge `origin/dev` into your branch, make `recover_on_start` iterate `project_ids()`, and remove `remember_project` / `known_projects` and their meta-table keys. T-4's tick uses the same method. Then set `Status: review` again. This was decided before review, so no reviewer was started for the first hand-in.
