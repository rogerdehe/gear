# Review of T-227

Status: done
Verdict: fail
Reviewed: keel/t227 @ abafba46   Reviewer: review-11 (Codex)   Round: 1
Contract: v3. Bar: FASTEST; block only a wrong restart, a missed or repeated alert, or a broken test.

## Mechanical acceptance
- Build: `cargo build -p gear-engine -p gear-store --all-targets` passed with zero warnings.
- Tests: `cargo test -p gear-engine` passed: 294 unit tests, 303 integration tests, zero failures; doc tests also passed.
- Tests: `scripts/test.sh` passed: 1,434 tests, zero failures, 67 seconds excluding build (83 seconds). All six new silent tests passed, each below 0.1 seconds.
- Allowed files: candidate patch (`origin/dev...abafba46`) changes only the new detector, its module declaration/call, the allowed store helper, and the task report. No unrelated implementation changes.
- Merge: `git merge-tree --write-tree origin/dev abafba46` passed cleanly after fetching origin; resulting tree b5c2ad0b001dad0959d830d0ea680d84303c6f4c. Local dev merge check also passed.
- Review commits contain only this review; candidate code untouched.

## Spec compliance
- The detector implements the requested age threshold, project and seat roles, usage records and message fallback, per-round marker, urgent System message, exact body, captain lookup, and returned seat IDs. No git calls were added. The six acceptance scenarios are covered.
- Extra guards skip terminal/missing attempts and require an idle observation. Terminal attempts no longer hold work, consistent with the done-sentence and the cancellation regression test.
- The required `report.flagged` integration exposes an existing downstream stall-alert path: one silent incident generates two captain alerts. The isolated detector tests do not exercise this seam.

## Findings
F1 [P1, blocking] One silent seat creates two urgent captain alerts in the same engine tick — `crates/gear-engine/src/lib.rs:333` (downstream consumer at line 603). This breaks done-sentence (2)'s one captain alert and the FASTEST bar's prohibition on repeated alerts. `silent::tick` inserts `silent:<seat>` itself, then its returned seat enters `report.flagged`; the final loop calls `stall::alert_fact`, inserting a second urgent System message on `stall:<seat>` even when there is no independent delivery failure. Smallest fix: keep the IDs returned by `silent::tick` in a per-tick collection, still extend `report.flagged` as required, and skip the generic stall insertion for those IDs in the final loop (roughly 6–10 lines in `lib.rs::tick`). Preserve generic alerts for other seats. Add a full-tick regression in `silent.rs` asserting one urgent captain alert on the first tick and no additional alert on the second. The captain must expand the lib.rs allowance beyond only the call to permit this deduplication.

Reproduction (verified against the candidate through an external harness, without editing candidate files): reuse `silent.rs`'s `the_world` and `insert_round` fixtures, then run the following instead of calling only `silent::tick`:

```rust
let mut w = the_world();
insert_round(&mut w, 1, NOW - 660, None);
let mut h = gear_herdr::fake::FakeHerdr::new();
h.add_agent_named("w1:p9", "pi", AgentStatus::Working, "p-captain");
h.add_agent_named("w1:p2", "pi", AgentStatus::Idle, "p-review-1");
let mut cfg = gear_engine::EngineCfg::default();
cfg.profiles = w.profiles.clone();
let report = gear_engine::tick(&mut w.store, &mut h, &cfg, NOW).unwrap();
for thread in ["silent:p:review-1", "stall:p:review-1"] {
    let messages = w.store.read(|tx| tx.thread_messages("p", thread)).unwrap();
    println!("{thread}: {messages:?}");
}
```

Observed: `report.flagged == ["p:review-1"]`; each thread contains one urgent System message addressed to `p:captain`. The first says the reviewer produced nothing for 11 minutes; the second says its delivery was not confirmed or continuing it made no progress. There was no failed delivery in this fixture.

## Set aside
- The full suite warned about unchanged merge tests `a_merge_waits_for_an_earlier_attempt_of_the_task_it_carries` (10.2 seconds) and `a_merge_waits_for_the_task_its_commits_carry` (10.0 seconds). Per reviewer rules these are not failed mechanical facts for this candidate; the suite remains below the 300-second red line.
- `dev..abafba46` shows two documentation differences in the voyage/backlog because dev contains newer documentation commits absent from the candidate. The merge-base candidate patch does not modify those files, and the merge is clean.
- Missing/changed agents and blocked dialogs already have their own alert paths; this review blocks on the proven duplicate for an ordinary idle reviewer, not on those alternate observations.
- Restart configuration behavior is T-226's scope; its regression tests pass here. Release and merge are captain follow-up work.

## Questions for the captain
- Permit the narrowly scoped lib.rs deduplication described in F1: v3 requires extending `report.flagged` but currently permits only that call, while its existing downstream consumer emits a second alert.

## Verdict
Fail: F1 is blocking. Required build and test commands pass.
