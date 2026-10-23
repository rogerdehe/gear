# 12 Output Sampling and Acceptance

Implementation: [tasks.rs](../../crates/gear-app/src/tasks.rs).
Invariants upheld: completion is bound to the contract and the artifact version; unknown is not treated as success; file changes are only a clue.

## Output sampling (engine, every 20 minutes, once per seat; does not hold the SQLite write lock)

Sampling records three things separately (git subprocesses only, 10-second timeout, never walks the whole tree):

- `head`: `git rev-parse HEAD`;
- `status_digest`: hash of `git status --porcelain` (paths and states);
- `content_digest`: hash computed over the `git diff HEAD` output, the **task evidence files** among untracked files, and the contents of untracked files listed by `git status` (with a total byte cap) — so a modified file that is modified again is still detected. Over the cap or failure → `complete=0`, not treated as no change.

| finding | Condition | Result |
|---|---|---|
| `progress` | head / content_digest / evidence files changed | Recorded; used by 09 to reset the no-progress count |
| `missing_evidence` | Past the contract's `evidence-due` and the files listed in `evidence-files` still do not exist | Give the captain one merged item "T-17 is missing expected output, needs check" |
| `unreviewed` | ≥ 2 hours since the last inspection and there were changes in between | Same as above, "T-17 not inspected for 2 hours" |
| `sample_failed` | git failed / worktree missing / incomplete | Mark as observation failure; do not infer the work stopped |

Tasks whose contract does not specify `evidence-files/evidence-due` only get progress/unreviewed; no deadline is guessed.

**Inspection**: what the captain sees with `gear --show T-17 --diff` is **the diff of the current worktree against the HEAD of the last inspection baseline** (only HEAD and digests are stored; it does not claim to be able to restore the previous uncommitted snapshot). The captain records a judgement with `gear reply <item> --next-check <time> "judgement…"` (written to `reviews_of_work`: time, baseline HEAD, content_digest, judgement, next check), and uses `gear send --to <lane> --kind instruction` when a correction is needed. If the sample is incomplete or has just changed at inspection time, prompt a re-sample.

## Acceptance

- Lane runs `gear send --kind result --task T-17 --attempt 3 --contract 4 --candidate <commit or artifact version> "Report: <path>"` → attempt SUBMITTED, task VERIFYING (06).
- The captain verifies it personally or starts a temp reviewer (10). **Evidence files must carry a header**:

```text
gear-verdict: task=T-17 attempt=3 contract=4 candidate=abc1234 result=pass
checks: <which verifications were run>
```

- The captain writes the conclusion in the draft's `# Verdicts` → `plan apply` validates (05): attempt SUBMITTED, candidate matches, contract version equals the current one; pass lines: the candidate matches the adopted contract, the contract equals the current contract after this pending apply, the evidence header matches and `result=pass`; fail lines: only target the candidate and contract originally submitted by that attempt, give a rework reason, and are not required to equal the latest contract (see 05); tasks that need integration acceptance also require integration evidence. Pass → 06 `settle(passed)` → DONE.
- Fail: write `fail rework="…"` in `# Verdicts` → attempt REJECTED, task READY, rework instructions go into the task packet; then a new attempt is dispatched normally; for temp-worker tasks, resume the original temp worker per 10. `gear reply` only conveys opinions and does not change task state.
- Non-code tasks: the candidate may be a pinned version reference to a report/artifact; a commit is not mandatory.
- Candidate or contract changes → old evidence becomes invalid.
- Rework of the same item on a completed task: draft `state=ready` + `reopen:`, keeping the historical passing conclusion (05).

## Test points

A modified file modified again is detected; untracked evidence file changes are detected; an incomplete sample does not report "no change"; missing evidence only fires when the contract has a deadline; evidence header result=fail does not allow DONE; candidate/contract mismatch is rejected.
