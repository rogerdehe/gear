//! All integration tests of gear-board in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.
mod board_contract;
mod decisions_contract;
mod deliveries_contract;
mod deps_contract;
mod golden_contract;
mod golden_fixture;
mod graph_contract;
mod idle_contract;
mod measures_contract;
mod overlaps_contract;
mod quiet_contract;
mod stations_contract;
mod workload_contract;
mod colour_contract;
mod voyage_scope_contract;

/// The screen state a test draws: a tab, a selected row and a size.
pub fn screen(
    tab: gear_board::frame::Tab,
    selected: usize,
    size: (usize, usize),
) -> gear_board::frame::Screen {
    gear_board::frame::Screen {
        tab,
        selected,
        detail: false,
        size,
    }
}
