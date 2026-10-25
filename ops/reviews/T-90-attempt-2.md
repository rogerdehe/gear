# Review of T-90

Status: done
Verdict: pass
Reviewed: course/t90 @ 18e962bc41ace7c6cb5692d04c909e9aa25f3bbe   Reviewer: Codex (GPT-6)   Round: 2

## Mechanical acceptance

- Build: `cargo build --workspace --all-targets` → exit 0, zero warnings.
- Tests: initial `cargo test --workspace` hit the existing ten-second deadline in `merge::tests::a_sleeping_gate_does_not_block_the_merge_step` while other workspace test processes were running. Its isolated rerun passed (1 passed, 0 failed). Full rerun `RUST_TEST_THREADS=4 cargo test --workspace` → exit 0, 730 passed, 0 failed, zero warnings; all doc tests passed.
- Independent probes: `cargo test --manifest-path /tmp/gear-t90-review-probe/Cargo.toml --test probe` → 6 passed, 0 failed. These include both round-1 reproductions, the three original acceptance assertions, and resolving an unsettled old merge item after the next attempt starts. Only the external probe was extended; candidate code was untouched.
- Allowed files: ok; no explicit allowlist. The nine paths in `dev..18e962b` implement home/reply guidance, settlement, merge-message metadata, contract tests and the task report. The actual rework commit `54ce10f` changes only `tasks.rs`, `verdict_item_contract.rs`, and the report. Other changes since attempt 1 came from integration merges.
- Merges into dev: clean. `git merge-tree --write-tree dev 18e962bc41ace7c6cb5692d04c909e9aa25f3bbe` and the same command against fetched `origin/dev` (`9e6ab953b67baa6b527a90249fd4d322593d9792`) both exit 0. `git diff --check` against both bases passes.
- Reviewer commits contain review files only; the prior remote review history was merged and preserved.

## Spec compliance

- Compliant. The captain sees the filled-in voyage Verdicts command for an unsettled review fail; resolving by reply returns that same command without changing the Verifying task or closing the item. Applying the plan settles the fail and permits resolution.
- The rework recognizes a recorded reopening verdict for the same attempt/candidate, so the historical Failed merge op no longer traps the item. It also limits the handed-in guard to failed reviews and captain decision threads, allowing ordinary task questions to be answered without a verdict.
- Both earlier normal-use errors are fixed at the M3 DELIVERABLE bar. No new blocking behavior found in the rework delta.

## Findings

F1 [P1] resolved — `crates/gear-app/src/tasks.rs:298`: the failed-merge hint now excludes an attempt/candidate with a recorded reopening fail. Independent probes confirm resolution after settlement, including when the next rework attempt starts before the old item is resolved.

F2 [P1] resolved — `crates/gear-app/src/tasks.rs:286`: the handed-in guard now checks failed-review classification or the engine's captain decision thread. The ordinary-question regression and independent reproduction pass; the task remains Verifying with no new verdict.

No new blocking or logged findings.

## Set aside

- The initial timing-test timeout is recorded above. The unchanged deadline passed both alone and in the full rerun with four threads; this does not establish a T-90 regression.
- The repository's new failed-merge test resolves the item before starting the next attempt. The external probe separately verifies the stronger ordering: start the next attempt first, then resolve the still-open old item.
- Missing milestone fallback, historical messages without task metadata, and fail-only hint wording retain the round-1 assessment as edge cases or accepted limitations. No redesign requested in this capped rework round.
- Broader voyage deliverables and already integrated parallel-scan work are outside this re-review.

## Questions for the captain

- None.

## Verdict

Pass — both prior blockers are resolved, the full workspace rerun and build pass, and no new finding breaks the milestone done-sentence or DELIVERABLE bar.
