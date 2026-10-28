# Review of T-228 — attempt 2

Status: done
Verdict: pass
Reviewed: mast/t228 @ 65a863ad   Reviewer: review-11 (Codex)   Round: 2
Contract: v2. Bar: STABLE (M4).

## Mechanical acceptance
- Build: the required `cargo test -p gear-app -p gear-cli` compiled the app and CLI test targets with zero warnings (13.41 seconds).
- Tests: the command exited 0: 28 app unit, 403 app integration, 86 CLI unit, and 39 CLI integration tests passed (556 total); doc tests passed. Existing init tests remain unchanged and pass. The new `check_creates_no_default_store_without_gear_db` regression passes, as do both original check-mode tests.
- Allowed files: `git diff --name-only origin/dev...65a863ad` contains only the four requested implementation/test/guide files and `ops/reports/T-228.md`. The rework commit changes only CLI startup, its contract test, and the report. Other differences since attempt 1 are merged integration work.
- Merge: after fetching origin, `git merge-tree --write-tree origin/dev 65a863ad` exited 0; tree `74bef9ee6680f4b97d4e1678e8ab8bcdd9c643ef`.
- Full workspace build and `scripts/test.sh` deliberately not run: contract v2 assigns them to the merge gate.
- Review commits contain only this review; candidate code untouched.

## Spec compliance
- F1 resolved at `crates/gear-cli/src/main.rs:290`: `main` skips `default_store_path` for exactly `AdminOp::Init { check: true, .. }`. Other commands retain their startup path, and check mode retains its engine-start exclusion.
- The added regression snapshots a scratch HOME after skill installation, unsets GEAR_DB and caller identity, and asserts the same path tree after both successful validation and malformed TOML. This directly covers the prior failure.
- Independently repeated the prior reproduction with the freshly built candidate: valid config exits 0 with `config ok: project p, 0 lanes, 0 reviewers`, and HOME gains no paths. Malformed TOML exits 1 with `ERROR BAD_FORMAT`, leaves HOME unchanged, and creates no `.local/state/gear` directory.
- The app extraction, output formats, error parity, no-admin operation, and guide sentence reviewed in attempt 1 are unchanged by this rework.

## Findings
None. Prior blocking finding F1 is resolved; no new blocking or logged findings.

## Set aside
- Re-review is limited to the rework delta and F1 verification. Integration changes merged between candidates belong to their own tasks.
- The report has a trailing blank line; this has no effect on the milestone or normal operation and is not a finding.
- Existing repo/session validation scope, requested validation ordering, and captain-guide sentence placement remain as assessed in attempt 1; no new issues introduced.

## Questions for the captain
None.

## Verdict
Pass: F1 is fixed, required mechanical acceptance passes, and no new findings remain. The full workspace gate remains the merge gate's responsibility under v2.
