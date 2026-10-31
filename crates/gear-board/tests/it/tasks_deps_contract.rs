//! T-494: the Tasks tab draws the dependencies.
//!
//! The deps column stands right of the lanes: a line from every task not yet
//! merged to each direct dependency it waits for that is shown and not merged.
//! A dependency on a merged task draws nothing; the selected row's lines are
//! lit; a frame too narrow for the lines caps them and shows the rest as `+N`.

use gear_board::frame::{canvas, Tab};
use gear_board::style::Color;
use gear_board::{build, Options};
use gear_model::{SeatRole, TaskState};
use gear_store::{testing, Store, TaskRow};

const T0: i64 = 1_800_000_000;

/// The deps column of the one-lane fixture: `9 + 6 * 0 + 4`.
const DEPS_X: usize = 13;

/// A project with one lane and the tasks `(id, rank, state, deps)`.
fn world(tasks: &[(&str, i64, TaskState, &[&str])]) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                "w1:p1",
                "claude",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            for (id, rank, state, deps) in tasks {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: (*id).into(),
                    rank: *rank,
                    state: *state,
                    priority: "must".into(),
                    lane: Some("data".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: deps.iter().map(|dep| (*dep).to_string()).collect(),
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                    issue: None,
                    updated: T0,
                })?;
            }
            Ok(())
        })
        .unwrap();
    store
}

/// The Tasks tab as the terminal shows it.
fn tasks_frame(store: &mut Store, selected: usize, size: (usize, usize)) -> Vec<String> {
    let board = build(
        store,
        &Options {
            project: Some("p".into()),
            ..Options::default()
        },
        T0,
    )
    .unwrap();
    let text = canvas(&board, &crate::screen(Tab::Tasks, selected, size), None).text();
    text.lines().map(str::to_string).collect()
}

/// The row of a task's own text.
fn row_of<'a>(lines: &'a [String], task: &str) -> &'a str {
    lines
        .iter()
        .find(|line| line.contains(task) && !line.contains("time"))
        .unwrap_or_else(|| panic!("no row of {task}: {lines:?}"))
}

/// The dependency glyphs of the deps column in one row.
fn glyphs(row: &str, deps_x: usize) -> Vec<char> {
    row.chars()
        .skip(deps_x)
        .filter(|character| matches!(character, '┌' | '└' | '│'))
        .collect()
}

/// The dependency glyphs in the deps column of every task row.
fn task_glyphs(lines: &[String], deps_x: usize) -> Vec<char> {
    lines
        .iter()
        .filter(|line| line.contains("T-"))
        .flat_map(|line| glyphs(line, deps_x))
        .collect()
}

#[test]
fn a_task_waiting_for_a_task_waiting_for_another_draws_two_lines_and_no_transitive_one() {
    let mut store = world(&[
        ("T-1", 1, TaskState::Ready, &[]),
        ("T-2", 2, TaskState::Ready, &["T-1"]),
        ("T-3", 3, TaskState::Ready, &["T-2"]),
    ]);
    let lines = tasks_frame(&mut store, 0, (160, 40));

    // T-3 waits for T-2 and T-2 waits for T-1: one line each.
    let t3 = row_of(&lines, "T-3");
    let t2 = row_of(&lines, "T-2");
    let t1 = row_of(&lines, "T-1");
    assert_eq!(glyphs(t3, DEPS_X), vec!['┌'], "T-3's line: {t3:?}");
    assert_eq!(
        glyphs(t2, DEPS_X),
        vec!['└', '┌'],
        "T-2's line ends and the next starts: {t2:?}"
    );
    assert_eq!(glyphs(t1, DEPS_X), vec!['└'], "T-1's line: {t1:?}");
    // No T-3-T-1 line: two starts in all, not three.
    let starts = task_glyphs(&lines, DEPS_X)
        .iter()
        .filter(|glyph| **glyph == '┌')
        .count();
    assert_eq!(starts, 2, "two lines: {lines:?}");
}

#[test]
fn a_task_waiting_for_four_tasks_draws_one_line_to_each() {
    let mut store = world(&[
        ("T-1", 1, TaskState::Ready, &[]),
        ("T-2", 2, TaskState::Ready, &[]),
        ("T-3", 3, TaskState::Ready, &[]),
        ("T-4", 4, TaskState::Ready, &[]),
        ("T-9", 9, TaskState::Ready, &["T-1", "T-2", "T-3", "T-4"]),
    ]);
    let lines = tasks_frame(&mut store, 0, (160, 40));

    let release = row_of(&lines, "T-9");
    assert_eq!(
        glyphs(release, DEPS_X),
        vec!['┌', '┌', '┌', '┌'],
        "one line to each dependency: {release:?}"
    );
    let ends = task_glyphs(&lines, DEPS_X)
        .iter()
        .filter(|glyph| **glyph == '└')
        .count();
    assert_eq!(ends, 4, "four lines end: {lines:?}");
}

#[test]
fn a_dependency_on_a_merged_task_draws_nothing() {
    let mut store = world(&[
        ("T-1", 1, TaskState::Done, &[]),
        ("T-2", 2, TaskState::Ready, &["T-1"]),
    ]);
    let lines = tasks_frame(&mut store, 0, (160, 40));

    let row = row_of(&lines, "T-2");
    assert_eq!(glyphs(row, DEPS_X), Vec::<char>::new(), "{row:?}");
    assert!(
        task_glyphs(&lines, DEPS_X).is_empty(),
        "no line to the merged T-1: {lines:?}"
    );
    assert!(
        !row.contains("waits for"),
        "a merged dependency is no wait: {row:?}"
    );
}

#[test]
fn the_selected_rows_lines_are_lit_and_the_others_are_not() {
    let mut store = world(&[
        ("T-1", 1, TaskState::Ready, &[]),
        ("T-2", 2, TaskState::Ready, &["T-1"]),
        ("T-3", 3, TaskState::Ready, &["T-2"]),
    ]);
    let board = build(
        &mut store,
        &Options {
            project: Some("p".into()),
            ..Options::default()
        },
        T0,
    )
    .unwrap();
    let canvas = canvas(&board, &crate::screen(Tab::Tasks, 0, (160, 40)), None);
    let lines: Vec<String> = canvas.text().lines().map(str::to_string).collect();
    let t3_row = lines
        .iter()
        .position(|line| line.contains("T-3") && !line.contains("time"))
        .expect("T-3's row");
    // T-3 is the first pick: the line from its row to T-2 is lit along the
    // rows it crosses (the selected row itself is the board's reverse bar).
    let lit = canvas.style_at(DEPS_X, t3_row + 1);
    assert_eq!(lit.fg, Some(Color::Warn), "{lit:?}");
    assert!(lit.bold, "the selected row's line is bold: {lit:?}");
    // The line of T-2 to T-1 is not the selected task's: it stays dim.
    let t2_row = lines
        .iter()
        .position(|line| line.contains("T-2") && !line.contains("time"))
        .expect("T-2's row");
    let dim = canvas.style_at(DEPS_X + 1, t2_row + 1);
    assert_eq!(dim.fg, Some(Color::Dim), "{dim:?}");
    assert!(!dim.bold, "an unselected line is not bold: {dim:?}");
}

#[test]
fn a_frame_narrower_than_the_lines_caps_them_and_shows_the_rest_as_a_count() {
    let mut store = world(&[
        ("T-1", 1, TaskState::Ready, &[]),
        ("T-2", 2, TaskState::Ready, &[]),
        ("T-3", 3, TaskState::Ready, &[]),
        ("T-4", 4, TaskState::Ready, &[]),
        ("T-9", 9, TaskState::Ready, &["T-1", "T-2", "T-3", "T-4"]),
    ]);
    // 55 columns: the deps column keeps 40 for the task text, so two columns
    // fit; one is kept for the count, and the release's four lines become one
    // line and `+3`.
    let lines = tasks_frame(&mut store, 0, (55, 40));

    let release = row_of(&lines, "T-9");
    assert_eq!(
        glyphs(release, DEPS_X),
        vec!['┌'],
        "the line that fits: {release:?}"
    );
    assert!(release.contains("+3"), "the rest as a count: {release:?}");
}
