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
        &gear_board::Options {
            project: Some("p".into()),
            ..Default::default()
        },
        now,
    )
    .unwrap();
    let second = gear_board::build(
        &mut store,
        &gear_board::Options {
            project: Some("p".into()),
            ..Default::default()
        },
        now,
    )
    .unwrap();

    let first_frame = canvas(&first, &crate::screen(Tab::Now, 0, (120, 40)), None).text();
    let second_frame = canvas(&second, &crate::screen(Tab::Now, 0, (120, 40)), None).text();

    assert_eq!(first.now, now, "the board keeps the now it was built at");
    assert_eq!(
        first_frame, second_frame,
        "the frame depends on the store and now only"
    );
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
#[derive(Clone, Copy)]
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
/// (5) is empty: the header and the tab bar are the prototype's height, so
/// every view is compared from its own first row (T-356). The `prototype
/// data` entries stay until the user's data fix regenerates the frames.
const PENDING: &[Pending] = &[
    // (5) Every view's rows are compared from the first row of the view: the
    // tab bar and the header's height are the prototype's (T-356), so no
    // exclusion is left for the header. Row 47 (the footer) is compared from
    // the start: T-341 drew it.

    // T-494: the Tasks tab has a deps column right of the lanes, and the task
    // text follows it. The prototype has no deps column, so its Tasks frames
    // cannot match the view's right side; the lane graph left of the column is
    // still compared.
    // T-494: the Tasks tab has a deps column right of the lanes, the task text
    // follows it, the stage's dependency words are a count and the right-hand
    // dependency text is gone. The prototype has none of these, so its Tasks
    // frames cannot match the view's right side; the lane graph left of the
    // deps column is still compared.
    Pending {
        frame: "tasks-folded",
        rows: (14, 46),
        cols: (25, 159),
        swap: None,
        by: "T-494: the deps column and the task text after it",
    },
    Pending {
        frame: "tasks-open",
        rows: (22, 46),
        cols: (25, 159),
        swap: None,
        by: "T-494: the deps column and the task text after it",
    },
    // (6) The prototype's constants contradict its own formulas, so no store
    // can give both.
    //
    // Its projected finish (455 min, slow lane `mast`) against its own
    // forecast (346 min, lane `hull`).
    Pending {
        frame: "now-folded",
        rows: (8, 8),
        cols: (43, 84),
        swap: None,
        by: "prototype data: expected finish",
    },
    Pending {
        frame: "tasks-folded",
        rows: (8, 8),
        cols: (43, 84),
        swap: None,
        by: "prototype data: expected finish",
    },
    Pending {
        frame: "cost-folded",
        rows: (8, 8),
        cols: (43, 84),
        swap: None,
        by: "prototype data: expected finish",
    },
    Pending {
        frame: "log-folded",
        rows: (8, 8),
        cols: (43, 84),
        swap: None,
        by: "prototype data: expected finish",
    },
    Pending {
        frame: "now-open",
        rows: (13, 13),
        cols: (51, 136),
        swap: None,
        by: "prototype data: expected finish",
    },
    Pending {
        frame: "tasks-open",
        rows: (13, 13),
        cols: (51, 136),
        swap: None,
        by: "prototype data: expected finish",
    },
    Pending {
        frame: "cost-open",
        rows: (13, 13),
        cols: (51, 136),
        swap: None,
        by: "prototype data: expected finish",
    },
    Pending {
        frame: "log-open",
        rows: (13, 13),
        cols: (51, 136),
        swap: None,
        by: "prototype data: expected finish",
    },
    // Its stations carry the prototype's own minutes from the departure: S1
    // reached at 3h00m, S2 planned by 5h30m, S3 by 8h00m. The board reads them
    // from its data instead (a reached station's last merge, a station ahead
    // placed by its minutes left), so only the three `<hm>` cells differ: the
    // rectangle spans them, the words and the layout match.
    Pending {
        frame: "now-open",
        rows: (10, 12),
        cols: (52, 71),
        swap: None,
        by: "prototype data: station times",
    },
    Pending {
        frame: "tasks-open",
        rows: (10, 12),
        cols: (52, 71),
        swap: None,
        by: "prototype data: station times",
    },
    Pending {
        frame: "cost-open",
        rows: (10, 12),
        cols: (52, 71),
        swap: None,
        by: "prototype data: station times",
    },
    Pending {
        frame: "log-open",
        rows: (10, 12),
        cols: (52, 71),
        swap: None,
        by: "prototype data: station times",
    },
    // Its `finished` clocks are 11 min before departure + WHEN's merge minute.
    Pending {
        frame: "tasks-folded",
        rows: (23, 23),
        cols: (144, 158),
        swap: None,
        by: "prototype data: merge clocks",
    },
    Pending {
        frame: "tasks-folded",
        rows: (26, 26),
        cols: (144, 158),
        swap: None,
        by: "prototype data: merge clocks",
    },
    Pending {
        frame: "tasks-folded",
        rows: (30, 30),
        cols: (144, 158),
        swap: None,
        by: "prototype data: merge clocks",
    },
    Pending {
        frame: "tasks-folded",
        rows: (33, 33),
        cols: (144, 158),
        swap: None,
        by: "prototype data: merge clocks",
    },
    Pending {
        frame: "tasks-folded",
        rows: (35, 35),
        cols: (144, 158),
        swap: None,
        by: "prototype data: merge clocks",
    },
    Pending {
        frame: "tasks-open",
        rows: (31, 31),
        cols: (144, 158),
        swap: None,
        by: "prototype data: merge clocks",
    },
    Pending {
        frame: "tasks-open",
        rows: (34, 34),
        cols: (144, 158),
        swap: None,
        by: "prototype data: merge clocks",
    },
    Pending {
        frame: "tasks-open",
        rows: (38, 38),
        cols: (144, 158),
        swap: None,
        by: "prototype data: merge clocks",
    },
    Pending {
        frame: "tasks-open",
        rows: (41, 41),
        cols: (144, 158),
        swap: None,
        by: "prototype data: merge clocks",
    },
    Pending {
        frame: "tasks-open",
        rows: (43, 43),
        cols: (144, 158),
        swap: None,
        by: "prototype data: merge clocks",
    },
    // T-267 is queued although T-263 and T-266 are not merged. Its row is the
    // first of the waiting ones, the newest first (T-362's regeneration of the
    // Tasks frames).
    Pending {
        frame: "tasks-folded",
        rows: (15, 15),
        cols: (92, 122),
        swap: None,
        by: "prototype data: T-267 stage",
    },
    Pending {
        frame: "tasks-open",
        rows: (23, 23),
        cols: (92, 122),
        swap: None,
        by: "prototype data: T-267 stage",
    },
    // The prototype colours lane `mast` blue by name, the board colours lanes
    // by their order: its blue is the board's magenta, bold for bold. Every
    // frame that draws lanes needs it now that no view's rows are excluded
    // (T-356; T-355 had dropped the unreachable ones).
    Pending {
        frame: "now-folded",
        rows: (0, 47),
        cols: (0, 159),
        swap: Some(('u', 'm')),
        by: "prototype data: lane colour",
    },
    Pending {
        frame: "now-open",
        rows: (0, 47),
        cols: (0, 159),
        swap: Some(('u', 'm')),
        by: "prototype data: lane colour",
    },
    Pending {
        frame: "tasks-folded",
        rows: (0, 47),
        cols: (0, 159),
        swap: Some(('u', 'm')),
        by: "prototype data: lane colour",
    },
    Pending {
        frame: "tasks-open",
        rows: (0, 47),
        cols: (0, 159),
        swap: Some(('u', 'm')),
        by: "prototype data: lane colour",
    },
    Pending {
        frame: "cost-folded",
        rows: (0, 47),
        cols: (0, 159),
        swap: Some(('u', 'm')),
        by: "prototype data: lane colour",
    },
    Pending {
        frame: "cost-open",
        rows: (0, 47),
        cols: (0, 159),
        swap: Some(('u', 'm')),
        by: "prototype data: lane colour",
    },
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
        &gear_board::Options {
            project: Some("gear".into()),
            ..Default::default()
        },
        now,
    )
    .unwrap();
    board.engine = EngineState::Running;
    let screen = Screen {
        tab,
        selected: 0,
        detail,
        opened: Vec::new(),
        size: (160, 48),
    };
    gold::planes(&canvas(&board, &screen, None))
}

/// A line without the terminal's escape sequences.
fn plain_line(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            // A control sequence (`ESC [`, its parameters, its final letter)
            // or a two-character escape: neither prints anything.
            if chars.peek() == Some(&'[') {
                chars.next();
                for character in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&character) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        out.push(character);
    }
    out
}

/// The terminal columns a text takes: the wide glyphs of the board's head (⛵
/// and ⚓, one glyph in two columns) take two, everything else one.
fn terminal_columns(text: &str) -> usize {
    text.chars()
        .map(|character| {
            if matches!(character, '⛵' | '⚓') {
                2
            } else {
                1
            }
        })
        .sum()
}

/// The terminal column a text starts at in a line.
fn column_of(line: &str, needle: &str) -> usize {
    let at = line
        .find(needle)
        .unwrap_or_else(|| panic!("no {needle:?} in {line:?}"));
    terminal_columns(&line[..at])
}

/// The first row of the golden fixture's frame as the terminal shows it.
fn head_row(board: &gear_board::Board, size: (usize, usize)) -> String {
    let ansi = canvas(board, &crate::screen(Tab::Now, 0, size), None).ansi(true);
    plain_line(ansi.lines().next().unwrap_or_default())
}

/// T-353 (review F1): the terminal's columns are the canvas's. A wide glyph
/// (⛵, ⚓) fills two columns, and its continuation cell — a space the canvas
/// keeps so that the golden planes match the prototype — is not printed again.
/// The clock of row 0 sits at column 152 and the second voyage's label starts
/// at column 20; with one more voyage the row is still cut at the terminal's
/// edge.
#[test]
fn a_wide_glyph_fills_two_columns_and_its_continuation_is_not_printed() {
    use gear_board::EngineState;

    let (mut store, now) = crate::golden_fixture::load();
    let mut board = gear_board::build(
        &mut store,
        &gear_board::Options {
            project: Some("gear".into()),
            ..Default::default()
        },
        now,
    )
    .unwrap();
    board.engine = EngineState::Running;
    let size = (160, 48);

    let row = head_row(&board, size);
    assert_eq!(column_of(&row, "14:32"), 152, "the clock's column: {row:?}");
    assert_eq!(
        column_of(&row, "M4.2"),
        20,
        "the second voyage's column: {row:?}"
    );
    assert!(
        terminal_columns(&row) <= 160,
        "the row fits the terminal: {row:?}"
    );
    assert!(
        row.contains(" M5 ⛵  "),
        "the first label keeps its spaces: {row:?}"
    );

    // One more voyage, newest first: the row is cut at the terminal's edge, it
    // never grows past it.
    let extra = {
        let project = &mut board.projects[0];
        let mut extra = project
            .voyage_list
            .last()
            .cloned()
            .expect("a voyage of the fixture");
        extra.milestone = "M3".into();
        project.voyage_list.push(extra.clone());
        extra
    };
    assert_eq!(extra.milestone, "M3");
    let row = head_row(&board, size);
    assert!(
        row.contains(" M3 ⚓ "),
        "the fourth label is drawn: {row:?}"
    );
    assert!(
        terminal_columns(&row) <= 160,
        "the row is {} columns wide: {row:?}",
        terminal_columns(&row)
    );
}

/// T-353 (review F3): a write that lands on a wide glyph's continuation
/// blanks the glyph, so the glyph is never half overwritten. Eight voyages on an
/// 80-column terminal put the labels under the clock: the clock still starts at
/// column `W - 8` and the row is still 80 cells wide.
#[test]
fn a_write_over_a_wide_glyph_blanks_it_and_keeps_the_clock_its_column() {
    use gear_board::EngineState;

    let (mut store, now) = crate::golden_fixture::load();
    let mut board = gear_board::build(
        &mut store,
        &gear_board::Options {
            project: Some("gear".into()),
            ..Default::default()
        },
        now,
    )
    .unwrap();
    board.engine = EngineState::Running;
    // Five more voyages, as the review reproduces it: their labels run into the
    // clock's columns.
    {
        let project = &mut board.projects[0];
        let last = project
            .voyage_list
            .last()
            .cloned()
            .expect("a voyage of the fixture");
        for milestone in ["M3.9", "M3.8", "M3.7", "M3.6x", "M3.5"] {
            let mut extra = last.clone();
            extra.milestone = milestone.into();
            project.voyage_list.push(extra);
        }
    }

    let row = head_row(&board, (80, 48));
    assert_eq!(
        column_of(&row, "14:32"),
        72,
        "the clock's column (W - 8): {row:?}"
    );
    assert!(
        terminal_columns(&row) <= 80,
        "the row is {} columns wide: {row:?}",
        terminal_columns(&row)
    );
    // The glyph the clock landed on is blanked, not half printed: no wide glyph
    // sits in front of the clock's first digit.
    let at = row.find("14:32").unwrap_or(0);
    let before = &row[..at];
    assert!(
        !before.ends_with('⛵') && !before.ends_with('⚓'),
        "no wide glyph in front of the clock: {row:?}"
    );
}

/// One plane of golden files: `name.txt`, `name.col` or `name.rev`, split on
/// `\n` with a last empty piece dropped.
fn plane(name: &str, ext: &str) -> Vec<String> {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../gear/voyages/M5/board/golden/"
    );
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
    Planes {
        text: plane(name, "txt"),
        colour: plane(name, "col"),
        reverse: plane(name, "rev"),
    }
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
        rows.get(row)
            .map(|row| row.chars().collect())
            .unwrap_or_default()
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
                .map(|(c, text)| Cell {
                    text,
                    colour: at(&colour, c),
                    reverse: at(&reverse, c),
                })
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
fn pending_at(
    pending: &[Pending],
    frame: Frame,
    row: usize,
    col: usize,
) -> (bool, Option<(char, char)>) {
    let mut skip = false;
    let mut swap = None;
    for entry in pending
        .iter()
        .filter(|entry| entry.frame == frame && inside(entry, row, col))
    {
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
    want.text == got.text
        && want.reverse == got.reverse
        && colour_matches(want.colour, got.colour, swap)
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

/// The row of a view's TO HANDLE block, when the frame has one: the board's
/// is at its `top + lanes + 3` anchor (T-360), the prototype's where its own
/// frame draws it.
fn block_row(planes: &Planes) -> Option<usize> {
    planes
        .text
        .iter()
        .position(|line| line.starts_with(" TO HANDLE ("))
}

/// Where the task's block starts, relative to the prototype's: the number of
/// rows the board's header is short, and the first prototype row the shift
/// covers (the blank row above the block is part of it).
///
/// A frame without a block, or with the block on the prototype's row, has no
/// shift: every row is compared as it is.
fn block_shift(want: &Planes, got: &Planes) -> Option<(usize, usize)> {
    let (want_block, got_block) = (block_row(want)?, block_row(got)?);
    if want_block <= got_block {
        return None;
    }
    Some((want_block - 1, want_block - got_block))
}

/// Why `frame` fails the gate: the cells that differ and the pending entries
/// that match now. An empty `Vec` is a pass.
fn failures(frame: Frame, want: &Planes, got: &Planes, pending: &[Pending]) -> Vec<String> {
    let (want_cells, got_cells) = (cells(want), cells(got));
    // The task's block is compared at its anchor (T-360, F1 of the review):
    // from its first row on, the board's row is the prototype's less the rows
    // its header is still short. The footer keeps its own row.
    let shift = block_shift(want, got);
    let mut problems = Vec::new();
    let mut differences = Vec::new();
    let mut differing = 0;
    for row in 0..want_cells.len() {
        let got_row = match shift {
            Some((start, amount)) if row >= start && row < want_cells.len() - 1 => row - amount,
            _ => row,
        };
        for col in 0..want_cells[row].len() {
            let (skip, swap) = pending_at(pending, frame, row, col);
            if skip {
                continue;
            }
            let (want_cell, got_cell) = (want_cells[row][col], got_cells[got_row][col]);
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
    for entry in pending
        .iter()
        .filter(|entry| entry.frame == frame && entry.by.starts_with("NEW:"))
    {
        let matches_now = (entry.rows.0..=entry.rows.1).all(|row| {
            let got_row = match shift {
                Some((start, amount)) if row >= start && row < want_cells.len() - 1 => row - amount,
                _ => row,
            };
            (entry.cols.0..=entry.cols.1).all(|col| {
                let (want_cell, got_cell) = (want_cells[row][col], got_cells[got_row][col]);
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
    Pending {
        frame: "probe",
        rows: (0, 0),
        cols: (0, 0),
        swap,
        by: "NEW: task",
    }
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
    assert!(
        problems.is_empty(),
        "a skip entry hides its cell and is not retired: {problems:?}"
    );
}

#[test]
fn a_new_swap_entry_is_retired_only_when_the_swap_is_not_needed() {
    // The board draws the prototype's own colour: the entry has nothing left
    // to stand for, so it goes.
    let problems = failures(
        "probe",
        &one_cell('u'),
        &one_cell('u'),
        &[new_entry(Some(('u', 'm')))],
    );
    assert_eq!(problems, vec![RETIRED.to_string()]);
    // The cell still needs the swap: the entry stays (and the cell matches).
    let problems = failures(
        "probe",
        &one_cell('u'),
        &one_cell('m'),
        &[new_entry(Some(('u', 'm')))],
    );
    assert!(
        problems.is_empty(),
        "the swap covers the cell and keeps the entry: {problems:?}"
    );
}

#[test]
fn a_lane_colour_swap_keeps_the_case() {
    // The review of T-345, F2: the prototype's blue is the board's magenta,
    // bold for bold — `u` stands for `m` and `U` for `M`, never across cases.
    let cell = |colour| Cell {
        text: 'x',
        colour,
        reverse: '.',
    };
    let swap = Some(('u', 'm'));
    assert!(matches(&cell('u'), &cell('m'), swap), "a plain lane");
    assert!(matches(&cell('U'), &cell('M'), swap), "a bold lane");
    assert!(
        !matches(&cell('U'), &cell('m'), swap),
        "a bold prototype letter needs the bold board letter"
    );
    assert!(
        !matches(&cell('u'), &cell('M'), swap),
        "and a plain one the plain"
    );
}

// ---------------------------------------------------------------------------
// T-362: the Tasks view paints the prototype's git graph, from the row the
// prototype's view starts on.
// ---------------------------------------------------------------------------

/// The board of the fixture, as `render` builds it.
fn fixture_board() -> (Store, gear_board::Board) {
    let (mut store, now) = crate::golden_fixture::load();
    let mut board = gear_board::build(
        &mut store,
        &gear_board::Options {
            project: Some("gear".into()),
            ..Default::default()
        },
        now,
    )
    .unwrap();
    board.engine = gear_board::EngineState::Running;
    (store, board)
}

/// The cell differences of `frame`'s rows `rows` between the prototype and the
/// view `tab` painted on top of `top` (the row the prototype's view starts on).
///
/// The lane colour swap of the golden gate is allowed: the prototype colours a
/// lane by its name, the board by its place.
fn view_differences(tab: Tab, frame: Frame, top: usize, rows: Range) -> Vec<String> {
    let (_, board) = fixture_board();
    let mut canvas = gear_board::frame::Canvas::new(160, 48);
    let screen = gear_board::frame::Screen {
        tab,
        selected: 0,
        detail: false,
        opened: Vec::new(),
        size: (160, 48),
    };
    let picks = gear_board::frame::view(&board, &screen, &mut canvas, top);
    // The frame lights the selected row (README change 13).
    if let Some(row) = picks.first() {
        canvas.light_row(*row);
    }
    let (want, got) = (cells(&golden(frame)), cells(&gold::planes(&canvas)));
    // The prototype's own data that no store can give (its merge clocks and
    // T-267's stage) is skipped, as the golden gate skips it.
    let data: Vec<Pending> = PENDING
        .iter()
        .copied()
        .filter(|entry| {
            entry.frame == frame
                && (entry.by.starts_with("prototype data") || entry.by.starts_with("T-494"))
        })
        .collect();
    let mut problems = Vec::new();
    for row in rows.0..=rows.1 {
        for col in 0..want[row].len() {
            if pending_at(&data, frame, row, col).0 {
                continue;
            }
            let (want_cell, got_cell) = (want[row][col], got[row][col]);
            if !matches(&want_cell, &got_cell, Some(('u', 'm'))) {
                problems.push(format!(
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
    problems
}

/// T-360 (F1 of the review): the block sits at its documented anchor, the
/// board's `top + lanes + 3`. The prototype draws it one row lower because the
/// board's header is still short (T-356); the block comparison in `failures`
/// names that one-row difference apart instead of masking the block.
#[test]
fn the_to_handle_block_sits_at_its_anchor() {
    let (_, board) = fixture_board();
    let screen = crate::screen(Tab::Now, 0, (160, 48));
    let layout = gear_board::frame::layout(&board, &screen);
    let lanes = board.projects[0].lanes.len();
    let got = render(Tab::Now, false);
    let got_block = got
        .text
        .iter()
        .position(|line| line.starts_with(" TO HANDLE ("));
    assert_eq!(
        got_block,
        Some(layout.top + lanes + 3),
        "the block starts at top + lanes + 3:\n{}",
        got.text.join("\n")
    );
    let want = golden("now-folded");
    let want_block = want
        .text
        .iter()
        .position(|line| line.starts_with(" TO HANDLE ("));
    // The prototype's view starts on row 14; the board's block is that many
    // rows higher until T-356 moves the view (then the difference is zero).
    assert_eq!(
        want_block.unwrap_or(0) as i64 - got_block.unwrap_or(0) as i64,
        14 - layout.top as i64,
        "the header's short rows are the only difference"
    );
}

#[test]
fn the_now_view_paints_the_prototype_rows() {
    // The prototype's folded header ends on row 13 and its view starts on row
    // 14; the open header ends on row 21 and its view on row 22 (its
    // `TOP[0]`). T-359 draws the lane table there; the rows below it belong to
    // T-360. The view is the same in both frames.
    let folded = view_differences(Tab::Now, "now-folded", 14, (14, 18));
    assert!(folded.is_empty(), "\n{}", folded.join("\n"));
    let open = view_differences(Tab::Now, "now-open", 22, (22, 26));
    assert!(open.is_empty(), "\n{}", open.join("\n"));
}

#[test]
fn the_tasks_view_paints_the_prototype_rows() {
    // The prototype's folded header ends on row 13 and its view starts on row
    // 14; the open header ends on row 21 and its view on row 22 (its
    // `TOP[0]`). The view is the same in both frames.
    let folded = view_differences(Tab::Tasks, "tasks-folded", 14, (14, 44));
    assert!(folded.is_empty(), "\n{}", folded.join("\n"));
    let open = view_differences(Tab::Tasks, "tasks-open", 22, (22, 44));
    assert!(open.is_empty(), "\n{}", open.join("\n"));
}

/// T-363: the Cost view paints the prototype's rows from the row the
/// prototype's view starts on, so its columns and colours are held to the
/// golden frames even while the board's own header is still four rows.
#[test]
fn the_cost_view_paints_the_prototype_rows() {
    // The prototype's folded header ends on row 13 and its view starts on row
    // 14; the open header ends on row 21 and its view on row 22 (its
    // `TOP[0]`). The view is the same in both frames.
    let folded = view_differences(Tab::Cost, "cost-folded", 14, (14, 44));
    assert!(folded.is_empty(), "\n{}", folded.join("\n"));
    let open = view_differences(Tab::Cost, "cost-open", 22, (22, 44));
    assert!(open.is_empty(), "\n{}", open.join("\n"));
}

/// T-364: the Log view paints the prototype's rows from the row the prototype's
/// view starts on, so its columns and colours are held to the golden frames
/// even while the board's own header is still four rows.
#[test]
fn the_log_view_paints_the_prototype_rows() {
    // The prototype's folded view starts on row 14 and its open view on row 22
    // (its `TOP[0]`); row 13 is the header's own last row and row 47 the
    // footer, so the view's rows are compared through row 46. The view is the
    // same in both frames.
    let folded = view_differences(Tab::Log, "log-folded", 14, (14, 46));
    assert!(folded.is_empty(), "\n{}", folded.join("\n"));
    let open = view_differences(Tab::Log, "log-open", 22, (22, 46));
    assert!(open.is_empty(), "\n{}", open.join("\n"));
}
