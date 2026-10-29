//! The golden contract: the frame the board draws is the prototype's frame.
//!
//! T-340: a frame depends only on the store and the `now` it was built at.
//! The header's clock is the `now` the board was built with, never the wall
//! clock: the board is drawn twice with the same `now` and reads the same.
//! The `now` is 90 s behind the wall clock, so a build that still read the
//! clock would put another minute on the title's row.
//!
//! T-345: the gate. The board, drawn from the prototype's own fixture, is
//! compared with the prototype's golden frames cell by cell — the character,
//! the colour and the reverse mark — so a missing colour or a wrong width
//! fails the gate and names the cell.

use gear_board::frame::{canvas, Tab};
use gear_board::golden::{self as gold, Planes};
use gear_board::idle;
use gear_model::TaskState;
use gear_store::{testing, Store, TaskRow};

const NOW: i64 = 1_800_000_000;

/// Now, in epoch seconds.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// A project with one open task.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: Some("hull".into()),
                milestone: Some("M4".into()),
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(20),
                issue: None,
                updated: NOW,
            })?;
            Ok(())
        })
        .unwrap();
    store
}

#[test]
fn two_builds_with_the_same_now_draw_the_same_frame() {
    // 90 s apart on the wall clock: a frame that read the clock would differ.
    let now = now_seconds() - 90;
    let mut store = world();
    let first = gear_board::build(
        &mut store,
        &gear_board::Options { project: Some("p".into()), ..Default::default() },
        now,
    )
    .unwrap();
    let second = gear_board::build(
        &mut store,
        &gear_board::Options { project: Some("p".into()), ..Default::default() },
        now,
    )
    .unwrap();

    let first_frame = canvas(&first, &crate::screen(Tab::Now, 0, (120, 40)), None).text();
    let second_frame = canvas(&second, &crate::screen(Tab::Now, 0, (120, 40)), None).text();

    assert_eq!(first.now, now, "the board keeps the now it was built at");
    assert_eq!(first_frame, second_frame, "the frame depends on the store and now only");
    let title = first_frame.lines().next().unwrap();
    let clock = idle::clock(now);
    assert!(
        title.trim_end().ends_with(&clock),
        "row 0 ends with the clock of now ({clock}): {title:?}"
    );
}

// ---------------------------------------------------------------------------
// T-345: the gate compares the real board with the prototype cell by cell.
// ---------------------------------------------------------------------------

/// The name of a frame: the name of its golden files.
type Frame = &'static str;
/// A cell range, inclusive, counted from zero.
type Range = (usize, usize);

/// One place where the board and the prototype differ, and what closes the
/// gap.
///
/// A cell inside a skip entry (`swap: None`) is not compared. A cell inside a
/// swap entry and inside no skip entry is compared, except that the golden
/// colour `want` matches the board's `got` (case kept: `u` with `m`).
struct Pending {
    /// The frame (`now-folded`, …): the name of its golden files.
    frame: Frame,
    /// The rows of the entry, inclusive, counted from zero.
    rows: Range,
    /// The columns of the entry, inclusive, counted from zero.
    cols: Range,
    /// The golden colour `want` and the board colour `got` that stands for it.
    swap: Option<(char, char)>,
    /// The task that removes the entry, or why it stays.
    by: &'static str,
}

/// What is not built yet, and the prototype's data that no store can give.
///
/// The entries of (5) are the tasks queued behind this one: each removes its
/// own rows as it builds them. The `prototype data` entries stay until the
/// user's data fix regenerates the frames.
const PENDING: &[Pending] = &[
    // (5) The rows the queued tasks build. Row 47 (the footer) is compared
    // from the start: T-341 drew it.
    Pending { frame: "now-folded", rows: (0, 6), cols: (0, 159), swap: None, by: "T-353" },
    Pending { frame: "now-open", rows: (0, 6), cols: (0, 159), swap: None, by: "T-353" },
    Pending { frame: "tasks-folded", rows: (0, 6), cols: (0, 159), swap: None, by: "T-353" },
    Pending { frame: "tasks-open", rows: (0, 6), cols: (0, 159), swap: None, by: "T-353" },
    Pending { frame: "cost-folded", rows: (0, 6), cols: (0, 159), swap: None, by: "T-353" },
    Pending { frame: "cost-open", rows: (0, 6), cols: (0, 159), swap: None, by: "T-353" },
    Pending { frame: "log-folded", rows: (0, 6), cols: (0, 159), swap: None, by: "T-353" },
    Pending { frame: "log-open", rows: (0, 6), cols: (0, 159), swap: None, by: "T-353" },

    Pending { frame: "now-folded", rows: (7, 9), cols: (0, 159), swap: None, by: "T-354" },
    Pending { frame: "tasks-folded", rows: (7, 9), cols: (0, 159), swap: None, by: "T-354" },
    Pending { frame: "cost-folded", rows: (7, 9), cols: (0, 159), swap: None, by: "T-354" },
    Pending { frame: "log-folded", rows: (7, 9), cols: (0, 159), swap: None, by: "T-354" },
    Pending { frame: "now-open", rows: (7, 17), cols: (0, 159), swap: None, by: "T-355" },
    Pending { frame: "tasks-open", rows: (7, 17), cols: (0, 159), swap: None, by: "T-355" },
    Pending { frame: "cost-open", rows: (7, 17), cols: (0, 159), swap: None, by: "T-355" },
    Pending { frame: "log-open", rows: (7, 17), cols: (0, 159), swap: None, by: "T-355" },
    Pending { frame: "now-folded", rows: (10, 12), cols: (0, 159), swap: None, by: "T-356" },
    Pending { frame: "tasks-folded", rows: (10, 12), cols: (0, 159), swap: None, by: "T-356" },
    Pending { frame: "cost-folded", rows: (10, 12), cols: (0, 159), swap: None, by: "T-356" },
    Pending { frame: "log-folded", rows: (10, 12), cols: (0, 159), swap: None, by: "T-356" },
    Pending { frame: "now-open", rows: (18, 20), cols: (0, 159), swap: None, by: "T-356" },
    Pending { frame: "tasks-open", rows: (18, 20), cols: (0, 159), swap: None, by: "T-356" },
    Pending { frame: "cost-open", rows: (18, 20), cols: (0, 159), swap: None, by: "T-356" },
    Pending { frame: "log-open", rows: (18, 20), cols: (0, 159), swap: None, by: "T-356" },

    Pending { frame: "now-folded", rows: (13, 18), cols: (0, 159), swap: None, by: "T-359" },
    Pending { frame: "now-open", rows: (21, 26), cols: (0, 159), swap: None, by: "T-359" },
    Pending { frame: "now-folded", rows: (19, 46), cols: (0, 159), swap: None, by: "T-360" },
    Pending { frame: "now-open", rows: (27, 46), cols: (0, 159), swap: None, by: "T-360" },

    Pending { frame: "tasks-folded", rows: (13, 46), cols: (0, 159), swap: None, by: "T-362" },
    Pending { frame: "tasks-open", rows: (21, 46), cols: (0, 159), swap: None, by: "T-362" },
    Pending { frame: "cost-folded", rows: (13, 46), cols: (0, 159), swap: None, by: "T-363" },
    Pending { frame: "cost-open", rows: (21, 46), cols: (0, 159), swap: None, by: "T-363" },
    Pending { frame: "log-folded", rows: (13, 46), cols: (0, 159), swap: None, by: "T-364" },
    Pending { frame: "log-open", rows: (21, 46), cols: (0, 159), swap: None, by: "T-364" },

    // (6) The prototype's constants contradict its own formulas, so no store
    // can give both.
    //
    // Its projected finish (455 min, slow lane `mast`) against its own
    // forecast (346 min, lane `hull`).
    Pending { frame: "now-folded", rows: (8, 8), cols: (43, 84), swap: None, by: "prototype data: expected finish" },
    Pending { frame: "tasks-folded", rows: (8, 8), cols: (43, 84), swap: None, by: "prototype data: expected finish" },
    Pending { frame: "cost-folded", rows: (8, 8), cols: (43, 84), swap: None, by: "prototype data: expected finish" },
    Pending { frame: "log-folded", rows: (8, 8), cols: (43, 84), swap: None, by: "prototype data: expected finish" },
    Pending { frame: "now-open", rows: (13, 13), cols: (51, 136), swap: None, by: "prototype data: expected finish" },
    Pending { frame: "tasks-open", rows: (13, 13), cols: (51, 136), swap: None, by: "prototype data: expected finish" },
    Pending { frame: "cost-open", rows: (13, 13), cols: (51, 136), swap: None, by: "prototype data: expected finish" },
    Pending { frame: "log-open", rows: (13, 13), cols: (51, 136), swap: None, by: "prototype data: expected finish" },
    // Its `finished` clocks are 11 min before departure + WHEN's merge minute.
    Pending { frame: "tasks-folded", rows: (23, 23), cols: (144, 158), swap: None, by: "prototype data: merge clocks" },
    Pending { frame: "tasks-folded", rows: (26, 26), cols: (144, 158), swap: None, by: "prototype data: merge clocks" },
    Pending { frame: "tasks-folded", rows: (30, 30), cols: (144, 158), swap: None, by: "prototype data: merge clocks" },
    Pending { frame: "tasks-folded", rows: (33, 33), cols: (144, 158), swap: None, by: "prototype data: merge clocks" },
    Pending { frame: "tasks-folded", rows: (35, 35), cols: (144, 158), swap: None, by: "prototype data: merge clocks" },
    Pending { frame: "tasks-open", rows: (31, 31), cols: (144, 158), swap: None, by: "prototype data: merge clocks" },
    Pending { frame: "tasks-open", rows: (34, 34), cols: (144, 158), swap: None, by: "prototype data: merge clocks" },
    Pending { frame: "tasks-open", rows: (38, 38), cols: (144, 158), swap: None, by: "prototype data: merge clocks" },
    Pending { frame: "tasks-open", rows: (41, 41), cols: (144, 158), swap: None, by: "prototype data: merge clocks" },
    Pending { frame: "tasks-open", rows: (43, 43), cols: (144, 158), swap: None, by: "prototype data: merge clocks" },
    // T-267 is queued although T-263 and T-266 are not merged.
    Pending { frame: "tasks-folded", rows: (19, 19), cols: (92, 122), swap: None, by: "prototype data: T-267 stage" },
    Pending { frame: "tasks-open", rows: (27, 27), cols: (92, 122), swap: None, by: "prototype data: T-267 stage" },
    // The prototype colours lane `mast` blue by name, the board colours lanes
    // by their order: its blue is the board's magenta, bold for bold.
    Pending { frame: "now-folded", rows: (0, 47), cols: (0, 159), swap: Some(('u', 'm')), by: "prototype data: lane colour" },
    Pending { frame: "now-open", rows: (0, 47), cols: (0, 159), swap: Some(('u', 'm')), by: "prototype data: lane colour" },
    Pending { frame: "tasks-folded", rows: (0, 47), cols: (0, 159), swap: Some(('u', 'm')), by: "prototype data: lane colour" },
    Pending { frame: "tasks-open", rows: (0, 47), cols: (0, 159), swap: Some(('u', 'm')), by: "prototype data: lane colour" },
    Pending { frame: "cost-folded", rows: (0, 47), cols: (0, 159), swap: Some(('u', 'm')), by: "prototype data: lane colour" },
    Pending { frame: "cost-open", rows: (0, 47), cols: (0, 159), swap: Some(('u', 'm')), by: "prototype data: lane colour" },
];

/// The eight frames the prototype draws: `(name, tab, detail)`.
const FRAMES: &[(Frame, Tab, bool)] = &[
    ("now-folded", Tab::Now, false),
    ("now-open", Tab::Now, true),
    ("tasks-folded", Tab::Tasks, false),
    ("tasks-open", Tab::Tasks, true),
    ("cost-folded", Tab::Cost, false),
    ("cost-open", Tab::Cost, true),
    ("log-folded", Tab::Log, false),
    ("log-open", Tab::Log, true),
];

/// The tab and the detail flag of a frame.
fn frame_of(frame: Frame) -> (Tab, bool) {
    let (_, tab, detail) = *FRAMES
        .iter()
        .find(|(name, _, _)| *name == frame)
        .unwrap_or_else(|| panic!("the frame {frame} is one of the eight"));
    (tab, detail)
}

/// The board as the prototype draws it: the fixture's store, the engine
/// running (the fixture has no engine and the prototype draws no warning).
fn render(tab: Tab, detail: bool) -> Planes {
    use gear_board::frame::{canvas, Screen};
    use gear_board::EngineState;

    let (mut store, now) = crate::golden_fixture::load();
    let mut board = gear_board::build(
        &mut store,
        &gear_board::Options { project: Some("gear".into()), ..Default::default() },
        now,
    )
    .unwrap();
    board.engine = EngineState::Running;
    let screen = Screen { tab, selected: 0, detail, size: (160, 48) };
    gold::planes(&canvas(&board, &screen, None))
}

/// One plane of golden files: `name.txt`, `name.col` or `name.rev`, split on
/// `\n` with a last empty piece dropped.
fn plane(name: &str, ext: &str) -> Vec<String> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../gear/voyages/M5/board/golden/");
    let text = std::fs::read_to_string(format!("{dir}{name}.{ext}"))
        .unwrap_or_else(|e| panic!("the golden file {name}.{ext}: {e}"));
    let mut rows: Vec<&str> = text.split('\n').collect();
    if rows.last() == Some(&"") {
        rows.pop();
    }
    rows.into_iter().map(|row| row.to_string()).collect()
}

/// The prototype's frame `name`, read from its golden files.
fn golden(name: &str) -> Planes {
    Planes { text: plane(name, "txt"), colour: plane(name, "col"), reverse: plane(name, "rev") }
}

/// One cell of a frame: what it draws and how.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    text: char,
    colour: char,
    reverse: char,
}

/// The cells of three planes, row by row.
fn cells(planes: &Planes) -> Vec<Vec<Cell>> {
    let plane = |rows: &[String], row: usize| -> Vec<char> {
        rows.get(row).map(|row| row.chars().collect()).unwrap_or_default()
    };
    planes
        .text
        .iter()
        .enumerate()
        .map(|(r, text)| {
            let colour = plane(&planes.colour, r);
            let reverse = plane(&planes.reverse, r);
            let at = |cells: &[char], c: usize| cells.get(c).copied().unwrap_or(' ');
            text.chars()
                .enumerate()
                .map(|(c, text)| Cell { text, colour: at(&colour, c), reverse: at(&reverse, c) })
                .collect()
        })
        .collect()
}

/// Whether `(row, col)` is inside `entry`.
fn inside(entry: &Pending, row: usize, col: usize) -> bool {
    let (first_row, last_row) = entry.rows;
    let (first_col, last_col) = entry.cols;
    (first_row..=last_row).contains(&row) && (first_col..=last_col).contains(&col)
}

/// What the pending entries of `frame` say about `(row, col)`: whether a skip
/// entry holds it, and the colour swap that stands for the prototype's.
fn pending_at(pending: &[Pending], frame: Frame, row: usize, col: usize) -> (bool, Option<(char, char)>) {
    let mut skip = false;
    let mut swap = None;
    for entry in pending.iter().filter(|entry| entry.frame == frame && inside(entry, row, col)) {
        match entry.swap {
            None => skip = true,
            Some(pair) => swap = Some(pair),
        }
    }
    (skip, swap)
}

/// Whether the board's `got` cell draws the prototype's `want` cell: the same
/// character and reverse mark, and its colour — unless `swap` stands for it.
fn matches(want: &Cell, got: &Cell, swap: Option<(char, char)>) -> bool {
    want.text == got.text && want.reverse == got.reverse && colour_matches(want.colour, got.colour, swap)
}

/// Whether the board's `got` colour letter stands for the prototype's `want`
/// colour: the same letter, or the declared `swap` — which keeps its case, so a
/// plain lane's `u` stands for `m` and a bold lane's `U` for `M`, never `U`
/// for `m`.
fn colour_matches(want: char, got: char, swap: Option<(char, char)>) -> bool {
    if want == got {
        return true;
    }
    let Some((want_letter, got_letter)) = swap else {
        return false;
    };
    (want == want_letter && got == got_letter)
        || (want == want_letter.to_ascii_uppercase() && got == got_letter.to_ascii_uppercase())
}

/// Why `frame` fails the gate: the cells that differ and the pending entries
/// that match now. An empty `Vec` is a pass.
fn failures(frame: Frame, want: &Planes, got: &Planes, pending: &[Pending]) -> Vec<String> {
    let (want_cells, got_cells) = (cells(want), cells(got));
    let mut problems = Vec::new();
    let mut differences = Vec::new();
    let mut differing = 0;
    for row in 0..want_cells.len() {
        for col in 0..want_cells[row].len() {
            let (skip, swap) = pending_at(pending, frame, row, col);
            if skip {
                continue;
            }
            let (want_cell, got_cell) = (want_cells[row][col], got_cells[row][col]);
            if matches(&want_cell, &got_cell, swap) {
                continue;
            }
            differing += 1;
            if differences.len() < 30 {
                differences.push(format!(
                    "row {row} col {col}: want {:?} {} {} got {:?} {} {}",
                    want_cell.text,
                    want_cell.colour,
                    want_cell.reverse,
                    got_cell.text,
                    got_cell.colour,
                    got_cell.reverse,
                ));
            }
        }
    }
    if differing > 0 {
        problems.push(format!(
            "{frame} differs from the prototype in {differing} cell(s), the first {}:",
            differences.len(),
        ));
        problems.extend(differences);
    }
    // A "NEW:" entry whose cells match now is stale: its task built the rows
    // and did not remove the entry. The `prototype data` entries are not
    // checked this way: they go when the frames are regenerated.
    for entry in pending.iter().filter(|entry| entry.frame == frame && entry.by.starts_with("NEW:")) {
        let matches_now = (entry.rows.0..=entry.rows.1).all(|row| {
            (entry.cols.0..=entry.cols.1).all(|col| {
                let (want_cell, got_cell) = (want_cells[row][col], got_cells[row][col]);
                // Every cell has to match without the swap: a swap entry whose
                // cell still needs the swap has not arrived either.
                matches(&want_cell, &got_cell, None)
            })
        });
        if matches_now {
            problems.push(format!(
                "pending {} on {frame} rows {}-{} matches now: remove the entry",
                entry.by, entry.rows.0, entry.rows.1,
            ));
        }
    }
    problems
}

/// The gate for one frame: the board against the prototype, cell by cell.
fn gate(frame: Frame) {
    let (tab, detail) = frame_of(frame);
    let problems = failures(frame, &golden(frame), &render(tab, detail), PENDING);
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn now_folded() {
    gate("now-folded");
}

#[test]
fn now_open() {
    gate("now-open");
}

#[test]
fn tasks_folded() {
    gate("tasks-folded");
}

#[test]
fn tasks_open() {
    gate("tasks-open");
}

#[test]
fn cost_folded() {
    gate("cost-folded");
}

#[test]
fn cost_open() {
    gate("cost-open");
}

#[test]
fn log_folded() {
    gate("log-folded");
}

#[test]
fn log_open() {
    gate("log-open");
}

#[test]
fn a_difference_is_reported() {
    // One colour letter at row 3 col 7, in a frame with no pending entry: the
    // gate names the cell.
    let want = golden("now-folded");
    let mut got = want.clone();
    let row: Vec<char> = got.colour[3].chars().collect();
    let mut changed = row.clone();
    changed[7] = if row[7] == 'm' { 'M' } else { 'm' };
    got.colour[3] = changed.into_iter().collect();

    let problems = failures("uncharted", &want, &got, &[]);

    assert!(
        problems.iter().any(|line| line.contains("row 3 col 7")),
        "the gate names row 3 col 7: {problems:?}"
    );
}

/// A frame of one cell (`x`) in `colour`, upright: the gate's smallest case.
fn one_cell(colour: char) -> Planes {
    Planes {
        text: vec!["x".to_string()],
        colour: vec![colour.to_string()],
        reverse: vec![".".to_string()],
    }
}

/// The removal message of a `NEW:` entry that its frame has caught up with.
const RETIRED: &str = "pending NEW: task on probe rows 0-0 matches now: remove the entry";

/// A `NEW:` skip entry over the one cell of the probe frame.
fn new_entry(swap: Option<(char, char)>) -> Pending {
    Pending { frame: "probe", rows: (0, 0), cols: (0, 0), swap, by: "NEW: task" }
}

#[test]
fn a_new_entry_that_matches_now_is_retired() {
    // The review of T-345, F1: the entry goes once the cell matches, and the
    // gate has to say so — the inverted predicate kept it forever.
    let problems = failures("probe", &one_cell('u'), &one_cell('u'), &[new_entry(None)]);
    assert_eq!(problems, vec![RETIRED.to_string()]);
}

#[test]
fn a_new_entry_whose_cell_still_differs_stays() {
    // A skip entry leaves its cell uncompared, so the gate says nothing — and
    // the entry stays: its task has not built the cell yet.
    let problems = failures("probe", &one_cell('u'), &one_cell('m'), &[new_entry(None)]);
    assert!(problems.is_empty(), "a skip entry hides its cell and is not retired: {problems:?}");
}

#[test]
fn a_new_swap_entry_is_retired_only_when_the_swap_is_not_needed() {
    // The board draws the prototype's own colour: the entry has nothing left
    // to stand for, so it goes.
    let problems = failures("probe", &one_cell('u'), &one_cell('u'), &[new_entry(Some(('u', 'm')))]);
    assert_eq!(problems, vec![RETIRED.to_string()]);
    // The cell still needs the swap: the entry stays (and the cell matches).
    let problems = failures("probe", &one_cell('u'), &one_cell('m'), &[new_entry(Some(('u', 'm')))]);
    assert!(problems.is_empty(), "the swap covers the cell and keeps the entry: {problems:?}");
}

#[test]
fn a_lane_colour_swap_keeps_the_case() {
    // The review of T-345, F2: the prototype's blue is the board's magenta,
    // bold for bold — `u` stands for `m` and `U` for `M`, never across cases.
    let cell = |colour| Cell { text: 'x', colour, reverse: '.' };
    let swap = Some(('u', 'm'));
    assert!(matches(&cell('u'), &cell('m'), swap), "a plain lane");
    assert!(matches(&cell('U'), &cell('M'), swap), "a bold lane");
    assert!(!matches(&cell('U'), &cell('m'), swap), "a bold prototype letter needs the bold board letter");
    assert!(!matches(&cell('u'), &cell('M'), swap), "and a plain one the plain");
}
