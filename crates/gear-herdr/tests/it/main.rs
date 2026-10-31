//! All integration tests of gear-herdr in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.
mod builtin_contract;
mod codex_dialogs_contract;
mod profile_contract;
mod session_fields_contract;
