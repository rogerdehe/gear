# Review of T-79

Status: done
Verdict: pass
Reviewed: course/t79 @ c8641f1a5a3c78f2bdc0659a462fe094d04ab841
Reviewer: review-7 (Codex)
Round: 2

## Mechanical acceptance
- Build: `env -u GEAR_DB -u GEAR_CTX cargo build --workspace --all-targets` exits 0, zero warnings. Log: `/tmp/gear-review-t79-a2-build.log`.
- Tests: `env -u GEAR_DB -u GEAR_CTX cargo test --workspace` exits 0: 773 passed, 0 failed, doc tests passed, zero warnings. All 11 github_contract tests passed. Log: `/tmp/gear-review-t79-a2-tests.log`.
- Allowed files: contract v1 gives no explicit allowlist. The requested diff contains config/plan/store implementation, the issue renderer, dependency wiring, TaskRow fixture updates and the report. The current local dev additionally differs by a removed voyage queue line; this is task bookkeeping. The rework delta also carries T-86 changes already present on fetched origin/dev, which are not re-reviewed.
- Mergeability: `git merge-tree --write-tree dev c8641f1` exits 0 against dev 7041b9a (tree 73123564c166f9aa8f48f8ebba1efb9e1b662e15); the same check against fetched origin/dev 2890a9c exits 0 (tree 8ec027f064300a14659ab58297379545c57e767d). `git diff --check dev..c8641f1` passed again against dev 7041b9a.
- Independent standalone Rust probe linked against the candidate's compiled library confirms root identity, local-only behavior through repo_of_lane for a bound child, and the explicit child's identity. Source/executable: `/tmp/gear-review-t79-a2-lanes.rs`, `/tmp/gear-review-t79-a2-lanes`. Candidate code was not edited.

## Spec compliance
- Config supports optional project-root and per-repo GitHub sections, validates repository shape/labels/modes, and resolves absence to local-only. Priority labels map to the five queue priorities. The issue field is validated, stored and preserved across applies. issue_block renders a parser-valid NEW block.
- Earlier F1: resolved. Both merge-test TaskRow constructors now initialize issue; the all-targets build passes.
- Earlier F2: resolved. repo_of_section uses only the child's section; the regression checks root, absent child and configured child. The independent probe additionally checks lanes bound to both children.
- Earlier F3 (logged): resolved. The parser requires positive ASCII digits fitting i64; apply uses checked conversion. The regression rejects zero, plus-prefixed numbers and overflows.

## Findings
None. No blocking or new logged findings.

## Set aside
- The new child regression does not actually bind its lane to the child despite its comment. The independent probe covers that path, and repo_of_lane delegates to the corrected resolver; this does not leave a demonstrated behavior defect.
- The older test comment at github_contract.rs:73 still describes inheritance. Production documentation and behavior are corrected; this minor comment does not warrant rework under DELIVERABLE.
- Checked conversion in apply silently omits values outside i64, but normal apply receives the validated parser output, so that branch is unreachable through the supported input path.
- Full GitHub synchronization, import CLI, PR lifecycle and closing issues remain other tasks' work. This task supplies their config, issue field and rendering helper.
- Existing renderer choices (first nonempty body line, 200-character cap, default must and captain-editable placeholders) and ambiguous multi-priority label precedence were already considered in attempt 1; the rework does not change them.
- Fixture formatting is cosmetic. T-86 behavior already on integration is outside this re-review.

## Questions for the captain
None.

## Verdict
Pass. Mechanical acceptance passes, earlier F1/F2 are resolved, and the previously logged F3 is also fixed. No blocking findings remain.
