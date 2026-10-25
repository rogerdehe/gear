//! All integration tests of gear-cli in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.
mod admin_init_contract;
mod fixtures_contract;
mod install_skill_contract;
mod push_check_contract;
mod readonly_no_engine;
mod scratch;
mod skill_contract;
mod version_contract;
