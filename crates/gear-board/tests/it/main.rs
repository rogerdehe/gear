//! All integration tests of gear-board in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.
mod board_contract;
mod decisions_contract;
mod deliveries_contract;
mod deps_contract;
mod graph_contract;
mod idle_contract;
mod measures_contract;
mod overlaps_contract;
mod workload_contract;
mod colour_contract;
