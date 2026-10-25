# Review of T-78

Status: done
Verdict: fail
Reviewed: review/t78 @ 73adda61f6d40ed3783bad8c114cabd3eb963b8b   Reviewer: Codex   Round: 1

## Mechanical acceptance
- Build: `env -u GEAR_DB -u GEAR_CTX cargo build --workspace --all-targets` exited 0, zero warnings.
- Tests: `env -u GEAR_DB -u GEAR_CTX cargo test --workspace` exited 0: 739 passed, 0 failed, 0 ignored across 22 result summaries (including empty suites), zero warnings. Includes all 15 gear-forge tests. Logs: `/tmp/t78-review-tests.log`, `/tmp/t78-review-build.log` in this review session.
- Allowed files: ok. The packet specifies no narrower allowlist. Inspected the requested `dev..73adda6` diff; T-71 changes and artifacts in it are already integrated at fetched `origin/dev` d75a38b88fb9ed56c2663c415398756c95ae12a3. The remaining 12 paths are the workspace manifests, new forge crate and tests, and T-78 report.
- Merge: `git merge-tree --write-tree origin/dev 73adda61f6d40ed3783bad8c114cabd3eb963b8b` exited 0, clean. `git diff --check dev..73adda6` passed.
- Independent probes: a temporary integration target exercised a stateful fake draft PR, a successful queued merge response, and an exited child with an inherited output pipe. Final run `env -u GEAR_DB -u GEAR_CTX cargo test -p gear-forge --test review_probe -- --nocapture --test-threads=1` passed all three behavior-confirmation probes, zero warnings. The target was removed. The candidate's tracked source is unchanged; review commits contain only this file.

## Spec compliance
- The public Forge trait, typed results/errors, GitHub implementation, Local no-op implementation, explicit repositories, prompt suppression, issue JSON parsing, draft claims, and CLAIM/COMPLETION helpers are present. The tests pin all five method command lines and the four error variants.
- URL parsing for `gh pr create` is a reasonable exception to JSON output: the installed gh 2.97.0 command does not offer `--json`. Mutating commands without structured output need no fabricated JSON support.
- The normal draft-claim-to-merge path is incomplete (F1). Merge results do not distinguish queued work (F2), and the timeout does not cover output collection after child exit (F3). F2/F3 are logged edge cases under DELIVERABLE, rather than additional rework requests.
- Engine synchronization, config, import commands and live sandbox orchestration belong to T-79/T-80/T-81; their absence is not charged to this crate task.

## Findings
F1 [P1, blocking] A claim opened by this crate cannot land through its merge method — `crates/gear-forge/src/github.rs:224`. `open_claim` always supplies `--draft`, while `merge` only invokes `gh pr merge`; nothing in Forge makes that PR ready. GitHub refuses to merge a draft, so the ordinary claim → hand-in → verdict → merge path fails with Error::Failed and requires an external manual readiness change. The stateful fake independently reproduced this sequence. This breaks done-sentence (4), and the DELIVERABLE bar requiring errors in normal use to be fixed. Smallest fix: in GitHub::merge, query `gh pr view <n> --repo <repo> --json isDraft`, parse it with a typed error, and, when draft, call bounded `gh pr ready <n> --repo <repo>` before the existing merge (roughly 20–30 production lines); keep readiness idempotent for retries/already-ready PRs. Add a stateful fake-gh test covering open_claim followed by merge, plus readiness failure and an already-ready PR. Keep opening claims as drafts. GitHub's documented rule: https://docs.github.com/en/pull-requests/reference/pull-requests#draft-pull-requests.
F2 [P2, logged, nonblocking] A successful CLI exit is reported as a completed merge even when the PR is only queued — `crates/gear-forge/src/github.rs:234`. `gh pr merge` can successfully enqueue a PR or find it already queued; GitHub CLI's implementation returns success for those states. The independent fake returning exit 0 and an already-queued stderr message produced `MergeResult { merged: true, message: "" }`. A caller can therefore advance landing/issue closure before any merge. The public MergeResult docs also say already-merged is not a new merge, but the implementation cannot distinguish that case either. Suggested follow-up: query JSON PR state after the mutation and report merged only for confirmed MERGED, with a queued/pending result otherwise. This is logged as a merge-queue/retry edge case for this phase; no queue requirement is in this task. Source: https://github.com/cli/cli/blob/trunk/pkg/cmd/pr/merge/merge.go (inMergeQueue, merge, and RunE).
F3 [P2, logged, nonblocking] Output collection can outlive the configured timeout — `crates/gear-forge/src/github.rs:118`. Once try_wait observes child exit, unbounded reader.recv waits for pipe EOF without checking the deadline. Reproduction: fake gh runs `/bin/sleep 8 &`, prints `[]`, and exits 0; with a 3-second timeout, issues returned Ok([]) after 8.76 seconds. A descendant retaining a pipe indefinitely would hold the caller indefinitely. Suggested follow-up: use recv_timeout with the deadline's remaining duration and return Error::Timeout on expiry. The same reader drains stdout before stderr, so a child filling stderr while stdout stays open can spuriously time out; drain the pipes concurrently when touching this runner. These descendant/large-stderr cases are logged edge cases under DELIVERABLE, not additional rework.

## Set aside
- The fake-gh tests validate command construction, not GitHub server behavior. The draft rule was separately checked against official GitHub documentation; merge queue semantics against upstream CLI source and installed help. No live GitHub mutation was needed for this review.
- CLAIM and COMPLETION match the literal shapes required by the packet. Marker value escaping, hostile input, additional merge methods and pagination beyond the requested limit are not demanded by this task; no normal-use defect established.
- Local's zero-number sentinel and no-op methods satisfy the local-only contract. `PrRef::is_open` denotes existence in this API, not a queried remote lifecycle state; naming alone is not flagged.
- Automatic issue-closing keywords for a nondefault base need the explicit close_issue call supplied here and planned in T-80; no missing integration finding.
- F2 and F3 are retained explicitly as logged findings. No redesign, new retry mechanism, or queue support is required in this rework.

## Questions for the captain
None. The milestone and DELIVERABLE bar are explicit in `gear/voyages/M3.md`.

## Verdict
Fail for F1 only. Required build/tests and merge checks pass. Fix the normal draft readiness transition; F2 and F3 remain logged under the phase's edge-case policy.
