//! All integration tests of gear-forge in one binary: each test binary is
//! scanned by macOS on its first run, so one binary per crate keeps the gate
//! fast.
mod github_contract;
mod local_contract;
mod marker_contract;
mod sync_contract;
