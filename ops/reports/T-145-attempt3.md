# T-145 attempt 3 — session recovery rework

Scope: Fix both review findings: a completed session step no longer masks a required resume after an idle replacement, and resume fallback recovery has bounded exit/start readiness waits. Exhausted recovery sets NeedsCheck with a phase-specific reason and alerts the captain without typing into a working or unknown agent. A completed resume-fallback stays idempotent.

Files: `crates/gear-engine/src/session_runner.rs` — completed-step invalidation, bounded recovery and visible failure handling. `crates/gear-engine/src/session.rs` — injected-time regressions through round preparation and the runner, including temporary-reviewer state transitions.

Verification: `cargo test -p gear-engine session::tests --lib` → 27 passed. The changed-session and ineffective-exit regressions failed before their fixes. Tests cover a plain fallback after exit, a fallback agent that remains working, an unobservable fallback, a temp reviewer, and fallback idempotence. `scripts/test.sh` → 1130 passed, 0 skipped; suite wall time 98 s. The inherited multi-repo fixture took 14.9 s under contention (T-146 F5 remains logged); unchanged merge and fixture tests also exceeded 10 s.

Open questions: M-2774 asks about temp-seat transitions; this change uses the stated default of existing legal intermediate states. T-146's separate review-start rework and the captain's live end-to-end check remain separate work.

Out of scope: the T-153 dispatch integration.
