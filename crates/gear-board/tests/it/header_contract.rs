//! The folded header under the goal box (T-354, README changes 3, 4, 6 and 14):
//! the goal check, the milestone the voyage is on, the time against the plan
//! and the tasks, in three rows with bold labels, and the `d` key that unfolds
//! them.

use std::time::Duration;

use gear_board::frame::{self, Canvas, Screen, Tab};
use gear_board::style::{Color, Style};
use gear_board::{build, run_with, Event, Keys, Options};
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{
    testing, AttemptRow, AttemptStep, NewDecision, Store, TaskRow, VoyageRow, VoyageStation,
};

const NOW: i64 = 1_800_000_000;

/// A project of one voyage that sails: a merged task, a running one, a queued
/// one and a cancelled one, two stations and the plan's goal check.
fn world() -> Store {
    let mut store = world_of(
        &[
            ("T-1", TaskState::Done),
            ("T-2", TaskState::Running),
            ("T-3", TaskState::Ready),
            ("T-4", TaskState::Cancelled),
        ],
        "2h",
        3_600,
    );
    store
        .write(|tx| {
            tx.insert_attempt(&attempt("T-2", AttemptState::Started))?;
            tx.set_attempt_time("p", "T-2", 1, AttemptStep::Started, NOW - 600)?;
            // S1 is reached by the merged task, S2 is the station the voyage is
            // on.
            tx.set_voyage_stations(
                "p",
                "M5",
                &[
                    VoyageStation { id: "S1".into(), needs: "the box".into() },
                    VoyageStation { id: "S2".into(), needs: "the views".into() },
                ],
            )?;
            tx.set_task_station("p", "T-1", Some("S1"))?;
            tx.set_task_station("p", "T-2", Some("S2"))?;
            tx.set_task_station("p", "T-3", Some("S2"))?;
            // The plan's own goal check: one clause met, one not.
            tx.set_meta(
                "goal-check:p:M5",
                r#"[{"met":true,"text":"the goal box holds"},{"met":false,"text":"the tabs open"}]"#,
            )?;
            Ok(())
        })
        .unwrap();
    store
}

/// A project of one voyage that left `departure_s` seconds ago with the plan's
/// `eta`, and the tasks named, each in the state given; the first one merged
/// 30 minutes into the voyage.
fn world_of(tasks: &[(&str, TaskState)], eta: &str, departure_s: i64) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p1", "pi", NOW)?;
            testing::seed_active_seat(tx, "p", "keel", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            tx.upsert_voyage(&VoyageRow {
                project: "p".into(),
                milestone: "M5".into(),
                title: "the board".into(),
                done: Some("the goal box holds; the tabs open".into()),
                bar: Some("DELIVERABLE".into()),
                eta: Some(eta.into()),
                departure: Some(NOW - departure_s),
                departure_tasks: Some(tasks.len() as i64),
                departure_estimate_min: Some(30 * tasks.len() as i64),
                updated: NOW,
            })?;
            for (rank, (id, state)) in tasks.iter().enumerate() {
                let lane = if rank < 2 { "hull" } else { "keel" };
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: (*id).into(),
                    rank: rank as i64 + 1,
                    state: *state,
                    priority: "must".into(),
                    lane: Some(lane.into()),
                    milestone: Some("M5".into()),
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
            }
            // The first task merged 30 minutes into the voyage.
            tx.insert_attempt(&attempt("T-1", AttemptState::Accepted))?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, NOW - 2_700)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Merged, NOW - 1_800)?;
            Ok(())
        })
        .unwrap();
    store
}

/// One attempt of a task, its first round on the hull lane.
fn attempt(task: &str, state: AttemptState) -> AttemptRow {
    AttemptRow {
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
        project: "p".into(),
        task: task.into(),
        n: 1,
        seat: "p:hull".into(),
        generation: 1,
        contract_version: 1,
        adopted_version: 1,
        pending_update: None,
        state,
        candidate: None,
        report: None,
        safe_point: None,
        started: NOW - 600,
        updated: NOW,
    }
}

/// The board drawn, the screen it was drawn for and the frame as rows.
fn drawn(store: &mut Store) -> (gear_board::Board, Screen, Canvas, Vec<String>) {
    let board = build(
        store,
        &Options { project: Some("p".into()), ..Options::default() },
        NOW,
    )
    .unwrap();
    let screen = Screen { tab: Tab::Now, selected: 0, detail: false, opened: Vec::new(), size: (160, 40) };
    let canvas = frame::canvas(&board, &screen, None);
    let rows: Vec<String> = canvas.text().lines().map(str::to_string).collect();
    (board, screen, canvas, rows)
}

/// The characters of a row from a column, at most `width` of them.
fn at(row: &str, column: usize, width: usize) -> String {
    row.chars().skip(column).take(width).collect()
}

#[test]
fn the_goal_check_is_the_plans_or_the_done_sentence_unmet() {
    // The plan's check: as the store wrote it.
    let mut store = world();
    let plan = build(
        &mut store,
        &Options { project: Some("p".into()), ..Options::default() },
        NOW,
    )
    .unwrap();
    assert_eq!(
        plan.projects[0].goal_check,
        vec![
            (true, "the goal box holds".to_string()),
            (false, "the tabs open".to_string())
        ]
    );

    // Without the meta, the clauses of the done sentence stand in, unmet.
    let mut store = world_of(&[], "2h", 3_600);
    let (clauses, screen, canvas, rows) = drawn(&mut store);
    assert_eq!(
        clauses.projects[0].goal_check,
        vec![
            (false, "the goal box holds".to_string()),
            (false, "the tabs open".to_string())
        ]
    );
    // The row says none is met and names the first clause to do.
    let goal = frame::layout(&clauses, &screen).tab_row - 4;
    assert_eq!(at(&rows[goal], 15, 10), "0 of 2 met");
    assert_eq!(at(&rows[goal], 30, 24), "next: the goal box holds");
    assert_eq!(canvas.style_at(15, goal), Style::fg(Color::Bad));
    assert_eq!(canvas.style_at(30, goal), Style::fg(Color::Bad));

    // A meta that is not a goal check is no check: the clauses stand in.
    store
        .write(|tx| tx.set_meta("goal-check:p:M5", "not a check"))
        .unwrap();
    let broken = build(
        &mut store,
        &Options { project: Some("p".into()), ..Options::default() },
        NOW,
    )
    .unwrap();
    assert_eq!(
        broken.projects[0].goal_check,
        vec![
            (false, "the goal box holds".to_string()),
            (false, "the tabs open".to_string())
        ]
    );
}

#[test]
fn the_folded_header_gives_the_check_the_milestone_the_time_and_the_tasks() {
    let mut store = world();
    let (board, screen, canvas, rows) = drawn(&mut store);
    let head = frame::layout(&board, &screen);
    let (goal, time, key) = (head.tab_row - 4, head.tab_row - 3, head.tab_row - 2);

    // The goal check: how many are met, then the first one that is not.
    let row = &rows[goal];
    assert_eq!(at(row, 1, 10), "Goal check");
    assert_eq!(at(row, 15, 10), "1 of 2 met");
    assert_eq!(at(row, 30, 19), "next: the tabs open");
    // The milestone: the station the voyage is on, and how many are reached.
    assert_eq!(at(row, 90, 9), "Milestone");
    assert_eq!(at(row, 101, 31), "▶ S2 the views (1 of 2 reached)");
    // The labels are titles, and the two states are bad and bold.
    assert_eq!(canvas.style_at(1, goal), Style::bold());
    assert_eq!(canvas.style_at(15, goal), Style::fg(Color::Bad));
    assert_eq!(canvas.style_at(30, goal), Style::fg(Color::Bad));
    assert_eq!(canvas.style_at(90, goal), Style::bold());
    assert_eq!(canvas.style_at(101, goal), Style::bold());

    // The time against the plan: 60 minutes used, 2h planned, the forecast's
    // slowest end expected; a bar of the prototype's width with the plan's end
    // and the expected finish on it.
    let used = 60;
    let end = board.projects[0].forecast.iter().map(|end| end.end_min).max().unwrap_or(0);
    let expected = used + end;
    let long = format!("{}h{:02}m", expected / 60, expected % 60);
    let row = &rows[time];
    assert_eq!(at(row, 1, 4), "Time");
    let bar: Vec<char> = at(row, 15, 40).chars().collect();
    assert_eq!(bar.len(), 40, "a 160 column screen's bar is 40 cells: {row:?}");
    assert!(bar.contains(&'│'), "the plan's end is on the bar: {row:?}");
    assert!(bar.contains(&'◆'), "a sailing voyage's expected finish: {row:?}");
    let short = at(row, 56, 34);
    assert!(
        short.starts_with(&format!("expected {long} of 2h00m, ")),
        "the time in words, against the forecast: {short:?}"
    );
    // The tasks, in three parts (merged, going, waiting; cancelled excluded).
    assert_eq!(at(row, 90, 5), "Tasks", "the tasks' label");
    assert_eq!(at(row, 101, 13), "█".repeat(13), "1 of 3 merged");
    assert_eq!(at(row, 114, 13), "▓".repeat(13), "1 of 3 going");
    assert_eq!(at(row, 127, 14), "░".repeat(14), "the rest is not started");
    assert!(
        at(row, 142, 17).starts_with("1/3 merged · 1 go"),
        "3 tasks, 1 merged, 1 going: {row:?}"
    );
    assert_eq!(canvas.style_at(101, time), Style::fg(Color::Ok));
    assert_eq!(canvas.style_at(114, time), Style::fg(Color::Warn));
    assert_eq!(canvas.style_at(127, time), Style::fg(Color::Dim));
    assert_eq!(canvas.style_at(142, time), Style::PLAIN);

    // The key that unfolds the header, and the blank row before the tabs.
    assert_eq!(
        at(&rows[key], 1, 52),
        "d: unfold the goal check, milestones, time and tasks"
    );
    assert_eq!(canvas.style_at(1, key), Style::fg(Color::Dim));
    assert!(
        rows[head.tab_row - 1].trim().is_empty(),
        "one blank row before the tab row: {:?}",
        rows[head.tab_row - 1]
    );
    assert!(rows[head.tab_row].contains("1 Now"), "the tab row follows the rows");
}

#[test]
fn the_time_of_a_landed_voyage_is_how_long_it_took() {
    // Both tasks merged: the voyage landed 75 minutes after departure, against
    // a plan of 1h.
    let mut store = world_of(&[("T-1", TaskState::Done), ("T-2", TaskState::Done)], "1h", 5_100);
    store
        .write(|tx| {
            tx.insert_attempt(&attempt("T-2", AttemptState::Accepted))?;
            tx.set_attempt_time("p", "T-2", 1, AttemptStep::Started, NOW - 3_000)?;
            // The latest merge of the voyage is 75 minutes after departure.
            tx.set_attempt_time("p", "T-2", 1, AttemptStep::Merged, NOW - 600)?;
            Ok(())
        })
        .unwrap();
    let (board, screen, canvas, rows) = drawn(&mut store);
    let head = frame::layout(&board, &screen);
    let time = head.tab_row - 3;
    let shown = board.projects[0].shown.as_ref().expect("the landed voyage");
    assert!(!shown.sailing, "the voyage landed");
    assert!(board.projects[0].forecast.is_empty(), "no open work, no forecast");

    assert_eq!(
        at(&rows[time], 15, 40),
        format!("{}│{}", "█".repeat(32), "█".repeat(7)),
        "75 minutes used of the 1h plan, the plan's end on the bar"
    );
    assert_eq!(at(&rows[time], 56, 31), "took 1h15m of 1h00m, 0h15m over");
    assert_eq!(canvas.style_at(15, time), Style::fg(Color::Bad), "over the plan");
    assert_eq!(canvas.style_at(56, time), Style::fg(Color::Bad));
}

#[test]
fn the_d_key_unfolds_the_header_and_keeps_the_selection() {
    // T-355: `d` unfolds the header, one row per clause and per station, the
    // time and the tasks with their legends and the key that folds them again;
    // the selection stays on the row the user picked.
    let mut store = world();
    let folded = run_events(&mut store, vec![Event::Key('2'), Event::Key('j')]);
    let unfolded = run_events(
        &mut store,
        vec![Event::Key('2'), Event::Key('j'), Event::Key('d')],
    );
    assert_ne!(
        last_frame(&unfolded),
        last_frame(&folded),
        "d unfolds the header under the box"
    );
    let frame_text = last_frame(&unfolded);
    assert!(
        frame_text.contains("d: fold"),
        "the unfolded header names the key that folds it: {frame_text}"
    );
    assert!(
        frame_text.contains("Milestones"),
        "the stations are drawn one row each: {frame_text}"
    );
    let rows: Vec<&str> = frame_text.lines().collect();
    // Row 0 is the shown voyage's label in reverse video and the tab row the
    // open tab's; the selection is the one lit row besides them.
    let lit: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(y, line)| {
            *y != 0 && !line.contains("1 Now") && line.contains("\u{1b}[7m")
        })
        .map(|(y, _)| y)
        .collect();
    assert_eq!(lit.len(), 1, "one lit row, the selection: {frame_text}");
}

#[test]
fn an_unfolded_tasks_row_names_the_tasks_that_moved_out() {
    // The voyage's log holds a `move` decision (T-351): T-1 left for M6, so the
    // unfolded header's Tasks row names it apart and counts it nowhere.
    let mut store = world();
    store
        .write(|tx| {
            tx.insert_decision_if_new(&NewDecision {
                project: "p".into(),
                key: "golden-move-T-1".into(),
                voyage: "M5".into(),
                at: "2026-10-28 12:00".into(),
                kind: "move".into(),
                what: "T-1 moved to M6".into(),
                why: None,
                alt: None,
                changes: None,
                star: false,
                source: "gear".into(),
                line: None,
                plan_version: None,
            })
        })
        .unwrap();
    let board = build(
        &mut store,
        &Options { project: Some("p".into()), ..Options::default() },
        NOW,
    )
    .unwrap();
    assert_eq!(
        board.projects[0].moved_out,
        vec![("T-1".to_string(), "M6".to_string())],
        "the board reads the voyage's move decision"
    );

    let screen =
        Screen { tab: Tab::Now, selected: 0, detail: true, opened: Vec::new(), size: (160, 40) };
    let canvas = frame::canvas(&board, &screen, None);
    let rows: Vec<String> = canvas.text().lines().map(str::to_string).collect();
    let tasks: Vec<&String> = rows
        .iter()
        .filter(|row| row.trim_start().starts_with("Tasks"))
        .collect();
    assert_eq!(tasks.len(), 1, "one Tasks row: {rows:?}");
    let row = tasks[0];
    assert!(
        row.trim_end().ends_with("· moved out: T-1"),
        "the Tasks row names the task that moved out: {row:?}"
    );
}

struct Events {
    events: Vec<Event>,
    size: (usize, usize),
}

impl Keys for Events {
    fn next_event(&mut self, _timeout: Duration) -> Option<Event> {
        if self.events.is_empty() {
            return Some(Event::Key('q'));
        }
        Some(self.events.remove(0))
    }

    fn size(&self) -> (usize, usize) {
        self.size
    }
}

/// The board's frames as the board wrote them, the last one last.
fn run_events(store: &mut Store, events: Vec<Event>) -> String {
    let opts = Options {
        project: Some("p".into()),
        color: false,
        refresh: Duration::from_millis(1),
        ..Options::default()
    };
    let mut out: Vec<u8> = Vec::new();
    run_with(store, &opts, &mut out, &mut Events { events, size: (160, 40) }, None).unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

/// The last frame the board drew: the frames are separated by the clear.
fn last_frame(out: &str) -> String {
    out.rsplit("\u{1b}[?2026h\u{1b}[H")
        .next()
        .unwrap_or_default()
        .to_string()
}
