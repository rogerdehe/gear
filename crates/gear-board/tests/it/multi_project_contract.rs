//! T-489: a store with several projects gets a project bar at the top; the
//! selected project is bracketed, `]` (and `[`, `p`) switch, and every tab
//! below shows only the selected project. One project has no bar.

use std::time::Duration;

use gear_board::frame::{self, Tab};
use gear_board::{build, run_with, Event, Keys, Options};
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, Store, TaskRow, VoyageRow};

const NOW: i64 = 1_800_000_000;

/// One project with a sailing voyage, a lane and one task of it.
fn project(store: &mut Store, id: &str, lane: &str, pane: &str, milestone: &str, task: &str) {
    store
        .write(|tx| {
            testing::seed_project(tx, id, "sess", NOW)?;
            testing::seed_active_seat(tx, id, lane, SeatRole::Lane, pane, "pi", NOW)?;
            tx.upsert_voyage(&VoyageRow {
                project: id.into(),
                milestone: milestone.into(),
                title: format!("voyage {milestone}"),
                done: Some("it is delivered".into()),
                bar: Some("DELIVERABLE".into()),
                eta: Some("2h".into()),
                departure: Some(NOW - 3_600),
                departure_tasks: Some(1),
                departure_estimate_min: Some(30),
                updated: NOW,
            })?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: id.into(),
                id: task.into(),
                rank: 1,
                state: TaskState::Running,
                priority: "must".into(),
                lane: Some(lane.into()),
                milestone: Some(milestone.into()),
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(30),
                issue: None,
                updated: NOW,
            })?;
            tx.insert_attempt(&AttemptRow {
                project: id.into(),
                task: task.into(),
                n: 1,
                seat: format!("{id}:{lane}"),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Started,
                candidate: None,
                report: None,
                safe_point: None,
                started: NOW - 3_000,
                updated: NOW - 3_000,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })?;
            Ok(())
        })
        .unwrap();
}

/// Two projects, each with a lane and a sailing voyage.
fn two_projects() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    project(&mut store, "gear", "hull", "w1:p1", "M6", "T-1");
    project(&mut store, "prometheus", "keel", "w1:p2", "M7", "T-2");
    store
}

fn opts() -> Options {
    Options {
        project: None,
        color: false,
        refresh: Duration::from_millis(1),
        ..Options::default()
    }
}

/// The keys of a test, in order; `q` when they run out.
struct Events(Vec<Event>);

impl Keys for Events {
    fn next_event(&mut self, _timeout: Duration) -> Option<Event> {
        if self.0.is_empty() {
            return Some(Event::Key('q'));
        }
        Some(self.0.remove(0))
    }

    fn size(&self) -> (usize, usize) {
        (160, 40)
    }
}

fn last_frame(out: &str) -> String {
    out.rsplit("\u{1b}[?2026h\u{1b}[H")
        .next()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn two_projects_draw_a_bar_with_both_and_the_selected_one_bracketed() {
    let mut store = two_projects();
    let board = build(&mut store, &opts(), NOW).unwrap();
    let text = frame::frame(&board, &crate::screen(Tab::Now, 0, (160, 40)), None);
    let row = text.lines().next().unwrap_or_default();
    assert!(
        row.trim_start().starts_with("[gear · working"),
        "the first project is bracketed: {row:?}"
    );
    assert!(
        row.contains("prometheus · working"),
        "the second project is on the bar: {row:?}"
    );
    assert!(row.contains("M6"), "the voyage is on the bar: {row:?}");
    assert!(
        row.contains("M7"),
        "the other voyage is on the bar: {row:?}"
    );
    assert!(
        row.contains("S") || row.contains("✔") || row.contains('-'),
        "the station is on the bar: {row:?}"
    );
    assert!(row.contains('/'), "the time used of the eta: {row:?}");
    // The board's own title moved down one row.
    assert!(
        text.lines()
            .nth(1)
            .unwrap_or_default()
            .contains("GEAR · gear"),
        "the title follows the bar"
    );
}

#[test]
fn the_bracket_key_switches_project_and_the_now_tab_follows() {
    let mut store = two_projects();
    let mut out: Vec<u8> = Vec::new();
    run_with(
        &mut store,
        &opts(),
        &mut out,
        &mut Events(vec![Event::Key(']')]),
        None,
    )
    .unwrap();
    let text = last_frame(&String::from_utf8_lossy(&out));
    let row = text.lines().next().unwrap_or_default();
    assert!(
        row.contains("[prometheus · working"),
        "the second project is bracketed after `]`: {row:?}"
    );
    assert!(
        text.contains("keel"),
        "the Now tab shows the second project's lane: {text}"
    );
    assert!(
        !text.contains("hull"),
        "the first project's lane is gone: {text}"
    );
    assert!(
        text.lines()
            .nth(1)
            .unwrap_or_default()
            .contains("GEAR · prometheus"),
        "the title names the selected project"
    );
}

#[test]
fn one_project_draws_no_bar() {
    let mut store = Store::open_in_memory().unwrap();
    project(&mut store, "gear", "hull", "w1:p1", "M6", "T-1");
    let board = build(&mut store, &opts(), NOW).unwrap();
    let text = frame::frame(&board, &crate::screen(Tab::Now, 0, (160, 40)), None);
    let row = text.lines().next().unwrap_or_default();
    assert!(
        row.contains("GEAR · gear"),
        "the title is on the first row: {row:?}"
    );
    assert!(!row.contains('│'), "no bar: {row:?}");
    assert!(
        !row.contains("working"),
        "no state on the title row: {row:?}"
    );
}
