# Review of T-145

Status: done
Verdict: fail
Reviewed: review/t145 @ 37ed006   Reviewer: gear:review-10 (Codex)   Round: 2

## Mechanical acceptance
- Build: `cargo build -p gear-engine -p gear-app --all-targets` exited 0, zero compiler warnings/errors (65 s). `scripts/test.sh` also compiled the entire workspace's tests without compiler warnings.
- Tests: `scripts/test.sh` exited 0: 1,124 passed, 0 failed, 0 skipped; 30 s test compilation, 53.954 s nextest / 54 s script suite time, below the 300 s red line. Fake-Herdr session tests passed, covering new id changes/timeouts, Codex's five-second delay, compact records/done/five-minute fallback, pi's small-session keep, resume startup refusal fallback, large contexts and working/unknown guards.
- Supplemental probes: two standalone Rust tests linked against the candidate's compiled libraries; both failed their recovery assertions (details below). Candidate source was not modified.
- Allowed files: contract v1 gives no explicit path allowlist. Session runner, session preparation, model-window config and constructor adaptations are in scope. The supplied comparison also contains the captain-authorized T-146 merge and reports; the rendered voyage snapshot reflects concurrent integration updates. No path-based mechanical failure.
- Merge: `git merge-tree --write-tree dev 37ed006` exited 0 initially (tree `7b3afbedae80c2566093268a5c3f0c619bd51be6`). After fetching, `git merge-tree --write-tree origin/dev 37ed006` exited 0 (tree `b65237745eebcf84f239d5cd3b51521e473a0947`; integration `55a6b84b79dbe541c3b98fd54f4cb86b3f5e2306`). `git diff --check dev..37ed006` passed. Review commits change only this artifact.

## Spec compliance
- The runner persists action, target, old session, timing and phase per seat and round; separates typing from enter; checks live Idle/Done before commands; and uses last_context with the selected model window and unknown-is-large semantics. The normal action paths satisfy the supplied tests.
- Design §12 explicitly amends §3.4 for Codex new-unconfirmed after at least five seconds and pi keep after fifteen idle seconds without work; these are intentional contract behavior.
- Recovery is incomplete: a completed persisted step can suppress a newly selected resume (F1), and resume's fallback exit wait is unbounded (F2).
- Attempt 1 was abandoned before implementation/review, as recorded in the voyage decisions; `gear --show T-145` contains no earlier findings to recheck. This review independently checks attempt 2 and its full requested comparison.

## Findings
F1 [P1, blocking] A completed step suppresses resume when the agent has restarted into another session — `crates/gear-engine/src/session_runner.rs:58`. The cache invalidation discards a done state for Action::Resume only when the occupant is absent. With a live idle replacement, prepare_lane correctly selects S1 Resume, but the cached done branch returns Ready without an exit or start. Reproduction: complete New for T-1; record round-session in its round; change the binding to restarted-session while its replacement agent is Idle; call prepare_lane for the same round. Actual: Ready, no terminal/start calls. The round's required context is never restored. Breaks the STABLE bar for a normal single restart and v0.4.0 done clause (1)'s automatic session continuity. Smallest fix: in run's existing-state selection, invalidate a completed state when the caller requests Resume and either there is no occupant or the current session differs from the requested session; preserve completed resume-fallback idempotence explicitly so fallback does not restart on every tick (about 10–20 lines). Add a regression through prepare_lane, checking exit/start with the requested session and Ready only after it is restored; keep the existing same-session/no-repeat behavior.
F2 [P1, blocking] Resume's fallback can wait forever after its exit deadline — `crates/gear-engine/src/session_runner.rs:159`. wait_exit has a 30-second limit, but the recovery types the same exit again and enters fallback_wait_exit, which has no deadline, retry or visible terminal failure. A failed/ineffective profile exit leaves an Idle agent occupying the pane indefinitely; the session remains pending and no plain fallback start occurs. The independent fake-Herdr probe advances through 0, 1, 31, 32 and 33 seconds, then 1,000 and 10,000 seconds: every call returns Wait and the persisted phase remains fallback_wait_exit. Related fallback_exit and wait_fallback paths also have no terminal deadline. Breaks §3.4's explicit bounded-waits requirement and the STABLE bar's requirement to handle lifecycle errors rather than silently strand unattended work, supporting v0.4.0 done clause (1). Smallest fix: bound the fallback exit/ready phases using their persisted step_at (30 seconds for exit, 60 for start readiness), retain start_fallback without resume args when no agent remains, and on exhausted recovery park the seat NeedsCheck with an explicit phase/error reason instead of leaving an invisible infinite Wait; never type or start over a working/unknown occupant (about 25–45 lines using existing seat-state helpers). Add injected-time tests for an ineffective exit and a fallback start that never becomes ready, asserting either the plain fallback reaches Ready or a visible bounded failure occurs; retain the existing refused-resume-start test.

## Set aside
- Lane dispatch does not yet call prepare_lane in this candidate. T-153 expressly owns that integration and S1 dispatch recovery; no duplicate finding for its absence. F1 concerns the runner rejecting the resume its caller already selected, so integration alone cannot fix it.
- T-146's review.rs changes are an explicitly authorized merge. Its separate F1–F4 rework remains outstanding per the hand-in report; this review does not duplicate those findings or certify that task's lifecycle independently.
- Safety with Working/Unknown: initial and enter phases withhold commands; waits can observe status without typing. A five-minute compaction deadline does not authorize typing a packet into an agent still Working. F2 requests a visible bounded error when safe restart cannot proceed, not unsafe forced typing.
- Slash commands use pane_type, the adapter's text/send_text operation, followed by send_keys enter. Their separate ticks follow the task's one-step requirement.
- Five unchanged tests exceeded 10 s under concurrent work: fixtures_contract::the_fixtures_build_replace_and_pass_their_gates (10.1 s), merge::tests::a_merge_waits_for_the_task_its_commits_carry (11.7 s), merge::tests::a_merge_waits_for_an_earlier_attempt_of_the_task_it_carries (13.2 s), merge::tests::waiting_merges_take_turns_and_a_chain_lands_in_order (10.8 s), and parallel_contract::periodic_scan_and_new_hand_in_trigger_at_their_required_cadence (10.1 s). Per ops/REVIEW.md these unchanged fixtures are recorded here rather than a failed mechanical fact. The full suite passed its mandatory limit.
- No live panes were driven. The live checks in design §12 inform the contract; this task's stated acceptance uses fake Herdr, and the captain owns the complete voyage's live proof.

## Questions for the captain
None. F1 and F2 are bounded runner fixes; the separate T-153 dispatch hook and T-146 rework remain with their assigned tasks.

## Reproduction details
The supplemental tests use the existing `world()` fixture from session.rs unchanged, with a cfg containing the built-in Codex profile. They compile outside the repository against the candidate's gear-engine, gear-herdr, gear-store, gear-app and gear-model libraries. All time is injected; no sleep or live Herdr interaction.

```rust
#[test]
fn completed_step_must_resume_after_restart() {
 let mut s=world(); let mut h=FakeHerdr::new(); h.add_agent("w1:p1","codex",AgentStatus::Idle);
 let seat=s.read(|tx|tx.get_seat("p:heat")).unwrap().unwrap(); let cfg=cfg();
 assert_eq!(prepare_lane(&mut s,&mut h,&cfg,&seat,"T-1",1,T0).unwrap(),Prep::Wait);
 assert_eq!(prepare_lane(&mut s,&mut h,&cfg,&seat,"T-1",1,T0+1).unwrap(),Prep::Wait);
 s.write(|tx| {tx.set_binding_session_ref(&seat.id,1,"round-session")?; tx.add_round_session(&gear_store::RoundKey{project:"p".into(),task:"T-1".into(),attempt:1,role:"dev".into()},"round-session")}).unwrap();
 assert_eq!(prepare_lane(&mut s,&mut h,&cfg,&seat,"T-1",1,T0+6).unwrap(),Prep::Ready);
 s.write(|tx|tx.set_binding_session_ref(&seat.id,1,"restarted-session")).unwrap();
 let before=h.calls.len();
 let result=prepare_lane(&mut s,&mut h,&cfg,&seat,"T-1",1,T0+10).unwrap();
 println!("result={result:?}, calls={:?}",&h.calls[before..]);
 assert_eq!(result,Prep::Wait,"must resume the round session before returning Ready");
}
#[test]
fn resume_exit_timeout_must_not_wait_forever() {
 let mut s=world(); let mut h=FakeHerdr::new(); h.add_agent("w1:p1","codex",AgentStatus::Idle);
 let seat=s.read(|tx|tx.get_seat("p:heat")).unwrap().unwrap(); let cfg=cfg();
 s.write(|tx| {tx.insert_round(&gear_store::NewRound{key:gear_store::RoundKey{project:"p".into(),task:"T-1".into(),attempt:1,role:"dev".into()},seat:seat.id.clone(),generation:1,cli:"codex".into(),model:None,start_at:T0-1})?;tx.add_round_session(&gear_store::RoundKey{project:"p".into(),task:"T-1".into(),attempt:1,role:"dev".into()},"wanted-session")}).unwrap();
 for n in [0,1,31,32,33,1000,10000] { let result=prepare_lane(&mut s,&mut h,&cfg,&seat,"T-1",1,T0+n).unwrap(); println!("at {n}: {result:?}"); }
 let state=s.read(|tx|tx.get_meta("engine.session_step.p:heat")).unwrap().unwrap();
 println!("state={state}");
 assert!(!state.contains("fallback_wait_exit"),"exit fallback is still unbounded after 10,000 seconds");
}
```

## Verdict
Fail. Mechanical acceptance passes; F1 and F2 block under the STABLE bar.
