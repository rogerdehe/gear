//! The frame of the new board (design §8.1): the head (the voyages, the shown
//! voyage and the goal box with its quality bar), the four tabs, the views and
//! the overlay panel.
//!
//! The prototype of `gear/voyages/M4/board` is the contract for the layout, the
//! views, the keys and the wording; the data comes from the store.
//!
//! The frame is drawn on a **canvas** of the size of the terminal: every row
//! is cut at the right edge, the footer keeps the last row, and the panel is
//! drawn over the view (as the prototype draws its window over the board).

use crate::graph;
use crate::idle::IdleReason;
use crate::style::{lane_color, Color, Line, Style};
use crate::{Board, LaneBoard, ProjectBoard, QuotaReading, QuotaWindow, TaskBoard, VoyageEntry};

/// The four views of the board: the tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// The lanes, what they do now and what they have left.
    Now,
    /// The work as a graph.
    Tasks,
    /// The time and the tokens, per lane.
    Cost,
    /// The decisions of the captain.
    Log,
}

impl Tab {
    /// The number of the tab (the keys 1 to 4).
    pub fn number(self) -> u8 {
        match self {
            Tab::Now => 1,
            Tab::Tasks => 2,
            Tab::Cost => 3,
            Tab::Log => 4,
        }
    }

    /// The title of the tab.
    pub fn title(self) -> &'static str {
        match self {
            Tab::Now => "Now",
            Tab::Tasks => "Tasks",
            Tab::Cost => "Cost",
            Tab::Log => "Log",
        }
    }

    /// The label of the tab on the tab row: `  1 Now  `, two spaces on each
    /// side of the key and the title (T-356).
    pub fn label(self) -> String {
        format!("  {} {}  ", self.number(), self.title())
    }

    /// The tab of a number (the keys 1 to 4).
    pub fn of_number(number: u8) -> Option<Tab> {
        match number {
            1 => Some(Tab::Now),
            2 => Some(Tab::Tasks),
            3 => Some(Tab::Cost),
            4 => Some(Tab::Log),
            _ => None,
        }
    }

    /// The next tab (the right arrow).
    pub fn next(self) -> Tab {
        match self {
            Tab::Now => Tab::Tasks,
            Tab::Tasks => Tab::Cost,
            Tab::Cost => Tab::Log,
            Tab::Log => Tab::Now,
        }
    }

    /// The tab before (the left arrow).
    pub fn previous(self) -> Tab {
        match self {
            Tab::Now => Tab::Log,
            Tab::Tasks => Tab::Now,
            Tab::Cost => Tab::Tasks,
            Tab::Log => Tab::Cost,
        }
    }

    /// Every tab, in the order of the row.
    pub fn all() -> [Tab; 4] {
        [Tab::Now, Tab::Tasks, Tab::Cost, Tab::Log]
    }
}

/// What the board shows: the view, the selected row, the folded header and
/// the size of the terminal.
///
/// `detail` is carried for the unfolded header (the `d` key); `opened` holds
/// the lanes the Cost view shows the tasks of (T-363).
pub struct Screen {
    pub tab: Tab,
    pub selected: usize,
    pub detail: bool,
    pub opened: Vec<String>,
    pub size: (usize, usize),
}

/// The head of the board: where the view starts, the tab row and the columns
/// of the tabs and of the voyages on the first row (T-353).
///
/// Every column is counted from zero, and a range's first and last column are
/// inclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The row of the view's heading (counted from zero): the view draws from
    /// `top`, its first pick on the row after it.
    pub top: usize,
    /// The row of the tabs (counted from zero).
    pub tab_row: usize,
    /// The columns of every tab on the tab row.
    pub tabs: Vec<(usize, usize, Tab)>,
    /// The columns of every voyage's label on the first row, and the voyage's
    /// milestone (T-353).
    pub voyages: Vec<(usize, usize, String)>,
}

/// The footer of the board: the keys.
///
/// Short enough for an 80 column terminal: a row that is cut at the edge
/// hides the end of it, and the end says how to leave.
pub const FOOTER: &str = "←→ / Tab / 1-4 / click: views · click a voyage · [ ] older / newer voyage · d header details · ↑↓ select · Enter: a lane's work / a task's rounds / a lane's tasks · q quit";

/// A screen of characters: the frame, the view and the panel are drawn on it.
///
/// Every cell has a style (the prototype's colours, bold, reverse video);
/// [`Canvas::text`] is the screen as plain text and [`Canvas::ansi`] the
/// screen as the terminal shows it.
///
/// A wide glyph (the prototype's ⛵, ⚓ and ⏳) takes two cells: the glyph and a
/// continuation, a space the terminal does not draw again. The continuation is
/// marked, so the cells keep the prototype's columns and [`Canvas::row_text`]
/// leaves the mark's cell out of its text: the terminal's columns are the
/// canvas's (T-353, review F1).
pub struct Canvas {
    width: usize,
    height: usize,
    cells: Vec<char>,
    styles: Vec<Style>,
    continuation: Vec<bool>,
}

impl Canvas {
    /// A canvas of the size of the terminal.
    pub fn new(width: usize, height: usize) -> Canvas {
        let width = width.max(20);
        let height = height.max(8);
        Canvas {
            width,
            height,
            cells: vec![' '; width * height],
            styles: vec![Style::PLAIN; width * height],
            continuation: vec![false; width * height],
        }
    }

    /// Write a text at a column and a row; the text is cut at the edge.
    pub fn put(&mut self, x: usize, y: usize, text: &str) {
        self.put_styled(x, y, text, Style::PLAIN);
    }

    /// Write a text in a style; what was under it loses its own style.
    ///
    /// As the prototype's `put` does, the last column is never written: the
    /// text is cut at `width - x - 1` characters.
    pub fn put_styled(&mut self, x: usize, y: usize, text: &str, style: Style) {
        if y >= self.height {
            return;
        }
        let last = self.width - 1;
        for (offset, character) in text.chars().enumerate() {
            let column = x + offset;
            if column >= last {
                break;
            }
            let cell = y * self.width + column;
            self.clear_wide_at(cell);
            self.cells[cell] = character;
            self.styles[cell] = style;
            self.continuation[cell] = false;
        }
    }

    /// Write a line, every character in its own style; cut at the last
    /// column, as [`Canvas::put_styled`] is.
    pub fn put_line(&mut self, x: usize, y: usize, line: &Line) {
        if y >= self.height {
            return;
        }
        let last = self.width - 1;
        for (offset, (character, style)) in line.cells().enumerate() {
            let column = x + offset;
            if column >= last {
                break;
            }
            let cell = y * self.width + column;
            self.clear_wide_at(cell);
            self.cells[cell] = character;
            self.styles[cell] = style;
            self.continuation[cell] = false;
        }
    }

    /// Write a text that may hold a wide glyph (the prototype's ⛵, ⚓ and ⏳).
    ///
    /// The glyph takes two cells, the second its continuation: a space with
    /// the same style, marked so that the text does not print it again. It
    /// returns the number of cells the text took.
    pub fn put_wide(&mut self, x: usize, y: usize, text: &str, style: Style) -> usize {
        if y >= self.height {
            return 0;
        }
        let last = self.width - 1;
        let mut column = x;
        for character in text.chars() {
            if column >= last {
                break;
            }
            let cell = y * self.width + column;
            self.clear_wide_at(cell);
            self.cells[cell] = character;
            self.styles[cell] = style;
            self.continuation[cell] = false;
            column += 1;
            if is_wide(character) && column < last {
                let cell = y * self.width + column;
                self.clear_wide_at(cell);
                self.cells[cell] = ' ';
                self.styles[cell] = style;
                self.continuation[cell] = true;
                column += 1;
            }
        }
        column - x
    }

    /// A write lands on one cell, but a wide glyph is drawn in two: overwriting
    /// half of one would leave the other half to the terminal, which draws the
    /// glyph over the write and shifts every column after it (T-353, review
    /// F3). So the other half is blanked first, as the prototype's export
    /// blanks it.
    ///
    /// A write onto a glyph's continuation blanks the glyph's leading cell; a
    /// write onto a leading cell clears its continuation, which stays a space
    /// in the glyph's style. Either way the glyph is never half overwritten.
    fn clear_wide_at(&mut self, cell: usize) {
        let same_row = |cell: usize| cell / self.width;
        if self.continuation[cell] {
            // This cell is the second half: blank the leading one.
            if cell > 0 && same_row(cell - 1) == same_row(cell) {
                self.cells[cell - 1] = ' ';
                self.continuation[cell - 1] = false;
            }
            self.continuation[cell] = false;
        } else if is_wide(self.cells[cell]) {
            // This cell is the first half: its continuation is no longer one.
            let next = cell + 1;
            if next < self.cells.len() && same_row(next) == same_row(cell) {
                self.continuation[next] = false;
            }
        }
    }

    /// Is the cell the continuation of a wide glyph? The terminal draws that
    /// column as part of the glyph, so the text leaves its cell out.
    pub fn is_continuation(&self, x: usize, y: usize) -> bool {
        x < self.width && y < self.height && self.continuation[y * self.width + x]
    }

    /// Copy `count` rows of another canvas: its row `src` to this canvas's
    /// row `dst`, and so on. Both canvases are cut at their own edge.
    pub fn copy_rows(&mut self, from: &Canvas, src: usize, dst: usize, count: usize) {
        let columns = self.width.min(from.width);
        for index in 0..count {
            let (source, target) = (src + index, dst + index);
            if source >= from.height || target >= self.height {
                break;
            }
            for column in 0..columns {
                self.cells[target * self.width + column] = from.cells[source * from.width + column];
                self.styles[target * self.width + column] =
                    from.styles[source * from.width + column];
                self.continuation[target * self.width + column] =
                    from.continuation[source * from.width + column];
            }
        }
    }

    /// Show a part of the screen in reverse video (the tab that is open).
    pub fn reverse(&mut self, y: usize, x: usize, len: usize) {
        if y < self.height && x < self.width {
            for column in x..(x + len).min(self.width) {
                self.styles[y * self.width + column].reverse = true;
            }
        }
    }

    /// The style of a cell (a test reads it).
    pub fn style_at(&self, x: usize, y: usize) -> Style {
        if x < self.width && y < self.height {
            self.styles[y * self.width + x]
        } else {
            Style::PLAIN
        }
    }

    /// The character of a cell; a space outside the canvas.
    pub fn char_at(&self, x: usize, y: usize) -> char {
        if x < self.width && y < self.height {
            self.cells[y * self.width + x]
        } else {
            ' '
        }
    }

    /// The width of the canvas, in cells.
    pub fn width(&self) -> usize {
        self.width
    }

    /// The height of the canvas, in cells.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Light a whole row in reverse video (the prototype's `hl`): columns 1 to
    /// `width - 2`, both included; the characters stay.
    pub fn light_row(&mut self, y: usize) {
        if y >= self.height {
            return;
        }
        for column in 1..self.width.saturating_sub(1) {
            self.styles[y * self.width + column] = Style {
                fg: None,
                bold: false,
                reverse: true,
            };
        }
    }

    /// The screen as plain text: the rows, each cut at the edge, without a
    /// terminal code.
    pub fn text(&self) -> String {
        (0..self.height)
            .map(|row| self.row_text(row, None))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The screen as the terminal shows it: the styles as SGR codes. Without
    /// `colour` (NO_COLOR) the colours are left out; bold and the reverse
    /// video of the open tab stay.
    pub fn ansi(&self, colour: bool) -> String {
        (0..self.height)
            .map(|row| self.row_text(row, Some(colour)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One row of the screen; `codes` is `None` for the plain text, else
    /// whether the colours are on.
    fn row_text(&self, row: usize, codes: Option<bool>) -> String {
        let cells = &self.cells[row * self.width..(row + 1) * self.width];
        let styles = &self.styles[row * self.width..(row + 1) * self.width];
        let effective = |column: usize| -> Style {
            match codes {
                None => Style::PLAIN,
                Some(true) => styles[column],
                Some(false) => Style {
                    fg: None,
                    ..styles[column]
                },
            }
        };
        // The plain spaces at the end of the row are not drawn.
        let mut end = self.width;
        while end > 0 && cells[end - 1] == ' ' && !effective(end - 1).shows_on_space() {
            end -= 1;
        }
        let mut out = String::new();
        let mut column = 0usize;
        while column < end {
            let start = column;
            let style = effective(column);
            while column < end && effective(column) == style {
                column += 1;
            }
            // A continuation cell is the second column of a wide glyph, which
            // the terminal already draws: its space is left out of the text, so
            // the terminal's columns are the canvas's (T-353, review F1).
            let text: String = (start..column)
                .filter(|column| !self.continuation[row * self.width + column])
                .map(|column| cells[column])
                .collect();
            if text.is_empty() {
                continue;
            }
            let sgr = style.sgr(codes.unwrap_or(false));
            if sgr.is_empty() {
                out.push_str(&text);
            } else {
                out.push_str(&sgr);
                out.push_str(&text);
                out.push_str("\u{1b}[0m");
            }
        }
        out
    }
}

/// The whole screen as plain text: the frame, the view and the panel.
pub fn frame(board: &Board, screen: &Screen, panel: Option<&[String]>) -> String {
    let panel: Option<Vec<Line>> =
        panel.map(|rows| rows.iter().map(|row| Line::plain(row.clone())).collect());
    canvas(board, screen, panel.as_deref()).text()
}

/// The whole screen on a canvas, in the prototype's colours.
pub fn canvas(board: &Board, screen: &Screen, panel: Option<&[Line]>) -> Canvas {
    let (width, height) = screen.size;
    let mut canvas = Canvas::new(width, height);
    let layout = header(&mut canvas, board, screen);

    // The view paints on a tall canvas, the screen's width: the prototype's
    // views put their texts at fixed columns, a later one over an earlier one
    // (README change 13). The screen copies the part the user looks at.
    let top = layout.top;
    let mut tall = Canvas::new(width, 1000);
    let picks = view(board, screen, &mut tall, top);
    // The selected row stays on the screen: the rows above it scroll away
    // (F8). The offset is the one the mouse hit testing uses.
    let last = view_bottom(screen.tab, height);
    let offset = offset_of(&picks, screen.selected, top, last);
    // The footer's row, the legend's row and the row above them stay the
    // frame's.
    canvas.copy_rows(&tall, top + offset, top, last.saturating_sub(top));
    // The selected row is lit across the width, in one colour (README change
    // 13).
    if let Some(row) = picks
        .get(screen.selected)
        .map(|row| row.saturating_sub(offset))
    {
        if row >= top && row < last {
            canvas.light_row(row);
        }
    }
    if screen.tab == Tab::Tasks {
        // The Tasks view's legend (the prototype's row `h - 2`).
        canvas.put_styled(
            1,
            height.saturating_sub(2),
            TASKS_LEGEND,
            Style::fg(Color::Dim),
        );
    }
    canvas.put_styled(1, height - 1, FOOTER, Style::fg(Color::Dim));
    if let Some(panel) = panel {
        draw_panel(&mut canvas, panel, top);
    }
    canvas
}

/// The row the view's own rows stop at (exclusive): the footer's row is
/// `height - 1`, and the Tasks view keeps the legend's row `height - 2` (the
/// prototype stops its graph at `h - 3`).
pub fn view_bottom(tab: Tab, height: usize) -> usize {
    match tab {
        Tab::Tasks => height.saturating_sub(3),
        _ => height.saturating_sub(2),
    }
}

/// The legend under the Tasks view (the prototype's row `h - 2`).
pub const TASKS_LEGEND: &str = "● merged into dev   ◉ working   ◎ in review   ◍ passed / merging   ↺ reworking   ○┆ not started   a branch leaves dev where its work starts and joins it where it merges · ↑↓ select: its deps light up";

/// The offset of the first visible row of a view (the viewport, F8).
///
/// The rows above it are scrolled away, so the mouse hit testing adds the
/// same offset to the row the user clicked. The offset is zero while the
/// selected pick's row is above `last` (the first row below the view), else the
/// smallest offset that shows it.
pub fn offset_of(picks: &[usize], selected: usize, top: usize, last: usize) -> usize {
    let Some(row) = picks.get(selected).copied() else {
        return 0;
    };
    if row < last {
        0
    } else {
        (row - last + 1).min(row.saturating_sub(top))
    }
}

/// The head of the board (README changes 1 and 2): the first row lists every
/// voyage of the project with the shown one lit, the second row names the
/// shown voyage, and the third starts the goal box with its quality bar.
///
/// The tabs follow the box directly, framed by a rule above them and one under
/// them (T-356), and the view's heading comes three rows after the tab row
/// (the bottom rule, the spacer under the bar). Every text is put at the
/// prototype's column and cut at the edge, as [`Canvas::put_styled`] cuts it.
///
/// It returns the layout it drew ([`Layout`]), which is what the clicks and
/// the view need.
fn header(canvas: &mut Canvas, board: &Board, screen: &Screen) -> Layout {
    let project = board.projects.first();
    // The voyage the board shows (T-350): the whole head names it and boxes
    // its goal, so a landed voyage keeps the data of its landing.
    let voyage = project.and_then(|project| project.shown.as_ref());

    // Row 0: the title, every voyage's label and the clock.
    let title = match project {
        Some(project) => format!("GEAR · {}  ", project.project),
        None => "GEAR".to_string(),
    };
    canvas.put_styled(1, 0, &title, Style::bold());
    let mut voyages = Vec::new();
    let mut x = match project {
        Some(project) => project.project.chars().count() + 8,
        None => 1,
    };
    if let Some(project) = project {
        // One label per voyage, newest first: ` M5 ⛵ ` for a voyage that
        // sails, ` M4.1 ⚓ ` for a landed one. The shown voyage is in reverse
        // video and bold, the others dim.
        for entry in &project.voyage_list {
            let mark = if entry.sailing { '⛵' } else { '⚓' };
            let label = format!(" {} {} ", entry.milestone, mark);
            // The prototype draws a wide mark in two cells; the label's own
            // length is its characters, as the prototype counts them.
            let cells = canvas.put_wide(x, 0, &label, voyage_style(voyage, entry));
            voyages.push((x, x + cells - 1, entry.milestone.clone()));
            x += label.chars().count() + 1;
        }
    }
    canvas.put_styled(x + 1, 0, "[ older   ] newer", Style::fg(Color::Dim));
    if board.engine == crate::EngineState::Down {
        // The engine is down: the next `gear` call starts it (README). It says
        // so two columns after "newer", so that the tab row stays where it is.
        let after = x + 1 + "[ older   ] newer".chars().count() + 1;
        canvas.put_styled(
            after,
            0,
            "! engine down — the next gear call starts it",
            Style::fg(Color::Bad),
        );
    }
    // The clock sits where the prototype puts it, `wd - 8` (T-340 contract v2:
    // the golden frames are the standard).
    let clock = crate::idle::clock(board.now);
    canvas.put_styled(
        canvas.width.saturating_sub(8),
        0,
        &clock,
        Style::fg(Color::Dim),
    );

    // Row 1: the shown voyage's milestone and title, and what it does.
    if let Some(voyage) = voyage {
        let state = if voyage.sailing {
            "⛵ sailing"
        } else {
            "⚓ landed"
        };
        let line = format!("{}: {} · {}", voyage.milestone, voyage.title, state);
        canvas.put_wide(1, 1, &line, Style::bold());
    }

    // The goal box: the voyage's done sentence, word-wrapped, and the quality
    // bar under it, in the prototype's yellow box (README change 2).
    let mut row = 2;
    let box_width = canvas.width.saturating_sub(3);
    if let Some(voyage) = voyage {
        let inside = box_width.saturating_sub(4);
        canvas.put_styled(
            1,
            row,
            &format!("╔═ GOAL {}╗", "═".repeat(box_width.saturating_sub(9))),
            Style::fg(Color::Warn).and_bold(),
        );
        row += 1;
        for line in wrap(voyage.done.as_deref().unwrap_or_default(), inside) {
            canvas.put_styled(
                1,
                row,
                &format!("║ {} ║", pad_to(&line, inside)),
                Style::fg(Color::Warn).and_bold(),
            );
            row += 1;
        }
        // The bar is how the goal is judged: it is locked in the box with it,
        // yellow and not bold.
        canvas.put_styled(
            1,
            row,
            &format!("║ {} ║", pad_to(&bar_line(voyage.bar.as_deref()), inside)),
            Style::fg(Color::Warn),
        );
        row += 1;
        canvas.put_styled(
            1,
            row,
            &format!("╚{}╝", "═".repeat(box_width.saturating_sub(2))),
            Style::fg(Color::Warn).and_bold(),
        );
        row += 1;
    }

    // The latest goal change, when the user agreed to one: a red line under
    // the box with when, before and after (T-358, README change 2).
    if let Some((time, before, after)) = project.and_then(|project| project.goal_changed.as_ref()) {
        let line = format!(
            "{:<14}{}: {} \u{2192} {} (the user agreed; Decisions shows both)",
            "Goal changed",
            clock_of(time),
            before,
            after
        );
        canvas.put_styled(1, row, &line, Style::fg(Color::Bad).and_bold());
        row += 1;
    }

    // The header's rows under the box (README changes 3, 4, 6 and 14): folded,
    // three rows with the goal check, the milestone the voyage is on, the time
    // against the plan and the tasks (T-354); unfolded with `d`, one row per
    // clause and per station, the time and the tasks with their legends
    // (T-355).
    if let Some(project) = project {
        row = if screen.detail {
            unfolded_rows(canvas, board, project, row)
        } else {
            folded_rows(canvas, board, project, row)
        };
    }

    // The tab row follows after one blank row, and the blank row is where the
    // tab bar's top rule goes: the bar is framed (T-356), its bottom rule
    // under the tabs and one blank row under that, so the view's heading is
    // three rows below the tabs. A tab is its key and its title between two
    // spaces, a rule between it and the next, and the clicked columns are the
    // tab's own and the rule after it, as the prototype's are.
    let tab_row = row + 1;
    let width = canvas.width;
    let accent = Style::fg(Color::Acc);
    canvas.put_styled(
        1,
        tab_row - 1,
        &format!("╭{}╮", "─".repeat(width.saturating_sub(4))),
        accent,
    );
    let mut tabs = Vec::new();
    let mut x = 1;
    for tab in Tab::all() {
        let label = tab.label();
        let count = label.chars().count();
        canvas.put_styled(x, tab_row, "│", accent);
        // The shown tab is lit: the accent colour, bold, in reverse video; a
        // tab that is not shown is bold alone, as the prototype draws it.
        let style = if tab == screen.tab {
            Style::fg(Color::Acc).and_bold().reversed_if(true)
        } else {
            Style::bold()
        };
        canvas.put_styled(x + 1, tab_row, &label, style);
        tabs.push((x, x + count, tab));
        x += count + 1;
    }
    canvas.put_styled(x, tab_row, "│", accent);
    canvas.put_styled(width.saturating_sub(2), tab_row, "│", accent);
    canvas.put_styled(
        1,
        tab_row + 1,
        &format!("╰{}╯", "─".repeat(width.saturating_sub(4))),
        accent,
    );

    Layout {
        top: tab_row + 3,
        tab_row,
        tabs,
        voyages,
    }
}

/// The numbers the header's time and tasks rows read (T-354), all in
/// minutes.
#[derive(Clone, Copy)]
struct HeaderNumbers {
    /// The time the voyage used: from its departure to now while it sails, to
    /// its merge once it landed.
    used: i64,
    /// The plan's eta.
    eta: i64,
    /// The end the lane forecast expects: the slowest lane's end after what is
    /// used.
    expected: i64,
    /// The voyage still sails.
    sailing: bool,
    /// The plan's eta less the expected end (sailing) or the used time
    /// (landed): never negative when the voyage is ahead of its plan.
    diff: i64,
    /// The voyage's tasks: the cancelled ones are not work.
    tasks: usize,
    /// The tasks merged, in progress and not started.
    merged: usize,
    going: usize,
    waiting: usize,
}

/// The header's numbers, all in minutes: the time used, the plan's eta and the
/// end the lane forecast expects (T-354).
fn header_numbers(board: &Board, project: &ProjectBoard) -> HeaderNumbers {
    let voyage = project.shown.as_ref();
    let used = voyage_used_minutes(voyage, board.now);
    let eta = project
        .workload
        .as_ref()
        .and_then(|voyage| voyage.eta.as_deref())
        .map(duration_minutes)
        .unwrap_or(0);
    let expected = project
        .forecast
        .iter()
        .map(|end| end.end_min)
        .max()
        .map(|end| used + end)
        .unwrap_or(used);
    let sailing = voyage.is_some_and(|voyage| voyage.sailing);
    let diff = if sailing { eta - expected } else { eta - used };

    // The tasks of the shown voyage: the cancelled ones are not work, the
    // merged ones, the waiting ones and the rest make the three parts.
    let mut tasks = 0usize;
    let mut merged = 0usize;
    let mut waiting = 0usize;
    for task in project.lanes.iter().flat_map(|lane| lane.tasks.iter()) {
        if task.state == gear_model::TaskState::Cancelled {
            continue;
        }
        tasks += 1;
        match task.stage {
            graph::Stage::Merged => merged += 1,
            graph::Stage::Deps | graph::Stage::Queued => waiting += 1,
            _ => {}
        }
    }
    let going = tasks.saturating_sub(merged + waiting);

    HeaderNumbers {
        used,
        eta,
        expected,
        sailing,
        diff,
        tasks,
        merged,
        going,
        waiting,
    }
}

/// The time bar of `n` cells (the prototype's `bar`, T-354): the cells used,
/// then the plan's end and, while the voyage sails, the expected finish.
fn time_bar(n: usize, numbers: &HeaderNumbers) -> String {
    let HeaderNumbers {
        used,
        eta,
        expected,
        sailing,
        ..
    } = *numbers;
    let span = eta.max(expected).max(used).max(1) as usize;
    let filled = (((used.max(0) as f64) / span as f64) * n as f64)
        .round_ties_even()
        .max(0.0) as usize;
    let mut cells: Vec<char> = std::iter::repeat('░').take(n).collect();
    for cell in cells.iter_mut().take(filled.min(n)) {
        *cell = '█';
    }
    cells[((eta.max(0) as usize * n) / span).min(n - 1)] = '│';
    if sailing {
        cells[((expected.max(0) as usize * n) / span).min(n - 1)] = '◆';
    }
    cells.into_iter().collect()
}

/// The folded header's three rows, from the row after the goal box: the goal
/// check and the milestone, the time against the plan and the tasks, and the
/// key that unfolds them (T-354; README changes 3, 4, 6 and 14).
///
/// Every text is put at the prototype's column, a later put over an earlier
/// one, as the prototype paints: the time's words run into the tasks' column
/// on a narrow terminal. It returns the row after the three rows.
fn folded_rows(
    canvas: &mut Canvas,
    board: &Board,
    project: &ProjectBoard,
    mut row: usize,
) -> usize {
    let width = canvas.width;
    let numbers = header_numbers(board, project);
    let HeaderNumbers {
        used,
        eta,
        expected,
        sailing,
        diff,
        tasks,
        merged,
        going,
        waiting,
    } = numbers;

    // The folded header's bar: the prototype's shorter one.
    let n = (width.saturating_sub(60) / 2).clamp(20, 40);
    let bar = time_bar(n, &numbers);
    let a = merged * n / tasks.max(1);
    let b = going * n / tasks.max(1);

    let mx = (width / 2 + 10).max(90);
    let time_style = if diff >= 0 {
        Style::fg(Color::Ok)
    } else {
        Style::fg(Color::Bad)
    };

    // Row 1: the goal check and the milestone the voyage is on.
    canvas.put_styled(1, row, "Goal check", Style::bold());
    let met = project.goal_check.iter().filter(|(met, _)| *met).count();
    let all = project.goal_check.len();
    canvas.put_styled(
        15,
        row,
        &format!("{met} of {all} met"),
        Style::fg(if met == all { Color::Ok } else { Color::Bad }),
    );
    if let Some((_, first)) = project.goal_check.iter().find(|(met, _)| !*met) {
        canvas.put_styled(30, row, &format!("next: {first}"), Style::fg(Color::Bad));
    }
    canvas.put_styled(mx, row, "Milestone", Style::bold());
    if project.stations.is_empty() {
        canvas.put_styled(mx + 11, row, "none set", Style::fg(Color::Dim));
    } else {
        let reached = project
            .stations
            .iter()
            .filter(|station| station.state == crate::stations::StationState::Reached)
            .count();
        let now = match project
            .stations
            .iter()
            .find(|station| station.state == crate::stations::StationState::Now)
        {
            Some(station) => format!(
                "▶ {} {} ({reached} of {} reached)",
                station.id,
                station.needs,
                project.stations.len()
            ),
            None => format!("✔ all {} reached", project.stations.len()),
        };
        canvas.put_styled(mx + 11, row, &now, Style::bold());
    }
    row += 1;

    // Row 2: the time against the plan and the tasks, in their three parts.
    canvas.put_styled(1, row, "Time", Style::bold());
    canvas.put_styled(15, row, &bar, time_style);
    let short = if sailing {
        if diff >= 0 {
            format!(
                "expected {} of {}, {} ahead",
                hm(expected),
                hm(eta),
                hm(diff)
            )
        } else {
            format!(
                "expected {} of {}, {} behind",
                hm(expected),
                hm(eta),
                hm(-diff)
            )
        }
    } else if diff >= 0 {
        format!("took {} of {}, {} under", hm(used), hm(eta), hm(diff))
    } else {
        format!("took {} of {}, {} over", hm(used), hm(eta), hm(-diff))
    };
    canvas.put_styled(16 + n, row, &short, time_style);
    canvas.put_styled(mx, row, "Tasks", Style::bold());
    canvas.put_styled(mx + 11, row, &"█".repeat(a), Style::fg(Color::Ok));
    canvas.put_styled(mx + 11 + a, row, &"▓".repeat(b), Style::fg(Color::Warn));
    canvas.put_styled(
        mx + 11 + a + b,
        row,
        &"░".repeat(n - a - b),
        Style::fg(Color::Dim),
    );
    canvas.put_styled(
        mx + 12 + n,
        row,
        &format!("{merged}/{tasks} merged · {going} going · {waiting} waiting"),
        Style::PLAIN,
    );
    row += 1;

    // Row 3: the key that unfolds the header.
    canvas.put_styled(
        1,
        row,
        "d: unfold the goal check, milestones, time and tasks",
        Style::fg(Color::Dim),
    );
    row + 1
}

/// The unfolded header's rows, from the row after the goal box: every clause of
/// the goal check, every station of the voyage, the time against the plan and
/// the tasks with their legends, and the key that folds them again (T-355;
/// README changes 3, 4, 6 and 14).
///
/// The bar is the prototype's wider one; the labels sit at column 1 and every
/// row's text at 15, as the prototype paints them. It returns the row after the
/// last one.
fn unfolded_rows(
    canvas: &mut Canvas,
    board: &Board,
    project: &ProjectBoard,
    mut row: usize,
) -> usize {
    let width = canvas.width;
    let numbers = header_numbers(board, project);
    let HeaderNumbers {
        used,
        eta,
        expected,
        sailing,
        diff,
        tasks,
        merged,
        going,
        waiting,
    } = numbers;

    // The unfolded header's bar: the prototype's wider one.
    let n = width.saturating_sub(95).clamp(20, 50);
    let bar = time_bar(n, &numbers);
    let a = merged * n / tasks.max(1);
    let b = going * n / tasks.max(1);
    let time_style = if diff >= 0 {
        Style::fg(Color::Ok)
    } else {
        Style::fg(Color::Bad)
    };

    // The goal check: every clause of the voyage's goal, met or not yet.
    canvas.put_styled(1, row, "Goal check", Style::bold());
    for (met, clause) in &project.goal_check {
        let (mark, style) = if *met {
            ('✔', Style::fg(Color::Ok))
        } else {
            ('✗', Style::fg(Color::Bad))
        };
        canvas.put_styled(15, row, &format!("{mark} {clause}"), style);
        row += 1;
    }

    // The milestones: every station of the voyage, how far it is and when the
    // plan has it (T-355 v3). A reached station has the merge that reached it;
    // a station ahead sits between the time used and the projected finish, by
    // the minutes it has left.
    canvas.put_styled(1, row, "Milestones", Style::bold());
    let projected = project
        .workload
        .as_ref()
        .and_then(|voyage| voyage.projected_min);
    let left_all: i64 = project
        .stations
        .iter()
        .map(|station| station.left_min)
        .sum();
    let mut left_upto = 0i64;
    for station in &project.stations {
        left_upto += station.left_min;
        let what = format!("{} {}", station.id, station.needs);
        let planned = projected
            .filter(|_| left_all > 0)
            .map(|projected| used + ((projected - used) * left_upto).div_euclid(left_all));
        let (text, style) = match (station.state, station.reached_at_min, planned) {
            (crate::stations::StationState::Reached, Some(at), _) => (
                format!("✔ {what} — reached at {}", hm(at)),
                Style::fg(Color::Ok),
            ),
            (crate::stations::StationState::Reached, None, _) => {
                (format!("✔ {what} — reached"), Style::fg(Color::Ok))
            }
            (crate::stations::StationState::Now, _, Some(at)) => (
                format!("▶ {what} — now, planned by {}", hm(at)),
                Style::bold(),
            ),
            (crate::stations::StationState::Now, _, None) => {
                (format!("▶ {what} — now"), Style::bold())
            }
            (_, _, Some(at)) => (
                format!("· {what} — planned by {}", hm(at)),
                Style::fg(Color::Dim),
            ),
            (_, _, None) => (format!("· {what}"), Style::fg(Color::Dim)),
        };
        canvas.put_styled(15, row, &text, style);
        row += 1;
    }

    // The time against the plan in words, and the key of the bar's marks.
    canvas.put_styled(1, row, "Time", Style::bold());
    canvas.put_styled(15, row, &bar, time_style);
    let slow = project
        .forecast
        .iter()
        .max_by_key(|end| end.end_min)
        .map(|end| end.lane.clone())
        .unwrap_or_default();
    let when = if sailing {
        if diff >= 0 {
            format!(
                "used {} · planned {} · expected {} (slowest lane: {slow}), {} ahead of plan",
                hm(used),
                hm(eta),
                hm(expected),
                hm(diff)
            )
        } else {
            format!(
                "used {} · planned {} · expected {} (slowest lane: {slow}), {} behind plan",
                hm(used),
                hm(eta),
                hm(expected),
                hm(-diff)
            )
        }
    } else if diff >= 0 {
        format!(
            "took {} · planned {} · {} under plan",
            hm(used),
            hm(eta),
            hm(diff)
        )
    } else {
        format!(
            "took {} · planned {} · {} over plan",
            hm(used),
            hm(eta),
            hm(-diff)
        )
    };
    canvas.put_styled(17 + n, row, &when, Style::bold());
    row += 1;
    canvas.put_styled(
        15,
        row,
        "█ used   ◆ expected finish   │ planned end",
        Style::fg(Color::Dim),
    );
    row += 1;

    // The tasks of the voyage in three parts, the ones that moved out named
    // apart, and the key of the bar's marks.
    canvas.put_styled(1, row, "Tasks", Style::bold());
    canvas.put_styled(15, row, &"█".repeat(a), Style::fg(Color::Ok));
    canvas.put_styled(15 + a, row, &"▓".repeat(b), Style::fg(Color::Warn));
    canvas.put_styled(
        15 + a + b,
        row,
        &"░".repeat(n - a - b),
        Style::fg(Color::Dim),
    );
    let mut words =
        format!("{merged} of {tasks} merged · {going} in progress · {waiting} not started");
    if !project.moved_out.is_empty() {
        let moved: Vec<&str> = project
            .moved_out
            .iter()
            .map(|(task, _)| task.as_str())
            .collect();
        words.push_str(&format!(" · moved out: {}", moved.join(", ")));
    }
    canvas.put_styled(17 + n, row, &words, Style::bold());
    row += 1;
    canvas.put_styled(
        15,
        row,
        "█ merged   ▓ in progress   ░ not started",
        Style::fg(Color::Dim),
    );
    row += 1;

    // The key that folds the header again, at the prototype's column.
    canvas.put_styled(
        width.saturating_sub(20),
        row,
        "d: fold",
        Style::fg(Color::Dim),
    );
    row + 1
}

/// Minutes as the header reads them: always the hours, the minutes in two
/// digits (`0h25m`; T-354). The board's other texts keep [`hours`].
fn hm(minutes: i64) -> String {
    format!("{}h{:02}m", minutes / 60, minutes % 60)
}

/// The minutes of a voyage's eta, as the plan writes it (`8h`, `1d 4h`,
/// `90m`): the units of `eta_seconds` in `gear-app`. An eta the board cannot
/// read is no time at all.
fn duration_minutes(text: &str) -> i64 {
    let mut rest = text.trim();
    let mut seconds = 0.0;
    while !rest.is_empty() {
        let end = rest
            .find(|character: char| !character.is_ascii_digit() && character != '.')
            .unwrap_or(rest.len());
        let Ok(number) = rest[..end].parse::<f64>() else {
            return 0;
        };
        rest = rest[end..].trim_start();
        let end = rest
            .find(|character: char| !character.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        let factor = match rest[..end].to_ascii_lowercase().as_str() {
            "d" | "day" | "days" => 86_400.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            _ => return 0,
        };
        seconds += number * factor;
        rest = rest[end..].trim_start();
    }
    if seconds.is_finite() && seconds > 0.0 {
        (seconds / 60.0).ceil() as i64
    } else {
        0
    }
}

/// The layout of the head of the board, drawn on a scratch canvas: the rows
/// and the columns the view and the clicks go by.
pub fn layout(board: &Board, screen: &Screen) -> Layout {
    let mut scratch = Canvas::new(screen.size.0, screen.size.1);
    header(&mut scratch, board, screen)
}

/// The style of a voyage's label on the first row: the shown voyage is lit,
/// the others are dim.
fn voyage_style(shown: Option<&VoyageEntry>, entry: &VoyageEntry) -> Style {
    let is_shown = shown
        .map(|shown| shown.milestone == entry.milestone)
        .unwrap_or(false);
    if is_shown {
        Style {
            fg: None,
            bold: true,
            reverse: true,
        }
    } else {
        Style::fg(Color::Dim)
    }
}

/// Is the character a wide one (East Asian wide or fullwidth), the way the
/// prototype's export counts a cell?
fn is_wide(character: char) -> bool {
    matches!(character, '⛵' | '⚓' | '⏳')
}

/// The prototype's `wrap`: the words split on whitespace; a word that would
/// pass `width` starts a new line.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + word.chars().count() + 1 > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The text padded with spaces to `width` characters (a longer text is not
/// cut: the canvas cuts the line at its edge).
fn pad_to(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - count))
    }
}

/// The quality bar's line of the goal box: its key and what it blocks.
///
/// The key is the leading run of capital letters (A-Z) of the voyage's bar;
/// a bar with another key is shown as it was written.
fn bar_line(bar: Option<&str>) -> String {
    let Some(bar) = bar else {
        return "Quality bar: not set".to_string();
    };
    let key: String = bar.chars().take_while(|c| c.is_ascii_uppercase()).collect();
    let meaning = match key.as_str() {
        "FASTEST" => "only what breaks the chain or loses data blocks a task",
        "DELIVERABLE" => "errors in normal use block a task; edge cases are logged",
        "STRICT" => "severe issues block a task; medium ones are the captain's call",
        _ => return format!("Quality bar: {bar}"),
    };
    format!("Quality bar: {key} — {meaning}")
}

/// The lanes of the project, in seat order: the order of the lane colours.
fn lane_order(project: &ProjectBoard) -> Vec<&str> {
    project
        .lanes
        .iter()
        .map(|lane| lane.lane.as_str())
        .collect()
}

/// The colour of a lane on this board.
fn lane_style(project: &ProjectBoard, lane: &str) -> Style {
    Style::fg(lane_color(&lane_order(project), lane))
}

/// Paint the view of the board on a canvas, at row `top`: the heading on its
/// own row, the rows of the view from `top + 1`.
///
/// The canvas row of every selectable row is returned, in order: the picks.
/// It is what the selection moves over, and what the selected row is lit on.
pub fn view(board: &Board, screen: &Screen, canvas: &mut Canvas, top: usize) -> Vec<usize> {
    let Some(project) = board.projects.first() else {
        canvas.put_line(0, top, &Line::plain("no project in the store"));
        return Vec::new();
    };
    match screen.tab {
        Tab::Now => view_now(project, board.now, screen, canvas, top),
        Tab::Tasks => view_tasks(project, board.now, screen, canvas, top),
        Tab::Cost => view_cost(project, screen, canvas, top),
        Tab::Log => view_decisions(project, screen, canvas, top),
    }
}

/// The departure of the shown voyage (T-350's entry): the voyage's own
/// timestamp, so a landed or older voyage picked with `[` and `]` reads its
/// tasks' starts from its own departure, not from the active voyage's elapsed
/// minutes (F1 of the T-362 review). A voyage with no departure has no start to
/// measure from: the view measures from `now`.
fn voyage_departure(project: &ProjectBoard, now: gear_model::Ts) -> gear_model::Ts {
    project
        .shown
        .as_ref()
        .and_then(|voyage| voyage.departure)
        .unwrap_or(now)
}

/// The lanes of the Tasks view, in the prototype's order: the lane of the
/// first task in plan order, then the next new one, and a lane of
/// `project.lanes` that no shown task names after them.
///
/// The prototype draws its voyage's lanes in the plan's order (`LANES`), while
/// `project.lanes` is in seat order (a seat id sorts by name), so the two orders
/// differ once a voyage has three lanes (hull, mast, keel against hull, keel,
/// mast). The golden frames are the prototype's, so the view takes the plan's
/// order (T-362, the captain's answer to M-5869 pending).
fn task_lanes(project: &ProjectBoard) -> Vec<String> {
    let mut plan: Vec<&graph::GraphTask> = project.graph.iter().collect();
    plan.sort_by_key(|task| task.rank);
    let mut lanes: Vec<String> = Vec::new();
    for task in plan {
        if !lanes.contains(&task.lane) {
            lanes.push(task.lane.clone());
        }
    }
    for lane in lane_order(project) {
        if !lanes.iter().any(|known| known == lane) {
            lanes.push(lane.to_string());
        }
    }
    lanes
}

/// The rows the Tasks view draws: the git graph of the project's tasks,
/// measured from the shown voyage's departure (the prototype's `view_graph`).
pub fn task_rows(
    project: &ProjectBoard,
    now: gear_model::Ts,
    width: usize,
) -> Vec<graph::GraphRow> {
    graph::git_rows(
        &project.graph,
        &task_lanes(project),
        voyage_departure(project, now),
        width,
    )
}

/// The tasks of the Tasks view's picks, in order: the rows with a task and a
/// text — the rows `Enter` opens.
pub fn task_picks(project: &ProjectBoard, now: gear_model::Ts, width: usize) -> Vec<String> {
    task_rows(project, now, width)
        .into_iter()
        .filter(|row| !row.text.is_empty())
        .filter_map(|row| row.task)
        .collect()
}

/// The second view: the work as a git graph (README change 8): `dev` on the
/// left, each task a branch of its lane, the newest work on top.
fn view_tasks(
    project: &ProjectBoard,
    now: gear_model::Ts,
    screen: &Screen,
    canvas: &mut Canvas,
    top: usize,
) -> Vec<usize> {
    let lanes = task_lanes(project);
    let width = canvas.width();
    // One row per row of the graph: dev's line, the cells of the drawing, the
    // deps column and the text after it.
    let rows = task_rows(project, now, width);
    // The deps column stands right of the lanes, one space after the last
    // lane label, before the task text (T-494): one column per line, up to
    // four. The task text follows the drawn columns and one space. It keeps at
    // least 40 columns for the text, so a narrower frame caps the lines.
    let deps_x = 9 + 6 * lanes.len().saturating_sub(1) + 4;
    let cap = 4.min(width.saturating_sub(deps_x + 40));
    let needed = graph::deps_needed(&rows, &project.graph);
    let omitted = needed > cap;
    let drawn = if omitted {
        cap.saturating_sub(1)
    } else {
        needed
    };
    // The `+N` count takes the column after the drawn lines.
    let deps_width = if omitted { drawn + 1 } else { drawn };
    let info_x = deps_x + deps_width + 1;
    let tw = (width as i64 - info_x as i64 - 52).clamp(30, 56) as usize;
    // The head of the columns: dev, the lanes, the deps column and the text's
    // columns.
    canvas.put_styled(2, top, "dev", Style::fg(Color::Ok).and_bold());
    for (index, lane) in lanes.iter().enumerate() {
        canvas.put_styled(
            9 + 6 * index - 1,
            top,
            lane,
            Style::fg(lane_color(&lanes, lane)).and_bold(),
        );
    }
    if drawn > 0 {
        canvas.put_styled(deps_x, top, &"deps"[..drawn.min(4)], Style::fg(Color::Dim));
    }
    canvas.put_styled(
        info_x,
        top,
        &format!("{:<7}{:<tw$}  {:<36}time", "task", "what", "stage"),
        Style::fg(Color::Dim),
    );
    let picks: Vec<(usize, String)> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| !row.text.is_empty())
        .filter_map(|(index, row)| {
            row.task
                .as_ref()
                .map(|task| (top + 1 + index, task.clone()))
        })
        .collect();
    let selected_task = picks.get(screen.selected).map(|(_, task)| task.as_str());
    let deps = graph::deps_rows(&rows, &project.graph, deps_x, drawn, selected_task);
    // The column kept for the lines that do not fit: `+N` on the task's row.
    let plus_x = deps_x + drawn;
    for (index, row) in rows.iter().enumerate() {
        let at = top + 1 + index;
        canvas.put_styled(3, at, "│", Style::fg(Color::Ok));
        for (x, character, style) in &row.cells {
            canvas.put_styled(*x, at, &character.to_string(), *style);
        }
        for (x, character, style) in &deps[index].cells {
            canvas.put_styled(*x, at, &character.to_string(), *style);
        }
        if deps[index].more > 0 {
            canvas.put_styled(
                plus_x,
                at,
                &format!("+{}", deps[index].more),
                Style::fg(Color::Warn).and_bold(),
            );
        }
        if row.text.is_empty() {
            continue;
        }
        canvas.put_styled(info_x, at, &row.text, row.style);
    }
    picks.into_iter().map(|(at, _)| at).collect()
}

/// The fourth view: the log (README change 10) — what changed the course, then
/// every decision and the agents' friction reports (the prototype).
///
/// The user found M4's log a wall of text, so the starred decisions — the ones
/// that changed the course — come first as a summary, and the full list below
/// it holds every decision, its reason under it; the friction reports close the
/// view.
fn view_decisions(
    project: &ProjectBoard,
    screen: &Screen,
    canvas: &mut Canvas,
    top: usize,
) -> Vec<usize> {
    canvas.put_styled(1, top, "WHAT CHANGED THE COURSE  (★)", Style::bold());
    let mut row = top + 1;
    // The starred decisions are the ones that changed the course: the summary
    // the user reads first. A voyage that changed nothing says so.
    let starred: Vec<&crate::DecisionBoard> = project
        .decisions
        .iter()
        .filter(|decision| decision.star)
        .collect();
    if starred.is_empty() {
        canvas.put_styled(3, row, "nothing", Style::fg(Color::Dim));
        row += 1;
    }
    for decision in &starred {
        canvas.put_styled(
            3,
            row,
            &format!("{} ★ {}", clock_of(&decision.at), decision.what),
            Style::fg(Color::Warn),
        );
        row += 1;
    }
    // The full list: every decision is a pick, its reason is not.
    row += 1;
    canvas.put_styled(
        1,
        row,
        &format!("ALL DECISIONS ({})", project.decisions.len()),
        Style::bold(),
    );
    let mut picks = Vec::new();
    for (index, decision) in project.decisions.iter().enumerate() {
        row += 1;
        let picked = index == screen.selected;
        canvas.put_styled(
            1,
            row,
            if picked { "›" } else { " " },
            Style::fg(Color::Acc),
        );
        canvas.put_styled(
            3,
            row,
            &format!(
                "{} {} {:<9}",
                clock_of(&decision.at),
                if decision.star { '★' } else { ' ' },
                decision.kind
            ),
            if decision.star {
                Style::fg(Color::Warn)
            } else {
                Style::PLAIN
            },
        );
        canvas.put_styled(22, row, &decision.what, Style::PLAIN);
        picks.push(row);
        // The reason stands on its own line, as the prototype draws it: a
        // decision line with the reason appended loses it on a narrow screen
        // (F2).
        if let Some(why) = &decision.why {
            row += 1;
            canvas.put_styled(22, row, &format!("why: {why}"), Style::fg(Color::Dim));
        }
    }
    // The friction reports the agents left (§6.1): an open one is unread.
    row += 2;
    canvas.put_styled(1, row, "FRICTION REPORTS FROM AGENTS", Style::bold());
    for friction in &project.frictions {
        row += 1;
        canvas.put_styled(
            3,
            row,
            &format!("{} {:<5}", friction.task, friction.lane),
            lane_style(project, &friction.lane),
        );
        let (status, color) = if friction.closed {
            ("read", Color::Dim)
        } else {
            ("unread", Color::Warn)
        };
        canvas.put_styled(15, row, &format!("{status:<7}"), Style::fg(color));
        canvas.put_styled(23, row, &friction.body, Style::PLAIN);
    }
    picks
}

/// The colour of a ratio of the actual time to the estimate.
fn ratio_color(ratio: f64) -> Color {
    if ratio > 1.5 {
        Color::Bad
    } else if ratio > 1.0 {
        Color::Warn
    } else {
        Color::Ok
    }
}

/// The third view: one row per lane, its time and its tokens with the
/// development and the review apart, and the tasks of an opened lane under it
/// (T-363; the prototype's `view_cost`).
fn view_cost(
    project: &ProjectBoard,
    screen: &Screen,
    canvas: &mut Canvas,
    top: usize,
) -> Vec<usize> {
    let mut picks = Vec::new();
    canvas.put_line(
        3,
        top,
        &Line::styled(
            format!(
                "{:<22}{:>6}{:>7}{:>7}{:>8}{:>12}{:>8}{:>15}{:>10}{:>9}",
                "lane / task",
                "est",
                "act",
                "ratio",
                "rounds",
                "dev tokens",
                "dev $",
                "review tokens",
                "review $",
                "total $"
            ),
            Style::fg(Color::Dim),
        ),
    );
    let mut row = top + 1;
    let mut dev_k_total = 0;
    let mut rev_k_total = 0;
    let mut cost_total = 0.0;
    // A project without `[prices]` priced none of its rounds: the total then
    // names the missing prices once, instead of a dollar sum that does not
    // exist (the user, 2026-10-28).
    let priced = project
        .lanes
        .iter()
        .flat_map(|lane| lane.tasks.iter())
        .filter(|task| task.actual_min.unwrap_or(0) > 0)
        .any(|task| task.dev_cost.is_some() || task.review_cost.is_some());
    let mut unknown = 0;
    for (index, lane) in project.lanes.iter().enumerate() {
        let picked = index == screen.selected;
        // The lane's shown tasks that ran, in plan order.
        let mine: Vec<&TaskBoard> = lane
            .tasks
            .iter()
            .filter(|task| task.actual_min.unwrap_or(0) > 0)
            .collect();
        let mut est = 0;
        let mut act = 0;
        let mut rounds = 0;
        let mut dev_k = 0;
        let mut rev_k = 0;
        let mut dev_c = 0.0;
        let mut rev_c = 0.0;
        for task in &mine {
            est += task.est_min.unwrap_or(0);
            act += task.actual_min.unwrap_or(0);
            rounds += task.rounds;
            // A part the rounds never counted is unknown, not a measured zero
            // (F5): it counts as zero here and is named in the voyage's line.
            dev_k += task.dev_tokens.unwrap_or(0) / 1000;
            rev_k += task.review_tokens.unwrap_or(0) / 1000;
            if let Some(cost) = task.dev_cost {
                dev_c += cost;
            }
            if let Some(cost) = task.review_cost {
                rev_c += cost;
            }
            // Without prices there is no cost to be unknown: the count then
            // counts the tokens only (the user, 2026-10-28).
            if task.dev_tokens.is_none()
                || task.review_tokens.is_none()
                || (priced && (task.dev_cost.is_none() || task.review_cost.is_none()))
            {
                unknown += 1;
            }
        }
        dev_k_total += dev_k;
        rev_k_total += rev_k;
        cost_total += dev_c + rev_c;
        let ratio = if est > 0 {
            act as f64 / est as f64
        } else {
            0.0
        };
        let expanded = screen.opened.iter().any(|open| *open == lane.lane);
        canvas.put_line(
            0,
            row,
            &Line::plain(" ")
                .push(if picked { "›" } else { " " }, Style::fg(Color::Acc))
                .push(" ", Style::PLAIN)
                .push(
                    format!("{}{:<19}", if expanded { "▾ " } else { "▸ " }, lane.lane),
                    lane_style(project, &lane.lane).and_bold(),
                )
                .push(" ", Style::PLAIN)
                .push(format!("{est:>5}m{act:>6}m"), Style::PLAIN)
                .push(format!("{ratio:>6.1}x"), Style::fg(ratio_color(ratio)))
                .push(
                    format!(
                        "{rounds:>8}{dev_k:>11}k{dev_c:>8.2}{rev_k:>14}k{rev_c:>10.2}{:>9.2}",
                        dev_c + rev_c
                    ),
                    Style::bold(),
                ),
        );
        picks.push(row);
        row += 1;
        // An opened lane shows its tasks, one row each, in the same columns.
        if expanded {
            for task in &mine {
                let est = task.est_min.unwrap_or(0);
                let act = task.actual_min.unwrap_or(0);
                let ratio = if est > 0 {
                    act as f64 / est as f64
                } else {
                    0.0
                };
                let dev_k = task.dev_tokens.unwrap_or(0) / 1000;
                let rev_k = task.review_tokens.unwrap_or(0) / 1000;
                let dev_c = task.dev_cost.unwrap_or(0.0);
                let rev_c = task.review_cost.unwrap_or(0.0);
                canvas.put_line(
                    0,
                    row,
                    &Line::plain("     ")
                        .push(format!("{:<20}", task.id), Style::fg(Color::Dim))
                        .push(format!("{est:>5}m{act:>6}m"), Style::fg(Color::Dim))
                        .push(format!("{ratio:>6.1}x"), Style::fg(ratio_color(ratio)))
                        .push(
                            format!(
                                "{:>8}{dev_k:>11}k{dev_c:>8.2}{rev_k:>14}k{rev_c:>10.2}{:>9.2}",
                                task.rounds,
                                dev_c + rev_c
                            ),
                            Style::fg(Color::Dim),
                        ),
                );
                row += 1;
            }
        }
    }
    // One blank row, then the voyage's own sum.
    row += 1;
    let mut total = format!(
        "this voyage: {:.1}M tokens (development {:.1}M, review {:.1}M), {}",
        (dev_k_total + rev_k_total) as f64 / 1000.0,
        dev_k_total as f64 / 1000.0,
        rev_k_total as f64 / 1000.0,
        if priced {
            format!("${cost_total:.2}")
        } else {
            "no prices set".to_string()
        }
    );
    if unknown > 0 {
        // The sum is what is known: the unknown parts are named.
        total.push_str(&format!(" (known so far; {unknown} unknown)"));
    }
    canvas.put_line(3, row, &Line::styled(total, Style::bold()));
    picks
}

/// The width of the Now view's task column (T-359): the prototype's
/// `tw = max(30, min(52, wd - 110))`.
fn project_tw(width: usize) -> usize {
    width.saturating_sub(110).min(52).max(30)
}

/// The minutes a voyage has used, as the header measures them: from its
/// departure to now while it sails, to its merge when it landed.
fn voyage_used_minutes(voyage: Option<&crate::VoyageEntry>, now: gear_model::Ts) -> i64 {
    let Some(voyage) = voyage else {
        return 0;
    };
    let departure = voyage.departure.unwrap_or(now);
    let end = if voyage.sailing {
        now
    } else {
        voyage.ended.unwrap_or(now)
    };
    (end - departure).max(0) / 60
}

/// A text cut to `width` characters (the prototype's `[:width]`).
fn cut(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

/// The clock of a decision: the time of its line (`2026-10-27 10:05` →
/// `10:05`).
fn clock_of(at: &str) -> &str {
    at.rsplit(' ').next().unwrap_or(at)
}

/// The colour of a quota reading: red from 90 %, yellow from 60 %.
fn quota_color(used_percent: f64) -> Color {
    if used_percent >= 90.0 {
        Color::Bad
    } else if used_percent >= 60.0 {
        Color::Warn
    } else {
        Color::Ok
    }
}

/// The first view: the lanes, what needs a look, the quota and the done
/// sentence.
fn view_now(
    project: &ProjectBoard,
    now: gear_model::Ts,
    screen: &Screen,
    canvas: &mut Canvas,
    top: usize,
) -> Vec<usize> {
    // T-359: one table, one row per lane (the prototype's `fn view_now`).
    let tw = project_tw(canvas.width);
    let voyage = project.shown.as_ref();
    let sailing = voyage.is_some_and(|voyage| voyage.sailing);
    let landed = voyage.is_some_and(|voyage| !voyage.sailing);
    let used = voyage_used_minutes(voyage, now);

    canvas.put_styled(3, top, "LANE", Style::bold());
    canvas.put_styled(9, top, "NOW DOING", Style::bold());
    canvas.put_styled(11 + tw, top, "STAGE", Style::bold());
    canvas.put_styled(34 + tw, top, "TIME", Style::bold());
    canvas.put_styled(46 + tw, top, "LANE DONE", Style::bold());
    canvas.put_styled(76 + tw, top, "ENDS", Style::bold());
    canvas.put_styled(
        86 + tw,
        top,
        "(Enter: the lane's work)",
        Style::fg(Color::Dim),
    );

    let mut picks = Vec::new();
    let mut row = top + 1;
    let mut ends: Vec<(usize, i64)> = Vec::new();
    for (index, lane) in project.lanes.iter().enumerate() {
        let picked = index == screen.selected;
        // The mark stays in the accent colour, picked or not (the frame then
        // lights the selected row).
        canvas.put_styled(
            1,
            row,
            if picked { "›" } else { " " },
            Style::fg(Color::Acc),
        );
        canvas.put_styled(
            3,
            row,
            &format!("{:<5}", lane.lane),
            lane_style(project, &lane.lane).and_bold(),
        );
        // The lane's first open task in plan order is what it does now. A
        // cancelled task is not work, whatever stage the graph still gives it
        // (T-347's rule, the captain's answer M-6103).
        match lane
            .tasks
            .iter()
            .find(|task| task.state != gear_model::TaskState::Cancelled && task.stage.is_open())
        {
            Some(task) => {
                let title = format!("{} {}", task.id, task.title);
                canvas.put_styled(9, row, &cut(&title, tw), Style::PLAIN);
                canvas.put_styled(
                    11 + tw,
                    row,
                    &cut(&task.stage_words, 22),
                    Style::fg(task.stage.color()),
                );
                let actual = task.actual_min.unwrap_or(0);
                let estimate = task.est_min.unwrap_or(0);
                let over = actual > estimate;
                canvas.put_styled(
                    34 + tw,
                    row,
                    &format!("{actual}/{estimate}m{}", if over { " ⚠" } else { "" }),
                    if over {
                        Style::fg(Color::Bad)
                    } else {
                        Style::PLAIN
                    },
                );
            }
            None => {
                let (reason, style) = idle_reason_text(project, lane, landed);
                canvas.put_styled(9, row, &cut(&reason, tw + 30), style);
            }
        }
        // The lane's own progress: the merged tasks of the ones it shows (a
        // cancelled task is not work), and the estimate still left.
        let mine: Vec<&TaskBoard> = lane
            .tasks
            .iter()
            .filter(|task| task.state != gear_model::TaskState::Cancelled)
            .collect();
        let merged = mine
            .iter()
            .filter(|task| task.stage == graph::Stage::Merged)
            .count();
        let left: i64 = mine
            .iter()
            .filter(|task| task.stage != graph::Stage::Merged)
            .map(|task| task.est_min.unwrap_or(0))
            .sum();
        let k = merged * 10 / mine.len().max(1);
        canvas.put_styled(
            46 + tw,
            row,
            &format!("{}{}", "█".repeat(k), "░".repeat(10 - k)),
            lane_style(project, &lane.lane),
        );
        canvas.put_styled(
            57 + tw,
            row,
            &format!("{merged} of {} · {left}m left", mine.len()),
            Style::PLAIN,
        );
        // When the lane ends: the time used plus its forecast from now.
        if sailing {
            if let Some(end) = project.forecast.iter().find(|end| end.lane == lane.lane) {
                let end = used + end.end_min;
                canvas.put_styled(76 + tw, row, &format!("~{}", hm(end)), Style::PLAIN);
                ends.push((index, end));
            }
        }
        picks.push(row);
        row += 1;
    }
    // One line on whether the work is balanced: the first and the last lane
    // to end, the first in lane order on a tie.
    if sailing && !ends.is_empty() {
        let mut first = ends[0];
        let mut last = ends[0];
        for end in &ends {
            if end.1 < first.1 {
                first = *end;
            }
            if end.1 > last.1 {
                last = *end;
            }
        }
        let gap = last.1 - first.1;
        if gap <= 30 {
            canvas.put_styled(
                3,
                row,
                &format!("✔ balanced: every lane ends within {}", hm(gap)),
                Style::fg(Color::Ok),
            );
        } else {
            canvas.put_styled(
                3,
                row,
                &format!(
                    "✗ not balanced: {} ends {} after {}",
                    project.lanes[last.0].lane,
                    hm(gap),
                    project.lanes[first.0].lane
                ),
                Style::fg(Color::Bad),
            );
        }
    }
    // The rest follows two rows below the balance line (T-360's block starts
    // there).
    // The rest starts two rows below the balance line (T-359, T-360): the
    // items to handle, then the quota on one line, then the overlaps.
    row = top + project.lanes.len() + 3;
    // The items to handle: what needs a look, a lane that will sit idle, and a
    // task that waits on someone — each with who acts.
    let mut items: Vec<(&str, String, Style)> = Vec::new();
    for text in needs_a_look(project) {
        items.push(("⚠", text, Style::fg(Color::Warn)));
    }
    for lane in &project.forecast {
        for (gap, dep) in &lane.waits {
            if *gap < 20 {
                continue;
            }
            let queue = project.lanes.iter().find(|board| board.lane == lane.lane);
            // The lane's first task in plan order that depends on the blocker.
            let next = queue.and_then(|board| {
                board
                    .tasks
                    .iter()
                    .filter(|task| task.deps.iter().any(|id| id == dep))
                    .min_by_key(|task| (task.rank, task.id.as_str()))
            });
            let owner = project
                .lanes
                .iter()
                .find(|board| board.tasks.iter().any(|task| &task.id == dep));
            let (Some(next), Some(owner)) = (next, owner) else {
                continue;
            };
            items.push((
                "⚠",
                format!(
                    "{} will sit idle ~{}: its next task {} waits for {dep} ({}) — captain: \
                     give {} work it can start now, or move {dep} to {}",
                    lane.lane,
                    hm(*gap),
                    next.id,
                    owner.lane,
                    lane.lane,
                    lane.lane
                ),
                Style::fg(Color::Bad),
            ));
        }
    }
    for text in &project.waiting_on {
        items.push(("⏳", text.clone(), Style::PLAIN));
    }
    let heading = format!("TO HANDLE ({})", items.len());
    let heading_style = if items.is_empty() {
        Style::bold()
    } else {
        Style::fg(Color::Warn).and_bold()
    };
    canvas.put_styled(1, row, &heading, heading_style);
    row += 1;
    if items.is_empty() {
        canvas.put_styled(3, row, "nothing", Style::fg(Color::Dim));
        row += 1;
    } else {
        for (mark, text, style) in &items {
            // The mark and the space after it carry its colour; the hourglass
            // is a wide character, so its two continuation cells come first and
            // the text starts one cell later, as a terminal draws it.
            let lead = if *mark == "⏳" {
                format!("{mark}  ")
            } else {
                format!("{mark} ")
            };
            canvas.put_styled(3, row, &lead, *style);
            canvas.put_styled(3 + lead.chars().count(), row, text, *style);
            row += 1;
        }
    }
    row += 1;
    // The quota on one line, a window after a window (T-360).
    canvas.put_styled(1, row, "Quota", Style::bold());
    let mut windows: Vec<(&QuotaReading, &QuotaWindow)> = project
        .quota
        .iter()
        .flat_map(|reading| reading.windows.iter().map(move |window| (reading, window)))
        .collect();
    // The window that resets first comes first; a window without a reset
    // comes last. The provider's name settles a tie.
    windows.sort_by(|(left, left_window), (right, right_window)| {
        let reset = |window: &QuotaWindow| window.resets_at.unwrap_or(i64::MAX);
        reset(left_window)
            .cmp(&reset(right_window))
            .then_with(|| left.provider.cmp(&right.provider))
            .then_with(|| left_window.name.cmp(&right_window.name))
    });
    if windows.is_empty() {
        canvas.put_styled(9, row, "none read yet", Style::fg(Color::Dim));
    } else {
        let mut x = 9;
        for (reading, window) in &windows {
            // A reset within a day is a clock time; a later one carries its
            // day.
            let reset = match window.resets_at {
                Some(reset) if reset - now < 86_400 => crate::idle::clock(reset),
                Some(reset) => crate::idle::month_day(reset),
                None => "-".to_string(),
            };
            let text = format!(
                "{} {} {}% · resets {reset}",
                reading.provider,
                window.name,
                window.used_percent.round() as i64
            );
            canvas.put_styled(x, row, &text, Style::fg(quota_color(window.used_percent)));
            x += text.chars().count() + 5;
        }
    }
    row += 1;
    // The overlaps with the integration branch (T-188): information lines.
    // They never wait for the captain.
    if !project.overlaps.is_empty() {
        row += 1;
        for overlap in &project.overlaps {
            canvas.put_styled(1, row, &crate::overlap_line(overlap), Style::PLAIN);
            row += 1;
        }
    }
    picks
}

/// The words and the style of a lane that has nothing open (the prototype's
/// `idle_reason`): the reason it sits, or what its next task waits for.
fn idle_reason_text(project: &ProjectBoard, lane: &LaneBoard, landed: bool) -> (String, Style) {
    if landed {
        return (
            crate::RETIRED_AT_THE_LANDING.to_string(),
            Style::fg(Color::Dim),
        );
    }
    // A lane that helps on the captain's instruction says so, in the accent
    // colour, as it did before the table (T-323).
    if let Some(helping) = &lane.helping {
        return (helping.clone(), Style::fg(Color::Acc));
    }
    // A reason of the seat itself says what the lane is doing about it.
    if matches!(
        lane.idle,
        Some(crate::idle::IdleReason::Quota { .. })
            | Some(crate::idle::IdleReason::Parked { .. })
            | Some(crate::idle::IdleReason::Held { .. })
            | Some(crate::idle::IdleReason::Waiting { .. })
    ) {
        return (
            lane.idle_text.clone().unwrap_or_default(),
            Style::fg(Color::Bad),
        );
    }
    // The next task of the lane, and the first dependency it waits for.
    let Some(next) = lane
        .tasks
        .iter()
        .find(|task| matches!(task.stage, graph::Stage::Deps | graph::Stage::Queued))
    else {
        return (
            "nothing left planned for this lane".to_string(),
            Style::fg(Color::Bad),
        );
    };
    for dep in &next.deps {
        for owner in &project.lanes {
            let Some(task) = owner.tasks.iter().find(|task| &task.id == dep) else {
                continue;
            };
            if task.stage != graph::Stage::Merged {
                return (
                    format!(
                        "next {} waits for {} ({}, {})",
                        next.id, dep, owner.lane, task.stage_words
                    ),
                    Style::fg(Color::Bad),
                );
            }
            break;
        }
    }
    (
        format!("next {} is ready but not delivered", next.id),
        Style::fg(Color::Bad),
    )
}

/// The things the captain should look at: a shown task that is still open and
/// either took more than one round or ran past its estimate, in plan order.
/// A merged task is history: the Time & tokens view keeps it (T-360).
fn needs_a_look(project: &ProjectBoard) -> Vec<String> {
    // The plan's order, not the lanes' grouping: the tasks flattened and
    // sorted by rank, then id, as the store ranks them (F2 of the review).
    let mut tasks: Vec<(&str, &TaskBoard)> = project
        .lanes
        .iter()
        .flat_map(|lane| {
            lane.tasks
                .iter()
                .map(move |task| (lane.lane.as_str(), task))
        })
        .collect();
    tasks.sort_by(|(_, left), (_, right)| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut looks = Vec::new();
    for (lane, task) in tasks {
        if !task.stage.is_open() {
            continue;
        }
        let actual = task.actual_min.unwrap_or(0);
        let estimate = task.est_min.unwrap_or(0);
        if task.rounds > 1 {
            looks.push(format!(
                "{} {lane} round {}, {actual}/{estimate}m: the captain scopes what is left",
                task.id, task.rounds
            ));
        } else if actual > estimate {
            looks.push(format!(
                "{} {lane} {actual}/{estimate}m, past its estimate: the captain scopes what is left",
                task.id
            ));
        }
    }
    looks
}

/// Draw the panel over the frame: a box at the right of the screen, as the
/// prototype draws its window over the board.
fn draw_panel(canvas: &mut Canvas, panel: &[Line], top: usize) {
    // One width for the whole box, borders included (F3): the interior rows
    // are padded or cut to it, so the right border is always inside the
    // canvas.
    // The box never grows past the canvas (F3): a long row is cut, not the
    // whole panel. The interior width follows the box.
    let available = canvas.width.saturating_sub(6);
    let inner = panel
        .iter()
        .map(Line::len)
        .max()
        .unwrap_or(18)
        .max(18)
        .min(available);
    let total = inner + 4;
    if total + 2 > canvas.width {
        // It does not fit at all: no half a box is drawn.
        return;
    }
    let left = canvas.width.saturating_sub(total + 1);
    // The footer row is reserved: the bottom border stands above it (F3).
    let bottom = (top + panel.len() + 1).min(canvas.height - 2);
    // The box: a top border, the rows, a bottom border.
    canvas.put(left, top, &format!("┌{}┐", "─".repeat(inner + 2)));
    for (index, row) in panel.iter().enumerate() {
        let line = top + 1 + index;
        if line >= bottom {
            break;
        }
        // The row keeps its styles; the padding and the borders are plain.
        canvas.put(left, line, "│ ");
        canvas.put_line(left + 2, line, &row_cut(row, inner));
        canvas.put(left + 2 + inner, line, " │");
    }
    canvas.put(left, bottom, &format!("└{}┘", "─".repeat(inner + 2)));
}

/// The first `width` characters of a line, padded with plain spaces.
fn row_cut(row: &Line, width: usize) -> Line {
    let mut cut = Line::new();
    for (character, style) in row.cells().take(width) {
        cut = cut.push(character.to_string(), style);
    }
    cut.pad(width)
}

/// The lane's panel: what it did, what it does now and what it has left.
///
/// `tasks` are the graph's records of the project (T-346): the panel shows the
/// lane's own, each with the stage the store's facts gave it.
pub fn lane_panel(lane: &LaneBoard, tasks: &[graph::GraphTask]) -> Vec<String> {
    styled_lane_panel(lane, tasks, Color::Dim)
        .into_iter()
        .map(|line| line.text)
        .collect()
}

/// Whether the graph task is in the lane's queue: a task a plan left without a
/// lane shows in the `unassigned` lane.
fn in_lane(task: &graph::GraphTask, lane: &str) -> bool {
    if task.lane.is_empty() {
        lane == "unassigned"
    } else {
        task.lane == lane
    }
}

/// The lane's panel in the prototype's colours; `color` is the lane's own.
pub fn styled_lane_panel(lane: &LaneBoard, tasks: &[graph::GraphTask], color: Color) -> Vec<Line> {
    let mine = || tasks.iter().filter(move |task| in_lane(task, &lane.lane));
    let mut rows = vec![
        Line::styled(format!("lane {}", lane.lane), Style::fg(color).and_bold()),
        Line::new(),
        Line::styled("DONE", Style::bold()),
    ];
    for task in mine().filter(|task| task.stage == graph::Stage::Merged) {
        let rounds = if task.rounds > 1 {
            format!(" {}r", task.rounds)
        } else {
            String::new()
        };
        rows.push(Line::styled(
            format!(
                "  {} {} {:<30} {}/{}m{}",
                task.stage.symbol(),
                task.id,
                task.title.chars().take(30).collect::<String>(),
                task.actual_min.unwrap_or(0),
                task.est_min.unwrap_or(0),
                rounds
            ),
            Style::fg(task.stage.color()),
        ));
    }
    rows.push(Line::new());
    rows.push(Line::styled("NOW", Style::bold()));
    for task in mine().filter(|task| task.stage.is_open()) {
        // The stage says what the task does now, in the prototype's words —
        // the same words as the lane lines (T-347).
        rows.push(Line::styled(
            format!(
                "  {} {} {:<30} {}",
                task.stage.symbol(),
                task.id,
                task.title.chars().take(30).collect::<String>(),
                graph::Stage::words_of(
                    task.stage,
                    &task.deps,
                    task.rounds,
                    task.moved_to.as_deref()
                )
            ),
            Style::fg(task.stage.color()),
        ));
    }
    rows.push(Line::new());
    rows.push(Line::styled("LEFT, in order", Style::bold()));
    let mut left = 0;
    let mut minutes = 0;
    for task in
        mine().filter(|task| matches!(task.stage, graph::Stage::Deps | graph::Stage::Queued))
    {
        left += 1;
        minutes += task.est_min.unwrap_or(0);
        let needs = if task.deps.is_empty() {
            String::new()
        } else {
            format!("  needs {}", task.deps.join(","))
        };
        rows.push(Line::plain(format!(
            "  ○ {} {:<30} {}m{}",
            task.id,
            task.title.chars().take(30).collect::<String>(),
            task.est_min.unwrap_or(0),
            needs
        )));
    }
    rows.push(Line::styled(
        format!("left: {left} tasks, {minutes}m of estimates"),
        Style::fg(Color::Dim),
    ));
    rows
}

/// The colour of a lane of the project (its panel's title).
pub fn lane_color_of(project: &ProjectBoard, lane: &str) -> Color {
    lane_color(&lane_order(project), lane)
}

/// A bar of `width` cells: how much of `total` is `part`.
pub fn bar(part: i64, total: i64, width: usize) -> String {
    if total <= 0 {
        return "░".repeat(width);
    }
    let filled = ((part as f64 / total as f64) * width as f64)
        .round()
        .max(0.0) as usize;
    let filled = filled.min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

/// Minutes as the board reads them: 45m, 3h05m.
pub fn hours(minutes: i64) -> String {
    if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h{:02}m", minutes / 60, (minutes % 60))
    }
}

/// The idle reason of a lane, in words (the board's own).
pub fn reason_word(reason: &IdleReason) -> &'static str {
    match reason {
        IdleReason::Quota { .. } => "quota",
        IdleReason::Parked { .. } => "parked",
        IdleReason::Held { .. } => "held",
        IdleReason::Waiting { .. } => "waiting",
        IdleReason::DepWait { .. } => "dep wait",
        IdleReason::Undelivered { .. } => "undelivered",
        IdleReason::NoWork => "no work",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_glyph_takes_two_cells_and_its_continuation_is_not_printed() {
        // T-353 (review F1): the canvas keeps the prototype's columns — the
        // glyph and its continuation — and the text leaves the continuation
        // out, so a terminal draws the same columns.
        let mut canvas = Canvas::new(20, 8);
        let taken = canvas.put_wide(1, 0, " M5 ⛵ ", Style::fg(Color::Warn));

        // The prototype's label: six characters, and the glyph's second cell.
        assert_eq!(taken, 7, "the cells the label took");
        assert_eq!(canvas.char_at(1, 0), ' ');
        assert_eq!(canvas.char_at(5, 0), '⛵');
        assert!(canvas.is_continuation(6, 0), "the glyph's second cell");
        assert_eq!(
            canvas.char_at(6, 0),
            ' ',
            "a space in the cell (the golden planes)"
        );
        assert_eq!(
            canvas.style_at(6, 0),
            Style::fg(Color::Warn),
            "in the label's style"
        );
        assert!(
            !canvas.is_continuation(7, 0),
            "the label's last space is its own"
        );
        // An ordinary text after the label keeps the trailing spaces printed.
        canvas.put(8, 0, "x");
        let line = canvas.text().lines().next().unwrap_or_default().to_string();
        assert_eq!(line, "  M5 ⛵ x", "the continuation cell prints nothing");
        let columns: usize = line
            .chars()
            .map(|character| {
                if matches!(character, '⛵' | '⚓') {
                    2
                } else {
                    1
                }
            })
            .sum();
        assert_eq!(columns, 9, "nine cells, nine terminal columns: {line:?}");
    }

    #[test]
    fn a_write_over_half_of_a_wide_glyph_blanks_the_other_half() {
        // T-353 (review F3): the terminal draws a glyph in two columns, so a
        // half-overwritten glyph would shift the columns after it.
        let mut canvas = Canvas::new(20, 8);
        canvas.put_wide(1, 0, "M5 ⛵", Style::fg(Color::Warn));

        // A write onto the continuation blanks the glyph's leading cell.
        assert_eq!(canvas.char_at(4, 0), '⛵');
        assert!(canvas.is_continuation(5, 0));
        canvas.put(5, 0, "x");
        assert_eq!(canvas.char_at(4, 0), ' ', "the glyph's first half is blank");
        assert!(!canvas.is_continuation(4, 0));
        assert_eq!(canvas.char_at(5, 0), 'x', "the write is where it landed");
        assert_eq!(canvas.text().lines().next().unwrap_or_default(), " M5  x");

        // A write onto a leading cell clears its continuation: the second half
        // stays a space in the glyph's style.
        let mut canvas = Canvas::new(20, 8);
        canvas.put_wide(1, 0, "M5 ⛵", Style::fg(Color::Warn));
        canvas.put(4, 0, "y");
        assert!(
            !canvas.is_continuation(5, 0),
            "the glyph has no second half any more"
        );
        assert_eq!(canvas.char_at(5, 0), ' ');
        assert_eq!(canvas.style_at(5, 0), Style::fg(Color::Warn));
        assert_eq!(canvas.text().lines().next().unwrap_or_default(), " M5 y");
    }

    #[test]
    fn an_eta_reads_in_minutes_and_the_header_reads_it_back_in_hours() {
        // T-354: the plan's units, as `eta_seconds` in gear-app reads them.
        assert_eq!(duration_minutes("8h"), 480);
        assert_eq!(duration_minutes("1d 4h"), 1_680);
        assert_eq!(duration_minutes("90m"), 90);
        assert_eq!(duration_minutes("2.5 hours"), 150);
        // An eta the board cannot read is no time at all.
        assert_eq!(duration_minutes(""), 0);
        assert_eq!(duration_minutes("soon"), 0);
        // The folded header always reads the hours, the minutes in two digits.
        assert_eq!(hm(0), "0h00m");
        assert_eq!(hm(25), "0h25m");
        assert_eq!(hm(346), "5h46m");
    }
}
