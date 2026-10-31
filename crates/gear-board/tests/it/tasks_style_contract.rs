//! T-502: the Tasks tab's rows stand out by stage. A live row keeps a bold id
//! and title, its stage words carry the stage's own colour (bold while live);
//! a merged row's id, title and time are dim. The selection still lights the
//! whole row.

use gear_board::frame::{canvas, Tab};
use gear_board::graph::Stage;
use gear_board::style::{Color, Style};
use gear_board::{build, Options};
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;

/// A task of the fixture.
fn task(id: &str, rank: i64, state: TaskState) -> TaskRow {
    TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank,
        state,
        priority: "must".into(),
        lane: Some("data".into()),
        milestone: None,
        required: false,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(20),
        issue: None,
        updated: T0,
    }
}

/// The attempt `n` of a task on the lane.
fn attempt(id: &str, n: i64, state: AttemptState, started: i64) -> AttemptRow {
    AttemptRow {
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
        project: "p".into(),
        task: id.into(),
        n,
        seat: "p:data".into(),
        generation: 1,
        contract_version: 1,
        adopted_version: 1,
        pending_update: None,
        state,
        candidate: None,
        report: None,
        safe_point: None,
        started,
        updated: started,
    }
}

/// One project with a lane and four tasks: T-1 working, T-2 in review, T-3
/// reworking and T-4 merged.
fn world() -> Store {
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
            for (id, rank, state, goal) in [
                ("T-1", 1, TaskState::Running, "goal one"),
                ("T-2", 2, TaskState::Verifying, "goal two"),
                ("T-3", 3, TaskState::Running, "goal three"),
                ("T-4", 4, TaskState::Done, "goal four"),
            ] {
                tx.insert_task(&task(id, rank, state))?;
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: id.into(),
                    version: 1,
                    goal: goal.into(),
                    accept: String::new(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                    mechanical: None,
                })?;
            }
            tx.insert_attempt(&attempt("T-1", 1, AttemptState::Started, T0))?;
            tx.insert_attempt(&attempt("T-2", 1, AttemptState::Submitted, T0))?;
            tx.insert_attempt(&attempt("T-3", 1, AttemptState::Rejected, T0))?;
            tx.insert_attempt(&attempt("T-3", 2, AttemptState::Started, T0 + 60))?;
            tx.insert_attempt(&attempt("T-4", 1, AttemptState::Accepted, T0))?;
            tx.set_attempt_time("p", "T-4", 1, AttemptStep::Merged, T0 + 30 * 60)?;
            // The reviewer holds T-2: the stage is `in review`.
            testing::seed_temp_seat(tx, "p", "review-1", "T-2", "w2:p1", "pi", T0)?;
            Ok(())
        })
        .unwrap();
    store
}

/// The Tasks tab as the terminal draws it.
fn tasks(store: &mut Store, selected: usize) -> gear_board::frame::Canvas {
    let board = build(
        store,
        &Options {
            project: Some("p".into()),
            ..Options::default()
        },
        T0,
    )
    .unwrap();
    canvas(
        &board,
        &crate::screen(Tab::Tasks, selected, (160, 40)),
        None,
    )
}

/// The row of a task's own text, found by its title.
fn row_with(canvas: &gear_board::frame::Canvas, title: &str) -> (usize, String) {
    let text = canvas.text();
    for (row, line) in text.lines().enumerate() {
        if line.contains(title) {
            return (row, line.to_string());
        }
    }
    panic!("no {title:?} on the Tasks tab:\n{text}");
}

/// The character column of a text in a line.
fn column(line: &str, needle: &str) -> usize {
    let at = line
        .find(needle)
        .unwrap_or_else(|| panic!("no {needle:?} in {line:?}"));
    line[..at].chars().count()
}

#[test]
fn a_working_row_keeps_a_bold_id_and_title_and_a_yellow_bold_stage() {
    let mut store = world();
    let canvas = tasks(&mut store, 0);
    let (row, line) = row_with(&canvas, "goal one");
    let x = column(&line, "working");
    assert_eq!(
        canvas.style_at(x, row),
        Style::fg(Color::Warn).and_bold(),
        "the stage words: {line}"
    );
    // The id and the title are bold.
    assert_eq!(canvas.style_at(column(&line, "T-1"), row), Style::bold());
    assert_eq!(
        canvas.style_at(column(&line, "goal one"), row),
        Style::bold()
    );
}

#[test]
fn an_in_review_row_draws_its_stage_in_the_review_colour() {
    let mut store = world();
    let canvas = tasks(&mut store, 0);
    let (row, line) = row_with(&canvas, "goal two");
    let x = column(&line, "in review");
    assert_eq!(
        canvas.style_at(x, row),
        Style::fg(Color::Acc).and_bold(),
        "{line}"
    );
}

#[test]
fn a_reworking_row_draws_its_stage_in_the_rework_colour() {
    let mut store = world();
    let canvas = tasks(&mut store, 0);
    let (row, line) = row_with(&canvas, "goal three");
    let x = column(&line, "reworking");
    assert_eq!(
        canvas.style_at(x, row),
        Style::fg(Color::Warn).and_bold(),
        "{line}"
    );
    assert_eq!(
        canvas.style_at(x, row).fg,
        Some(Stage::Rework.color()),
        "the stage's own colour"
    );
}

#[test]
fn a_merged_row_is_dim_while_its_stage_is_green() {
    let mut store = world();
    // Select another row: the highlight would otherwise light this one.
    let canvas = tasks(&mut store, 3);
    let (row, line) = row_with(&canvas, "goal four");
    assert_eq!(
        canvas.style_at(column(&line, "merged"), row),
        Style::fg(Color::Ok),
        "the stage words are green: {line}"
    );
    assert_eq!(
        canvas.style_at(column(&line, "T-4"), row),
        Style::fg(Color::Dim),
        "{line}"
    );
    assert_eq!(
        canvas.style_at(column(&line, "goal four"), row),
        Style::fg(Color::Dim),
        "{line}"
    );
    assert_eq!(
        canvas.style_at(column(&line, "20m"), row),
        Style::fg(Color::Dim),
        "{line}"
    );
}

#[test]
fn the_selected_live_row_still_shows_its_highlight() {
    let mut store = world();
    let picks = {
        let board = build(
            &mut store,
            &Options {
                project: Some("p".into()),
                ..Options::default()
            },
            T0,
        )
        .unwrap();
        let plan = board.projects.into_iter().next().expect("the project");
        gear_board::frame::task_picks(&plan, T0, 160)
    };
    let selected = picks.iter().position(|task| task == "T-1").expect("T-1");
    let canvas = tasks(&mut store, selected);
    let (row, _) = row_with(&canvas, "goal one");
    for column in 1..160 - 1 {
        assert_eq!(
            canvas.style_at(column, row),
            Style {
                fg: None,
                bold: false,
                reverse: true,
            },
            "column {column} of the lit row: {}",
            canvas.text()
        );
    }
}
