//! The colours of the board (the prototype of `gear/voyages/M4/board`), the
//! NO_COLOR switch, and the lane-name column that grows with the longest
//! lane name.

use std::time::Duration;

use gear_board::frame::{canvas, frame, Tab};
use gear_board::graph::styled_lines;
use gear_board::style::{colour_allowed, lane_color, Color, Style};
use gear_board::{run_with, to_json, Event, Keys, Options};
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, ContractRow, NewFriction, Store, TaskRow};

const NOW: i64 = 1_800_000_000;

/// Now, in epoch seconds.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// A task of the fixture.
fn task(id: &str, lane: &str, state: TaskState, estimate: i64) -> TaskRow {
    TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank: 1,
        state,
        priority: "must".into(),
        lane: Some(lane.into()),
        milestone: Some("M4".into()),
        required: false,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(estimate),
        issue: None,
        updated: NOW,
    }
}

/// The attempt of a task, started at `started`.
fn attempt(id: &str, lane: &str, state: AttemptState, started: i64) -> AttemptRow {
    AttemptRow {
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
        project: "p".into(),
        task: id.into(),
        n: 1,
        seat: format!("p:{lane}"),
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

/// A voyage an hour old with two lanes: `hull` merged T-1 (44 of 40
/// minutes), `rewrite` (a seven-letter name) works on T-2 for an hour on an
/// estimate of 30 minutes, and each lane left a friction report.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            testing::seed_active_seat(tx, "p", "rewrite", SeatRole::Lane, "w1:p3", "pi", NOW)?;
            tx.upsert_voyage(&gear_store::VoyageRow {
                project: "p".into(),
                milestone: "M4".into(),
                title: "one seat model".into(),
                done: Some("a rework resumes its session".into()),
                bar: None,
                eta: Some("12h".into()),
                departure: Some(departure),
                departure_tasks: Some(2),
                departure_estimate_min: Some(70),
                updated: NOW,
            })?;
            tx.insert_task(&task("T-1", "hull", TaskState::Done, 40))?;
            tx.insert_task(&task("T-2", "rewrite", TaskState::Running, 30))?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-2".into(),
                version: 1,
                goal: "Give internal/dbtest a fixture".into(),
                accept: String::new(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: NOW,
                mechanical: None,
            })?;
            tx.insert_attempt(&attempt("T-1", "hull", AttemptState::Accepted, departure))?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, departure)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::HandedIn, departure + 44 * 60)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Merged, departure + 44 * 60)?;
            tx.insert_attempt(&attempt("T-2", "rewrite", AttemptState::Started, departure))?;
            tx.set_attempt_time("p", "T-2", 1, AttemptStep::Started, departure)?;
            for (id, lane) in [("T-1", "hull"), ("T-2", "rewrite")] {
                tx.insert_friction(&NewFriction {
                    project: "p".into(),
                    task: id.into(),
                    attempt: 1,
                    role: "dev".into(),
                    seat: format!("p:{lane}"),
                    body: format!("friction of {lane}"),
                    required: false,
                })?;
            }
            Ok(())
        })
        .unwrap();
    store
}

/// The board of the fixture.
fn board() -> gear_board::Board {
    let mut store = world();
    gear_board::build(
        &mut store,
        &Options { project: Some("p".into()), ..Default::default() },
        now_seconds(),
    )
    .unwrap()
}

/// The row (counted from zero) and the column of a text on a plain screen.
fn find(screen: &str, needle: &str) -> (usize, usize) {
    for (row, line) in screen.lines().enumerate() {
        if let Some(at) = line.find(needle) {
            return (row, line[..at].chars().count());
        }
    }
    panic!("no {needle:?} on the screen:\n{screen}");
}

/// The line of a plain screen that holds a text.
fn line_with<'a>(screen: &'a str, needle: &str) -> &'a str {
    screen.lines().find(|line| line.contains(needle)).unwrap_or_else(|| panic!("no {needle:?}:\n{screen}"))
}

/// The column of a text in a line, in characters.
fn column(line: &str, needle: &str) -> usize {
    let at = line.find(needle).unwrap_or_else(|| panic!("no {needle:?} in {line:?}"));
    line[..at].chars().count()
}

#[test]
fn a_task_over_its_estimate_has_a_yellow_bar() {
    let board = board();
    let screen = canvas(&board, &crate::screen(Tab::Now, 0, (160, 40)), None);
    let text = screen.text();
    // T-2 has run 60 of its 30 minutes: the bar is full and yellow.
    let (row, x) = find(&text, "██████████ 60/30m");
    assert_eq!(screen.style_at(x, row).fg, Some(Color::Warn), "{text}");
    let ansi = screen.ansi(true);
    let end = ansi.find("██████████ 60/30m\u{1b}[0m").expect("the bar ends its yellow run");
    assert!(ansi[..end].rfind("\u{1b}[") == ansi[..end].rfind("\u{1b}[33m"), "{ansi:?}");
    // The heading of what needs a look is yellow and bold.
    let (row, x) = find(&text, "NEEDS A LOOK");
    assert_eq!(screen.style_at(x, row), Style::fg(Color::Warn).and_bold());
    // A lane name is bold, in its lane colour (the first lane is cyan).
    let (row, x) = find(&text, "› hull");
    assert_eq!(screen.style_at(x, row).fg, Some(Color::Acc));
    // The selected row is lit across the width (README change 13): the lane
    // name's own colour is not drawn on it.
    assert_eq!(screen.style_at(x + 2, row), Style { fg: None, bold: false, reverse: true });
}

#[test]
fn the_selected_row_is_lit_across_the_width_in_one_colour() {
    let board = board();
    let size = (160, 40);
    // The second lane of the fixture, `rewrite`, is the selected row.
    let screen_of = crate::screen(Tab::Now, 1, size);
    let screen = canvas(&board, &screen_of, None);
    let text = screen.text();
    let (row, _) = find(&text, "› rewrite");
    let lit = Style { fg: None, bold: false, reverse: true };
    for column in 1..size.0 - 1 {
        assert_eq!(screen.style_at(column, row), lit, "column {column} of the lit row: {text}");
    }
    assert_eq!(screen.style_at(0, row).reverse, false, "the mark's column is not lit: {text}");
    assert_eq!(screen.style_at(size.0 - 1, row).reverse, false, "the last column stays: {text}");
    // No other row carries a reverse cell, but the first row (the shown
    // voyage's label is lit, README change 1) and the row of the tabs (the
    // open tab is in reverse video).
    let tab_row = gear_board::frame::layout(&board, &screen_of).tab_row;
    for y in 0..size.1 {
        if y == row || y == 0 || y == tab_row {
            continue;
        }
        for x in 0..size.0 {
            assert!(!screen.style_at(x, y).reverse, "row {y} column {x} is in reverse: {text}");
        }
    }
}

#[test]
fn a_lane_cycles_the_prototypes_lane_colours() {
    // README: the lanes are cyan, magenta, yellow, blue, in the lane order
    // hull, deck, keel, mast.
    let lanes = ["hull", "deck", "keel", "mast"];
    assert_eq!(lane_color(&lanes, "hull"), Color::Acc);
    assert_eq!(lane_color(&lanes, "deck"), Color::Mag);
    assert_eq!(lane_color(&lanes, "keel"), Color::Warn);
    assert_eq!(lane_color(&lanes, "mast"), Color::Blu);
    // The fifth lane starts the cycle again, and a lane that is not in the
    // order is grey.
    let five = ["a", "b", "c", "d", "e"];
    assert_eq!(lane_color(&five, "c"), Color::Warn);
    assert_eq!(lane_color(&five, "e"), Color::Acc);
    assert_eq!(lane_color(&five, "f"), Color::Dim);
}

#[test]
fn a_merged_task_is_green_in_the_graph() {
    let board = board();
    let project = &board.projects[0];
    let lines = styled_lines(&project.graph, &project.graph_lanes, None);
    let line = lines.iter().find(|line| line.text.contains(" T-1 ")).expect("the row of T-1");
    let cells: Vec<(char, Style)> = line.cells().collect();
    // The merge into dev and the review's ✓ are green and bold.
    assert_eq!(cells[3], ('●', Style::fg(Color::Ok).and_bold()), "{:?}", line.text);
    assert_eq!(cells[9], ('✓', Style::fg(Color::Ok).and_bold()), "{:?}", line.text);
    // The working task's symbol is yellow, the dev column's head green.
    let line = lines.iter().find(|line| line.text.contains(" T-2 ")).expect("the row of T-2");
    let at = line.text.chars().position(|c| c == '◉').unwrap();
    assert_eq!(line.styles[at], Style::fg(Color::Warn).and_bold());
    assert_eq!(lines[0].styles[2], Style::fg(Color::Ok).and_bold());
    // The terminal gets the git graph in the same colours: the merge `●` at
    // dev's column is bold green and the working task's `◉` bold yellow
    // (T-362). The selected row is lit in one colour (README change 13), so
    // each is checked on the frame that does not light it.
    let ansi = canvas(&board, &crate::screen(Tab::Tasks, 1, (160, 40)), None).ansi(true);
    assert!(ansi.contains("\u{1b}[1;32m●\u{1b}[0m"), "{ansi:?}");
    let ansi = canvas(&board, &crate::screen(Tab::Tasks, 0, (160, 40)), None).ansi(true);
    assert!(ansi.contains("\u{1b}[1;33m◉\u{1b}[0m"), "{ansi:?}");
}

#[test]
fn no_color_draws_no_colour_code() {
    assert!(colour_allowed(None));
    assert!(colour_allowed(Some(std::ffi::OsStr::new(""))));
    assert!(!colour_allowed(Some(std::ffi::OsStr::new("1"))));

    let board = board();
    for tab in Tab::all() {
        let screen = canvas(&board, &crate::screen(tab, 0, (160, 40)), None);
        let coloured = screen.ansi(true);
        let plain = screen.ansi(false);
        assert!(coloured.contains("\u{1b}[3") || coloured.contains("\u{1b}[9"), "{tab:?} has colour");
        // No foreground colour at all: only bold and the reverse video of the
        // open tab are left.
        let codes: Vec<&str> = plain.split('\u{1b}').skip(1).map(|code| code.split('m').next().unwrap()).collect();
        for code in &codes {
            assert!(
                ["[0", "[1", "[7", "[1;7"].contains(code),
                "{tab:?}: a code that is not bold or reverse: {code:?}"
            );
        }
        assert!(
            plain.contains("\u{1b}[1;7m"),
            "the open tab stays in reverse video, bold (T-356): {plain:?}"
        );
    }
    // The plain frame carries no code at all, and neither does the JSON.
    assert!(!frame(&board, &crate::screen(Tab::Now, 0, (160, 40)), None).contains('\u{1b}'));
    assert!(!to_json(&board).unwrap().contains('\u{1b}'));
}

/// Keys that say `q` at once.
struct Quit;

impl Keys for Quit {
    fn next_event(&mut self, _timeout: Duration) -> Option<Event> {
        Some(Event::Key('q'))
    }

    fn size(&self) -> (usize, usize) {
        (160, 40)
    }
}

#[test]
fn the_board_draws_in_colour_unless_told_not_to() {
    let mut store = world();
    for (color, coloured) in [(true, true), (false, false)] {
        let opts = Options { project: Some("p".into()), color, ..Options::default() };
        let mut out: Vec<u8> = Vec::new();
        run_with(&mut store, &opts, &mut out, &mut Quit, None).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.contains("\u{1b}[33m"), coloured, "{text:?}");
        assert!(text.contains("\u{1b}[7m"), "{text:?}");
    }
}

/// A long lane name never moves the text columns of a view: the text after
/// the drawing starts at the same column on every row, whatever the names are.
/// The Tasks view's lane heads are six cells apart as the prototype writes them
/// (T-362), so a long name there runs into the next head as it does in the
/// prototype; the text column after the drawing still holds.
#[test]
fn a_long_lane_name_never_moves_the_text_columns() {
    let board = board();
    let size = (200, 40);

    // Tasks: the text after the drawing starts at the same column on every
    // row, whatever the lane names are. The heads name the lanes six cells
    // apart as the prototype writes them, so the long name is cut by the next
    // head (T-362).
    let graph = frame(&board, &crate::screen(Tab::Tasks, 0, size), None);
    let long = line_with(&graph, " T-2 ");
    let short = line_with(&graph, " T-1 ");
    assert_eq!(column(long, "T-2 "), column(short, "T-1 "), "{graph}");
    assert_eq!(column(long, "Give internal"), column(short, "T-1 ") + 7, "{graph}");
    let head = line_with(&graph, "dev");
    assert!(head.contains("rewrit") && head.contains("hull"), "{head:?}");

    // Now: the middle column starts at the same place for both lanes.
    let now = frame(&board, &crate::screen(Tab::Now, 0, size), None);
    assert!(now.contains("rewrite T-2 Give internal"), "{now}");
    let long = line_with(&now, "rewrite T-2");
    let short = line_with(&now, "› hull");
    assert_eq!(column(long, "T-2"), column(short, "hull") + "rewrite".len() + 1, "{now}");

    // Decisions: the friction's status follows the lane after a space.
    let decisions = frame(&board, &crate::screen(Tab::Log, 0, size), None);
    let long = line_with(&decisions, "friction of rewrite");
    let short = line_with(&decisions, "friction of hull");
    assert!(long.contains("T-2 rewrite unread"), "{decisions}");
    assert_eq!(column(long, "unread"), column(short, "unread"), "{decisions}");

    // Cost: the lane names are padded to one width, so the estimates line up
    // under their heading whichever lane the row names.
    let cost = frame(&board, &crate::screen(Tab::Cost, 0, size), None);
    let long = line_with(&cost, "rewrite");
    let short = line_with(&cost, "hull");
    assert!(long.contains("▸ rewrite"), "{cost}");
    assert_eq!(column(long, "30m"), column(short, "40m"), "{cost}");
    let heading = line_with(&cost, "lane / task");
    assert_eq!(column(heading, "est"), column(long, "30m"), "{cost}");
}
