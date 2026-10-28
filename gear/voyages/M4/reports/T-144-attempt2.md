# T-144 attempt 2

Scope: Fix the missing-session fallback for reworks and follow-ups. A live agent starts a new session when the earlier session is unknown; an absent agent remains fresh under L1.

Files: `crates/gear-engine/src/session.rs` — correct the fallback and extend L4/L7 regression coverage.

Verification: The L4 regression failed before the fix (`Fresh` versus `New`); `cargo test -p gear-engine session::tests --lib` passed all 13 tests after it. `scripts/test.sh` passed: 1117 tests, 0 skipped.

Open questions: none.

Out of scope: action execution and context-size detection belong to T-145.
