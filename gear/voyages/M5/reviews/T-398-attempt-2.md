# Review of T-398

Status: done
Verdict: pass
Reviewed: review/t398 @ 902019d6   Reviewer: Codex   Round: 2
Contract: v1

## Mechanical acceptance

- `make check CRATE=gear-cli`: exit 0; 168 passed, 0 skipped; zero build warnings. Build 5m48s including shared package/build-lock waits; tests 16.841s.
- `make check CRATE=gear-engine`: exit 0; 745 passed, 0 skipped; zero build warnings. Build 4m57s; tests 331.256s.
- Allowed files: ok. Against origin/dev, production edits are only CLI main.rs/build.rs and engine release.rs/review.rs; the fifth file is the normal task report. Other files in the local-dev merge-base diff are inherited integration changes.
- `git merge-tree --write-tree dev 902019d6`: exit 0, clean against dev e90ff06c. `git diff --check dev...902019d6`: clean.
- Candidate source untouched; review commits contain only review artifacts. Prior review-only commits were preserved before pushing this review.

## Spec compliance

Earlier blocking F1 is resolved: build.rs asks Git for HEAD, the symbolic branch ref and packed-refs paths, watches the existing paths plus build.rs and src, and retains the unknown fallback. The new embedded-SHA unit test passed.

Independently reproduced the earlier commit/rebuild steps with the candidate's unmodified build.rs and repository Makefile in a minimal external gear-cli workspace. The initial build and a source-only commit/rebuild on the same branch both passed a comparison of GEAR_BUILD_SHA with current Git HEAD. Repeated the initial build and source-only commit/rebuild in a linked worktree; both passed too. Four checks total, all using `make check CRATE=gear-cli`, without cleaning. Fixture: /var/folders/vw/chb_f3w93l7dbmt13hw3ztjw0000gp/T/t398-recheck-pb1rkqvl (linked sibling suffix -linked).

The startup version/build test and once-per-run scratch-binary change notification test passed. Release logging tests for caller/reason, captain release and job-gone passed. The integration adjustment passes caller through the release_seat wrapper and standby-agent-gone path introduced by T-399; it retains one log line per release. No new behavior or source-scope deviation found in that adjustment.

## Findings

None. Earlier F1 is resolved; no new blocking or logged findings.

## Set aside

- Existing release paths outside release_one, including cancel_start and release_dead_temps, remain outside the contract's explicit logging scope.
- Verdict-to-Standby is not a seat release. Binary replacements retaining mtime are outside the specified detector.
- Projects without captains have no captain inbox recipient. Thread-local release logging is drained by the single-threaded CLI tick loop, including failed ticks.
- Runtime Git HEAD changing after a binary was built is outside the new unit test's build-time assumption; review HEAD remained stable through the check.
- No broader workspace gate was run; the merge gate owns `make gate`.

## Questions for the captain

None.

## Verdict

Pass. Required mechanical acceptance and independent incremental-rebuild checks pass, with no error in normal use found under the DELIVERABLE bar.

## Friction

Required compilation, shared package/build-lock contention and the existing engine suite dominated the review time. The review push initially encountered prior review commits and was resolved by preserving those review-only commits. The environment cwd was stale, so commands used the actual review/t398 worktree explicitly.
