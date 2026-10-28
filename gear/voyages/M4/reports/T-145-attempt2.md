# T-145 attempt 2 — session actions

Scope: Run new, compact and resume as persisted steps, with bounded waits and the specified fallbacks. Use model context windows and last session usage for follow-up compaction; do not type commands into working or unknown agents. The captain instructed this branch to merge the T-146 candidate; its separate F1–F4 rework remains outstanding.

Files: `crates/gear-engine/src/session_runner.rs` — persisted per-seat action runner, typed commands, waits and fallback handling. `crates/gear-engine/src/session.rs` — connect round preparation, session metadata and context-size selection. `crates/gear-app/src/config.rs` and `crates/gear-app/tests/it/config_contract.rs` — parse and test optional model context windows. `crates/gear-engine/src/model.rs` and `crates/gear-engine/src/quota.rs` — preserve the optional window when constructing model entries. `crates/gear-engine/src/lib.rs` — register the runner. The merge also includes T-146's review files and report.

Verification: `cargo test -p gear-engine session::tests --lib` → 21 passed, including fake-Herdr timeouts, fallback, command readiness, JSONL fixture files for context and compaction records. `cargo test -p gear-app --test it a_model_entry_names_its_cli_and_its_arguments` → passed. `cargo test -p gear-engine --lib` → 197 passed before the final added compaction test. `scripts/test.sh` → 1124 passed, 0 skipped; suite 42 s.

Real environment: The captain's live checks in `ops/plan/m4-design.md` §12 cover pi/Codex new, compact and resume behavior. This task's runner was verified with fake Herdr and on-disk JSONL fixture files; the complete live loop remains the captain's check after rebuilding.

Open questions: T-146's released-reviewer start intent and F1–F4 are addressed in its separate rework; the engine is not rebuilt until both tasks land (M-2726).

Out of scope: T-146 rework and live end-to-end review delivery.
