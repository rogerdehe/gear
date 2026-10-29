# Review of T-351

Status: running
Verdict: pending
Reviewed: review/t351 @ 02404ce4   Reviewer: Codex (gear:review-11)   Round: 2
Contract: v1

## Mechanical acceptance

- Required checks running: `make check CRATE=gear-app` and `make check CRATE=gear-board`.
- Allowed files: fetched origin/dev comparison isolates the two permitted production files, tests in their crates, and the task report. Local dev is stale; its requested three-dot diff includes integrated ancestor work.
- Merge checks against dev and fetched origin/dev both exit 0 without conflicts; diff whitespace check is clean.
- Prior review history preserved by merging origin/review/t351; candidate code remains untouched.

## Spec compliance

The move recording and board projection remain intact after the integration merges. ProjectBoard retains both T-335's goal_changes and T-351's moved_out. Both sets of appended voyage-selection tests survive the T-362 merge.

## Findings

Pending completion of mechanical acceptance.

## Set aside

- Rendering and JSON export are outside the explicit model-field contract.
- Moves involving None and repeated historical departures follow the exact prescribed condition and event identity.
- T-353's downstream branch is outside this candidate review.

## Questions for the captain

None.
