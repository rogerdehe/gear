# T-146 attempt 2 — review startup rework

Scope: Address F1–F4: retry transient checkout failures with an attempt-scoped limit; release cancelled pending starts and clear their reservations; release a standby reviewer before changing repositories; and release/restart a reviewer on the selected model before delivery. Reopened reviewers bind the new pane and preserve prior Released facts and start intent before session preparation.

Files: `crates/gear-engine/src/review.rs` — retry budget, repository/model checks, selected-model recording, and resumed reviewer startup. `crates/gear-engine/src/release.rs` — pending-job cancellation and reservation cleanup. `crates/gear-engine/tests/it/dead_temp_slot_contract.rs` — cancelled startup and exhausted checkout integration regressions.

Verification: Focused review and release unit tests and the cancelled-start integration regression passed. `scripts/test.sh` — 1122 passed, 0 skipped; 81 s suite wall time. The existing multi-repo integration test took 10.2 s (F5 remains logged); other over-10-second warnings came from unchanged tests under contention. The four regressions were reproduced before their respective fixes. The checkout and stash paths also use the real Git fixtures from attempt 1.

Open questions: T-145's separate runner rework remains outstanding; the captain will rebuild and run the live loop after both tasks land. The session restart and selected-model setup are tested through preparation and fake Herdr, not a live agent.

Out of scope: T-145's runner findings F1–F2 and the logged T-146 timing finding F5.
