//! All integration tests of gear-board in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.
mod board_contract;
mod colour_contract;
mod decisions_contract;
mod deliveries_contract;
mod depart_contract;
mod deps_contract;
mod forecast_contract;
mod golden_contract;
mod golden_fixture;
mod graph_contract;
mod header_contract;
mod idle_contract;
mod measures_contract;
mod multi_project_contract;
mod overlaps_contract;
mod quiet_contract;
mod stations_contract;
mod tasks_deps_contract;
mod tasks_style_contract;
mod voyage_scope_contract;
mod voyage_select_contract;
mod workload_contract;

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
        opened: Vec::new(),
        size,
    }
}
