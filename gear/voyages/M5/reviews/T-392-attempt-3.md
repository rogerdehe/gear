# Review of T-392

Status: done
Verdict: pass
Reviewed: review/t392 @ 8f62568d   Reviewer: Codex   Round: 3
Contract: v2

## Mechanical acceptance

- `make check CRATE=gear-app`: exit 0; 544 passed, 0 skipped; zero build warnings. Build 2m27s; tests 77.372s.
- `make check CRATE=gear-cli`: exit 0; 166 passed, 0 skipped; zero build warnings. Build 4m09s including shared build-lock wait; tests 22.483s.
- Allowed files: ok. T-392 production changes remain in admin.rs, CLI main.rs, model error.rs and captain.md. Staffing types already implement PartialEq. Test fixtures supply the new optional argument; acceptance tests remain in the crates' test directories. Other files in the older local-dev merge-base diff are previously merged integration work, not additional T-392 changes; the origin/dev diff isolates the task's allowed scope.
- `git merge-tree --write-tree dev 8f62568d`: exit 0, clean against current local dev 5a9a9004. `git diff --check dev...8f62568d`: clean.
- Candidate source untouched. Review branch preserves prior review-only commits and adds this review artifact.

## Spec compliance

Attempt 3 resolves the integration conflict from attempt 2 by retaining both test blocks in admin_init_contract.rs: the staffing refusal test and dev's three config change-list tests. All four passed in this review's CLI run. The added helper argument is supplied consistently to the retained calls.

The staffing implementation and staffing regression suite are unchanged from the passed attempt-2 candidate. The required refusal, unchanged config version, authorized version increment and starred staff decision, nonstaff change, no-departure behavior, blank-user rejection, complete model argument/context messages and lane-order comparison all passed in the app suite. The CLI consent flag and exact captain instruction remain present. Earlier findings F1 and F2 remain resolved.

## Findings

None. No new blocking or logged findings.

## Set aside

- Changes inherited by merging dev are existing integration work; the rework requested preserving both sides, not redesigning them.
- Existing captain/session binding and lane-removal restrictions remain consistent with applying authorized changes as today.
- Corrupt stored configs, manual database edits and deliberate fabrication of nonempty user words remain outside normal use and this merge rework.
- Latest departed voyage plus project-wide nonterminal tasks matches the supplied predicate.
- No broader gate or workspace tests were run; the merge gate owns `make gate`.

## Questions for the captain

None.

## Verdict

Pass. Both required checks pass, the merge conflict is resolved without dropping either behavior, and no finding blocks under the DELIVERABLE bar.

Friction: required compilation and shared build-directory contention exceeded the review budget; the initial environment cwd also pointed to the stale direction worktree, so review commands used the actual review/t392 worktree. A non-fast-forward review push was resolved by preserving earlier review-only commits.
