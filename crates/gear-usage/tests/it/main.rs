//! All integration tests of gear-usage in one binary: each test binary is
//! scanned by macOS on its first run, so one binary per crate keeps the gate
//! fast.
mod claude_jsonl_contract;
mod codex_rollout_contract;
mod opencode_db_contract;
mod pi_jsonl_contract;
mod price_contract;
mod quota_contract;
mod shared;
