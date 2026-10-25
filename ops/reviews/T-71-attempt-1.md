# Review of T-71

Status: done
Verdict: fail
Reviewed: f1fedf1   Reviewer: Codex (GPT-6)   Round: 1

## Mechanical acceptance
- Tests: `env -u GEAR_DB cargo test --workspace` exited 0: 677 passed, 0 failed, 0 ignored across 19 test/doc-test suites; zero warnings. This ran against candidate code, with only the running review added on top.
- Build: `env -u GEAR_DB cargo build --workspace --all-targets` exited 0, zero warnings; repeated after removing the temporary probe target and passed again.
- Allowed files: ok. No narrower allowlist is in the packet. The six changed paths are the home page, seat helper, engine tick, focused engine tests and their registration, and the task report. All concern this contract.
- Merge: `git merge-tree --write-tree origin/dev f1fedf1` exited 0 after fetching; integration tip was `6c9fb79e2f304549e1bfd105f920f6628b80c850`. Requested `dev..f1fedf1` diff inspected; `git diff --check dev..f1fedf1` passed.
- Independent negative probes: a temporary engine integration target reused the candidate's real-store/FakeHerdr fixture and added three acceptance assertions. `env -u GEAR_DB cargo test -p gear-engine --test t71_review_probe review_probe -- --nocapture` exited 101: all three assertions failed as described below. The temporary target was removed; no candidate source was changed. Reproduction recipes are included in each finding, so the rework does not depend on this worktree or scratch files.

## Spec compliance
- Generation timestamps exclude older messages in the ordinary existing tests. Moving doorbell planning after delivery intentions correctly prevents the tested simultaneous delivery/doorbell case. The seven added tests pass.
- Missing: a temp's messages must also concern its current task, even when an earlier task's reply is created after the generation starts.
- Missing: an explicit first-delivery-confirmed gate. Occupied-slot checking only protects a delivery that already exists and still holds its slot.
- Regression: the captain's outstanding open items from before a generation change no longer ring, although its home inbox still displays them.

## Findings
F1 [P1, blocking] Late replies about an earlier task leak into the new reviewer's home page and doorbell inputs — `crates/gear-app/src/home.rs:127` and `crates/gear-engine/src/lib.rs:351`. Reproduce using `warm_reviewer`: its current task is T-7, generation starts at JOB_STARTED; insert a Reply addressed to that seat with task T-5 at JOB_STARTED+5, then build its home at JOB_STARTED+20. The home prints `UNSHOWN (1)` and `M-1 T-5:1 late judge v2 for previous task`. The engine uses the same timestamp-only inclusion, so that reply also qualifies to ring once its slot is free. A captain replying after the seat has been reused is a single normal operation, and the contract explicitly requires temp messages to be about the current task. Breaks the DELIVERABLE bar's requirement to fix the normal-use error, and done (1)'s unattended voyage: the new agent can act on the previous task's instructions. Smallest fix: in home::build_with_agents and doorbell_inputs, additionally require messages for a Temp seat to match `seat.task_id`, applying the same predicate to actionable and unshown lists (roughly 10–20 lines, optionally one shared predicate). Add a late-old-task regression alongside the existing current-task test.
F2 [P1, blocking] A captain generation change silences unresolved older open items — `crates/gear-engine/src/lib.rs:354`. Reproduce in the existing fixture: insert an open question for p:captain at T0+10, advance that captain to generation 2 with binding created JOB_STARTED, then tick at JOB_STARTED+20. No GEAR prompt is sent, despite the question remaining open. The unconditional timestamp filter runs before the captain-specific attention logic. A captain re-registration/restart therefore loses notifications for outstanding decisions; the home inbox still includes them. Breaks the explicit acceptance that the captain's open items still ring, the DELIVERABLE normal-use bar, and done (1)'s unattended voyage. Smallest fix: retain all captain actionable open items regardless of generation timestamp, letting attention::should_show govern them; keep generation isolation for worker seats (approximately 2–4 changed lines in doorbell_inputs). Add the old-open-item/new-captain-generation test, since the current captain test creates its item after that captain's binding start.
F3 [P1, blocking] A new lane can receive a doorbell before any first task delivery is confirmed — `crates/gear-engine/src/lib.rs:378`. Reproduce using `reregistered_lane`: leave it active/idle with no ready task and no delivery op, insert a current-generation reply at JOB_STARTED+5, then tick at JOB_STARTED+20. FakeHerdr receives `GEAR: 1 item(s) waiting for you. Run gear to see them.` An empty slot is treated as permission to ring, although this generation has never received its task. This is normal when a lane is onboarded before a task becomes ready. Breaks the explicit first-delivery-confirmed requirement and DELIVERABLE normal-use bar: the fresh agent is still first woken by a doorbell. Smallest fix: in doorbell_inputs, for non-captain seats require a Confirmed Deliver op for this seat's current generation before permitting a doorbell, in addition to the existing slot guard. `ops_of_seat_kind` can supply the rows (approximately 10–15 lines). Preserve the captain exception and add a no-delivery-yet lane regression plus the existing confirmed-delivery/current-reply case.

## Set aside
- The existing reviewer delivery-confirmation rule uses a verdict as evidence. This task does not change that rule; the current-reply acceptance test explicitly supplies a Confirmed delivery. Changing reviewer confirmation semantics is separate work.
- Timestamp inclusion at the binding's exact second follows the store's second-resolution clock. Same-second ordering ambiguity is an edge case; the blocking temp leak above occurs five seconds later and needs a task predicate regardless of clock precision.
- Retaining old messages in the store is appropriate; the task requires isolation, not deletion or automatic resolution.
- Other milestone capabilities (push destination enforcement, startup dialogs, GitHub claims, adopt skeleton) are unchanged by this candidate and are outside this review's requested fix.

## Questions for the captain
None.

## Verdict
Fail: F1, F2 and F3 block. The standard workspace tests and build pass, but independent acceptance probes reproduce normal-use contract failures. No code fixes are included in this review branch.
