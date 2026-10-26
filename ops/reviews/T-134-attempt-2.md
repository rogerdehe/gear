# Review of T-134

Status: done
Verdict: pass
Reviewed: review/t134 @ d6caf0a   Reviewer: gear:review-10 (Codex)   Round: 2

## Mechanical acceptance
- Build: `cargo build --workspace --all-targets` → exit 0, zero compiler warnings (0.19 s warm).
- Tests: `scripts/test.sh` run once → exit 0; 934 passed, 0 skipped across 15 binaries. Nextest 48.675 s; script reports build 18 s, suite 49 s, below the 60 s target and 300 s red line. Four existing slow-test warnings concern unchanged tests; no new naming test exceeds 0.1 s.
- All six `agent_names_contract` tests pass: the four original acceptance cases plus correctly named reviewer untouched and cleared reviewer renamed before packet delivery. All nine captain-take tests pass, including naming and collision refusal.
- Allowed files: contract v2 specifies no path allowlist. Inspected requested `dev..d6caf0a`, the rework implementation `63ac039`, and its merges/report. Fetched origin/dev is 232f59d (T-130 attempt 2 included); the remaining 24 paths are naming implementation/callers, fake/test support, docs/report, one cosmetic identity-condition change and an example's required trait implementation. No unapproved functional scope in the rework. Review branch differs from candidate only in review artifacts.
- Merge: `git merge-tree --write-tree dev d6caf0a` → exit 0, tree d81427809e877d66e8a60eb7f4ce7790d289e6b6. Fetched `origin/dev` check → exit 0, tree acb0a535c679f7872c86b0be2b3797682f132543. No conflicts. `git diff --check origin/dev..d6caf0a` is clean. Local-dev range retains the inherited trailing blank line at `gear/voyages/M3.1.md:140`.
- Independent probe: compiled `/tmp/t134-review-attempt2-probe.rs` with rustc against this candidate's built libraries, using only FakeHerdr. A correctly named Temp assigned T-134 remains `p-review-t134` with no rename. Clearing it then calling `deliver_doorbell_now` restores `p-review-t134`, records rename before prompt, and returns the delivered pane. Repeated captain collision restoration still creates zero items (F2). All assertions pass; no candidate source changed.

## Spec compliance
- Earlier blocking F1 is resolved. `names::desired` derives an assigned Temp's name from its task, and captain/lane names from their seats. Reviewer startup delegates to the same task-name function. Tick restoration, both ops delivery paths, restart, release and switch all use the helper; none infer the desired name from Herdr's cleared or incorrect value.
- Original lane/captain restoration, correct-name no-op, collision refusal/deduplication, same-tick delivery and captain-take naming remain passing. The requested origin/dev merge is present.
- Earlier F2 and F3 remain unchanged and logged as explicitly directed by the attempt-2 packet. No new blocking findings.

## Findings
F2 [P2, logged] Captain name collisions still create no captain item — `crates/gear-engine/src/names.rs:137-140`. `report_taken` returns for Captain, so another pane holding `p-captain` can leave the captain unnamed without a persisted collision notice; the independent probe confirms zero items over two restoration calls. Retained as an edge case under the DELIVERABLE bar and the explicit rework scope. Follow-up: let the existing insertion/deduplication address the captain itself, even if its doorbell cannot yet arrive, and test one persisted item across repeated ticks; the captain can read it with `gear`.
F3 [P2, logged] Onboarding teardown still sends to an unnamed leftover occupant without ensuring its name — `crates/gear-engine/src/onboard.rs:440-443`. This precedes tick restoration and activation of the new binding. An unnamed command-exit occupant refuses the exit prompts, so onboarding can exhaust its attempts and park instead of replacing it. Retained as the leftover-occupant recovery edge under DELIVERABLE and the explicit rework scope. Follow-up: ensure a suitable seat name before teardown sends, respect collision refusal, and add a fake regression for an unnamed command-exit occupant.

## Set aside
- Earlier F1 is fixed rather than waived: both new regressions pass, and the independent probe additionally checks the synchronous doorbell path. Restart/release/switch callers were inspected for the shared helper; no redesign is needed.
- The cosmetic `identity.rs` condition collapse preserves short-circuit behavior; reviewer name normalization remains the previous lowercase/dash-removal rule. Unassigned Temp fallback does not affect an assigned reviewer's name.
- Failed reads/renames and a clear between final ensure and prompt remain observation/recovery edges already considered in round 1; ordinary cleared-name delivery succeeds.
- T-130's separately reviewed claim/coverage changes are merged from dev. Its deferred overlap remains that task's logged finding, not additional T-134 rework.
- Existing slow-test warnings and local-dev-only whitespace are inherited. Probe source/executable/logs remain outside the repository. Prior review history is preserved; candidate code is untouched.

## Questions for the captain
None.

## Verdict
Pass — required F1 rework and all mechanical acceptance pass. F2 and F3 remain logged edge cases under the DELIVERABLE bar; no blocking findings.
