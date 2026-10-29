# Review of T-407

Status: running
Verdict: pass
Reviewed: review/t407 @ 7d9e6b322ddc602d89d695436af5d4d1122defd7   Reviewer: gear:review-9 (pi, deepseek-v4-pro-0813)   Round: 2
Contract: v5

## Mechanical acceptance

- build: `make check CRATE=gear-engine` → clean, zero compiler warnings (test-profile build). `cargo check -p gear-cli` also clean.
- tests: `make check CRATE=gear-engine` → 815 passed / 0 failed / 0 skipped. The rework's filter `TESTS='test(/probe|merge::tests::a_merge_that|ops::/)'` → 32 passed / 0 failed.
- allowed files: ok. Production edits are the four named engine modules plus the one permitted CLI drain; tests stay in the engine crate; the task report is the only other artifact.
- merges into origin/dev: clean (`git merge-tree --write-tree origin/dev 7d9e6b32`, exit 0). `git diff --check` passes.

## Spec compliance

F1 and F2 of the attempt-1 review are resolved.

- F1: `merge.rs Probe::begin` now keeps the unfinished-step early return and starts a fresh `now_ms()` boundary for a step whose earlier boundary already ended, so a retried merge step measures its own attempt. `a_retried_step_logs_its_own_time` covers the ended-boundary reopen and distinguishes it from the interrupted-boundary recovery test.
- F2: `execute_intended` extends the `Prepared::Run { probe: Some(..) }` handling with the `OpState::Failed` arm, writing `failed` and `done` from the durable intent for `Aimed::Gone`/`Aimed::Refused`; the agent-gone fixture now asserts both lines. No fabricated successful `typed`/`enter` lines.

The required happy-path probes are present and tested: session switch to done logs one line per step plus one total, a delivery confirmed after 40 s logs `confirmed 40000`, and a merge logs its five steps plus its `done` line. The probe buffer, `take_probe_log`, and the single CLI drain beside the release drain match T-398; nothing is written to stderr.

## Findings

- F1 [P2, logged] A session switch/resume whose recovery times out sets `step="failed"` but never writes a corresponding `failed` probe line — `crates/gear-engine/src/session_runner.rs:103` (the `step(seat, state, "failed", now)` call) and `:179` (`if phase == "failed" { return Ok(Prep::Wait) }`). `step` logs only the step that *leaves*, and `failed` never leaves, so the operation's last step is absent from the log. This is an error-recovery edge (two timeouts / missing agent), not the slow-step case the goal targets; under the DELIVERABLE bar it is logged, not blocking. Smallest fix: in `recovery_failed`, after `step(seat, state, "failed", now)`, emit `crate::ops::probe(str_field(state, "action"), &seat.id, "failed", elapsed(state, now), elapsed(state, now))`.

## Set aside

- Old persisted session state without the new `started` field falls back to the re-anchored `t0` in `elapsed`, and a restart Plan persisted before this change reads `at`/`started` as 0 (huge durations): both are upgrade-only edges for operations already in flight at deploy time, outside normal use.
- `resubmit_unsent` can emit a second `typed`/`enter` line when a lost text/Enter is actually re-sent: that is the step re-happening, so the line is true, not a duplicate artifact.
- The writer's report notes its final full suite run was backgrounded and polled; I ran both commands in the foreground myself and report my own numbers above.

## Questions for the captain

- none