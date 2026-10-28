# Review of T-130

Status: done
Verdict: pass
Reviewed: review/t130 @ b49cb87   Reviewer: Codex   Round: 2

## Mechanical acceptance
- Build: `cargo build --workspace --all-targets` passed, zero warnings (0.62 s warm).
- Tests: `scripts/test.sh` passed once, parallel, exit 0: 926 passed, 0 skipped; nextest 57.861 s, script 59 s, build 88 s. All working-lane policy, synchronous-send, refusal fallback and deduplication tests passed. The three added rework tests took 0.050–0.124 s; no test over 10 s was added. Existing slow-test warnings concern unchanged engine merge/parallel tests; total remains below the 300 s red line.
- Allowed files: v2 specifies no path whitelist. Reviewed `dev..b49cb87`, including integration ancestry; the task-specific diff against origin/dev 0dec23e has 14 paths, confined to doorbell implementation/policy, tests, docs and report. Attempt 2's implementation delta is commit 3085187; later merges preserve unrelated T-132/T-133 work. `git diff --check origin/dev..b49cb87` is clean.
- Merge: `git merge-tree --write-tree dev b49cb87` passes (dev 6d44d2a; tree 33926131360c6ffc6627fa551af6b3e03e9cd251). Also clean against origin/dev 0dec23e (tree df26dec95b446dda67908ea25f2104027c9f34ad).
- Review commits contain only review artifacts; candidate source is untouched. The existing attempt-1 review branch history is preserved.

## Spec compliance
- The captain-requested cheap F1 fix is implemented: `deliver_doorbell_now` checks coverage and records its Executing claim in the same write transaction before prompting. An existing Intended operation suppresses the sender's duplicate prompt. The deterministic regression passes with one prompt and one operation.
- F2 is resolved: Failed and Void doorbells cover nothing; Submitted and Unknown still cover. The fake-Herdr regression refuses the first engine prompt, then successfully rings on the next tick at injected time T0+21. CLI refusal fallback also passes.
- Existing v2 acceptance remains green: working pi gets captain instructions immediately; working codex gets urgent input immediately and normal input on idle; doorbells name IDs and request `gear`; successful synchronous sends suppress the next tick's doorbell.
- The unconditional one-doorbell promise still has the explicitly deferred overlap described in F1 below. This is logged under the packet's narrowed rework instruction and capped-rework bar.

## Findings
F1 [P1, logged] A stale engine decision can still duplicate a sender's doorbell — `crates/gear-engine/src/ops.rs:159-175` and `crates/gear-engine/src/lib.rs:335-339`. The engine reads coverage, computes an action, and subsequently inserts it without checking coverage again. If the sender claims and prompts between that read and insertion, the engine inserts and executes a second doorbell for the same message. Independently reproduced outside the worktree against candidate libraries with a working pi lane: prepare the engine action for M-1, call `deliver_doorbell_now`, insert the prepared action with `intend_doorbells`, then `execute_intended`; result: two prompts and two operations. Logged rather than blocking because the attempt-2 packet explicitly requests the cheap sender check and defers the full claim protocol, under the DELIVERABLE bar's capped rework/edge-case rule. A future small follow-up can recheck covered IDs in `intend_doorbells`' write transaction, omit already covered IDs, rebuild text for remaining IDs and skip empty actions, with a deterministic regression for this ordering. The documentation's unconditional wording should then match the guarantee.

## Set aside
- Earlier F1's engine-intended-before-sender ordering is fixed and tested; the residual reverse overlap is retained explicitly above rather than silently dropped. The new sender-first unit test calls a second sender and `execute_intended`; it does not test insertion of an engine action calculated before the claim. Existing CLI tests do cover an ordinary subsequent tick.
- Earlier F2 is fully resolved within the requested refusal path. Uncertain outcomes remain covered to avoid resending text that may already have arrived; process death after claiming retains the documented recovery-to-Unknown policy.
- Captain stale reminders, reviewer role policy, occupied-slot protection and identity-before-storage behavior were considered in attempt 1 and are unchanged by this rework.
- `git diff --check dev..b49cb87` reports a trailing blank line in `gear/voyages/M3.1.md:140`, inherited from integration ancestry and absent from the task-specific diff; no behavior or acceptance impact.
- Existing slow merge tests are unchanged; no new allowance or test above 10 s was introduced.

## Questions for the captain
- None; the packet already decides the deferred claim-protocol scope.

## Verdict
- pass — both requested fixes and all mechanical acceptance pass; F1 is logged, with no blocking findings.
