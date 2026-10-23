//! All integration tests of gear-cli in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.
mod install_skill_contract;
mod readonly_no_engine;
mod skill_contract;
