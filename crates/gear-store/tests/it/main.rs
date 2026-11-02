//! All integration tests of gear-store in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.
mod contract;
mod data_version_contract;
mod live_work_contract;
mod lock_contract;
mod migration_contract;
mod ops_by_kind_contract;
mod pause_contract;
mod read_indexes_contract;
mod slice2_contract;
mod time_contract;
mod upgrade_contract;
mod v6_contract;
mod v7_contract;
