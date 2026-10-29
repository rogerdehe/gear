# Review of T-392

Status: done
Verdict: pass
Reviewed: review/t392 @ 08535882   Reviewer: Codex   Round: 2
Contract: v1

## Mechanical acceptance

- `make check CRATE=gear-app`: exit 0; 538 passed, 0 skipped; zero build warnings. Build 55.89s; tests 51.020s.
- `make check CRATE=gear-cli`: exit 0; 158 passed, 0 skipped; zero build warnings. Build 1m43s including shared build-lock wait; tests 12.654s.
- Allowed files: ok. Production edits are confined to admin.rs, CLI main.rs, model error.rs and captain.md. Staffing types already derive PartialEq. Tests live in the crates' existing test directories; engine fixture edits only supply the new optional argument. The report is the normal hand-in artifact.
- `git merge-tree --write-tree dev 08535882`: exit 0, clean merge against local dev a64fef02. `git diff --check dev...08535882`: clean.
- Candidate source untouched. Preserved the prior review branch's two review-only commits by merging it before committing this separate attempt-2 review.

## Spec compliance

The attempt-2 delta resolves both earlier findings. F1: app init and CLI check trim and reject empty/whitespace-only words; passing regressions verify StaffLocked, unchanged stored config/version and no starred staff decision. F2: model messages now include arguments and context windows; the passing regression verifies medium/high and 200/300 in both the refusal and authorized decision. Whole-section PartialEq comparisons now drive the guard independently of its explanation, with a passing regression for lane reordering.

The original quota refusal, authorized version increment and single starred staff decision, nonstaff change, no-departure behavior and required captain sentence remain implemented. Required acceptance coverage, including the CLI staffing-check regression, passes in both completed suites.

## Findings

None. Earlier F1 and F2 are resolved; no new blocking or logged findings.

## Set aside

- Nonempty user words cannot be authenticated by this interface; deliberate fabrication is outside the contract.
- Corrupted stored configs and manual database changes remain outside normal use and this rework; no candidate regression identified there.
- Existing captain/session binding and lane-removal restrictions are retained, as required by applying authorized changes "as today".
- Latest departed voyage plus project-wide nonterminal tasks matches the specified predicate; no new voyage-selection design is required.
- Broader workspace checks and `make gate` are reserved for the merge gate; only the two requested make checks were run.

## Questions for the captain

None.

## Verdict

Pass. Mechanical acceptance passes, both prior blocking findings are resolved, and no new finding blocks under the DELIVERABLE bar.
