# Review of T-362

Status: running
Verdict: pending
Reviewed: review/t362 @ 00bc3867   Reviewer: gear-review-11 (Codex)   Round: 2
Contract: v4

## Mechanical acceptance
- `make check CRATE=gear-board`: running.
- Allowed files: implementation commits 404724c8 and 377d9055 affect only frame.rs, lib.rs, crate tests, the permitted prototype/Tasks goldens, and required reports.
- Merge: `git merge-tree --write-tree origin/dev 00bc3867` exits 0 against 63286b71.
- Prototype regeneration: all six Tasks .txt/.col/.rev files independently regenerated under target/tmp match byte for byte.

## Spec compliance
- Attempt 1 F1: the Tasks rows now use project.shown.departure, with regressions for an older landed voyage and a landed-only project.
- Attempt 1 F2: contract v4 explicitly permits the two T-356 exclusions and anchored view comparison.

## Findings
None established; acceptance still running.

## Set aside
- Existing header height and full-frame Tasks origins belong to T-356 under contract v4.
- Existing lane ordering and annotation/time overlap were considered in attempt 1; the departure-only delta does not change them.

## Questions for the captain
None.
