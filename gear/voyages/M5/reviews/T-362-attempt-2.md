# Review of T-362

Status: done
Verdict: pass
Reviewed: review/t362 @ 00bc3867   Reviewer: gear-review-11 (Codex)   Round: 2
Contract: v4

## Mechanical acceptance
- Build/tests: `make check CRATE=gear-board` passed with zero warnings: 189 passed, 0 skipped. Compilation 49.40s; tests 4.107s. Includes both departure regressions, Down/Enter opening the second task row, 40-task scrolling with the legend on H - 2, and the anchored Tasks golden comparison.
- Allowed files: implementation commits 404724c8 and 377d9055 affect only frame.rs, lib.rs, crate tests, the permitted prototype/Tasks goldens, and required reports.
- Merge: `git merge-tree --write-tree origin/dev 00bc3867` exits 0 against 63286b71.
- Prototype regeneration: all six Tasks .txt/.col/.rev files independently regenerated under target/tmp match byte for byte.

## Spec compliance
- Attempt 1 F1 resolved: the Tasks rows now use project.shown.departure exactly. Both regressions yield starts +0h10m for a task starting 600 seconds after its own voyage departure: older landed M1 alongside active M2, and the landed-only project. This also avoids elapsed-minute truncation.
- Attempt 1 F2 resolved by contract v4: the two T-356 exclusions and anchored view comparison are explicitly permitted. The passing anchored test allows only prototype-data entries and the lane-colour swap.
- Graph drawing, picks, dependency marks, viewport boundary, legend and selected-task panel remain as reviewed in attempt 1; the rework production delta only changes voyage_departure. Text-only graph::lines and its project_lines use remain.

## Findings
None.

## Set aside
- Existing header height and full-frame Tasks origins belong to T-356 under contract v4.
- Lane ordering was considered in attempt 1 against reported captain direction M-5872; the rework does not change it.
- Annotation/time overlap and fixed columns follow the prototype design. Tall-canvas and narrow-terminal limits are inherited; no new normal-use failure established.
- make gate and user view-by-view approval belong to later workflow stages; only the requested crate check was run.

## Questions for the captain
None.

## Verdict
Pass: both earlier findings are resolved against contract v4, mechanical acceptance passes, and no new blocking or logged finding was established.
