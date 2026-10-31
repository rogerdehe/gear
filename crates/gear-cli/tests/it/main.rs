//! All integration tests of gear-cli in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.
mod admin_init_contract;
mod board_project_contract;
mod captain_handover_contract;
mod engine_hold_contract;
mod fixtures_contract;
mod home_multi_contract;
mod import_issues_contract;
mod install_skill_contract;
mod offline_contract;
mod plan_check_contract;
mod projects_contract;
mod readonly_no_engine;
mod scratch;
mod skill_contract;
mod version_contract;
mod whoami_contract;
