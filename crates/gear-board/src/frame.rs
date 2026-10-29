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
use crate::style::{lane_color, lane_width, Color, Line, Style};
use crate::{Board, LaneBoard, ProjectBoard, TaskBoard, VoyageEntry};

/// The four views of the board: the tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// The lanes, what they do now and what they have left.
    Now,
    /// The work as a graph.
    Tasks,
    /// The time and the tokens, per task.
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

    /// The label of the tab on the tab row: ` 1 Now `.
    pub fn label(self) -> String {
        format!(" {} {} ", self.number(), self.title())
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
/// `detail` is carried for the unfolded header (the `d` key); it is not read
/// yet.
pub struct Screen {
    pub tab: Tab,
    pub selected: usize,
    pub detail: bool,
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
pub struct Canvas {
    width: usize,
    height: usize,
    cells: Vec<char>,
    styles: Vec<Style>,
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
            self.cells[y * self.width + column] = character;
            self.styles[y * self.width + column] = style;
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
            self.cells[y * self.width + column] = character;
            self.styles[y * self.width + column] = style;
        }
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
                self.styles[target * self.width + column] = from.styles[source * from.width + column];
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
            self.styles[y * self.width + column] = Style { fg: None, bold: false, reverse: true };
        }
    }

    /// The screen as plain text: the rows, each cut at the edge, without a
    /// terminal code.
    pub fn text(&self) -> String {
        (0..self.height).map(|row| self.row_text(row, None)).collect::<Vec<_>>().join("\n")
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
                Some(false) => Style { fg: None, ..styles[column] },
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
            let text: String = cells[start..column].iter().collect();
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
    if let Some(row) = picks.get(screen.selected).map(|row| row.saturating_sub(offset)) {
        if row >= top && row < last {
            canvas.light_row(row);
        }
    }
    if screen.tab == Tab::Tasks {
        // The Tasks view's legend (the prototype's row `h - 2`).
        canvas.put_styled(1, height.saturating_sub(2), TASKS_LEGEND, Style::fg(Color::Dim));
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
/// The tabs follow the box directly, and the view's heading comes two rows
/// after the tab row (its own row and the spacer under the tabs). Every text
/// is put at the prototype's column and cut at the edge, as
/// [`Canvas::put_styled`] cuts it.
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
            let cells = put_wide(canvas, x, 0, &label, voyage_style(voyage, entry));
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
        let state = if voyage.sailing { "⛵ sailing" } else { "⚓ landed" };
        let line = format!("{}: {} · {}", voyage.milestone, voyage.title, state);
        put_wide(canvas, 1, 1, &line, Style::bold());
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

    // The tab row follows the box directly: the four tabs, the open one in
    // reverse video, the others dim.
    let tab_row = row;
    let mut tabs = Vec::new();
    let mut x = 1;
    for tab in Tab::all() {
        let label = tab.label();
        if tab == screen.tab {
            canvas.put(x, tab_row, &label);
            canvas.reverse(tab_row, x, label.chars().count());
        } else {
            canvas.put_styled(x, tab_row, &label, Style::fg(Color::Dim));
        }
        tabs.push((x, x + label.chars().count() - 1, tab));
        x += label.chars().count() + 1;
    }

    Layout { top: tab_row + 2, tab_row, tabs, voyages }
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
        Style { fg: None, bold: true, reverse: true }
    } else {
        Style::fg(Color::Dim)
    }
}

/// Put a text that may hold a wide glyph (the prototype's ⛵ and ⚓, and ⏳):
/// the glyph takes two cells, the second its continuation, a space with the
/// same style. It returns the number of cells the text took.
fn put_wide(canvas: &mut Canvas, x: usize, y: usize, text: &str, style: Style) -> usize {
    let mut cells = String::with_capacity(text.len() + 2);
    for character in text.chars() {
        cells.push(character);
        if is_wide(character) {
            cells.push(' ');
        }
    }
    let width = cells.chars().count();
    canvas.put_styled(x, y, &cells, style);
    width
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

/// The selection mark of a row: `›` in the accent colour.
fn mark(selected: bool) -> Line {
    if selected {
        Line::styled("›", Style::fg(Color::Acc))
    } else {
        Line::plain(" ")
    }
}

/// The lanes of the project, in seat order: the order of the lane colours.
fn lane_order(project: &ProjectBoard) -> Vec<&str> {
    project.lanes.iter().map(|lane| lane.lane.as_str()).collect()
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
        Tab::Cost => view_tokens(project, screen, canvas, top),
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
pub fn task_rows(project: &ProjectBoard, now: gear_model::Ts, width: usize) -> Vec<graph::GraphRow> {
    graph::git_rows(&project.graph, &task_lanes(project), voyage_departure(project, now), width)
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
    let info_x = 9 + 6 * lanes.len().saturating_sub(1) + 6;
    let tw = (width as i64 - info_x as i64 - 52).clamp(30, 56) as usize;
    // The head of the columns: dev, the lanes and the text's columns.
    canvas.put_styled(2, top, "dev", Style::fg(Color::Ok).and_bold());
    for (index, lane) in lanes.iter().enumerate() {
        canvas.put_styled(
            9 + 6 * index - 1,
            top,
            lane,
            Style::fg(lane_color(&lanes, lane)).and_bold(),
        );
    }
    canvas.put_styled(
        info_x,
        top,
        &format!("{:<7}{:<tw$}  {:<36}time", "task", "what", "stage"),
        Style::fg(Color::Dim),
    );
    // One row per row of the graph: dev's line, the cells of the drawing and
    // the text after it.
    let rows = task_rows(project, now, width);
    let mut picks: Vec<(usize, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let at = top + 1 + index;
        canvas.put_styled(3, at, "│", Style::fg(Color::Ok));
        for (x, character, style) in &row.cells {
            canvas.put_styled(*x, at, &character.to_string(), *style);
        }
        if row.text.is_empty() {
            continue;
        }
        canvas.put_styled(info_x, at, &row.text, row.style);
        if let Some(task) = &row.task {
            picks.push((at, task.clone()));
        }
    }
    // The selected pick's dependencies light up: what it needs, and what waits
    // for it.
    if let Some((_, selected)) = picks.get(screen.selected) {
        let deps_of = |task: &str| -> Vec<String> {
            project
                .graph
                .iter()
                .find(|row| row.id == task)
                .map(|row| row.deps.clone())
                .unwrap_or_default()
        };
        let needed = deps_of(selected);
        for (at, task) in &picks {
            let waits = deps_of(task);
            let mark = if needed.contains(task) {
                // The row's task is what the selected one waits for.
                format!("◂ needed by {selected}")
            } else if waits.contains(selected) {
                // The row's task waits for the selected one.
                format!("▸ waits for {selected}")
            } else {
                continue;
            };
            canvas.put_styled(
                width.saturating_sub(22),
                *at,
                &mark,
                Style::fg(Color::Warn).and_bold(),
            );
        }
    }
    picks.into_iter().map(|(at, _)| at).collect()
}

/// The fourth view: the captain's decisions (★ = worth a look at landing) and
/// the agents' friction reports (the prototype).
fn view_decisions(
    project: &ProjectBoard,
    screen: &Screen,
    canvas: &mut Canvas,
    top: usize,
) -> Vec<usize> {
    canvas.put_line(
        0,
        top,
        &Line::styled("DECISIONS MADE  (★ = worth a look at landing)", Style::bold()),
    );
    let mut picks = Vec::new();
    let mut row = top + 1;
    // The starred decisions are the ones the user reads in full (F1): the
    // others are counted by kind below.
    let starred: Vec<&crate::DecisionBoard> =
        project.decisions.iter().filter(|decision| decision.star).collect();
    for (index, decision) in starred.iter().enumerate() {
        let picked = index == screen.selected;
        canvas.put_line(
            0,
            row,
            &mark(picked)
                .push(" ", Style::PLAIN)
                .push(
                    format!("{} ★ {:<9}", clock_of(&decision.at), decision.kind),
                    Style::fg(Color::Warn),
                )
                .push(" ", Style::PLAIN)
                .push(decision.what.clone(), Style::PLAIN),
        );
        // The decision is what the selection moves over; its reason is not.
        picks.push(row);
        row += 1;
        // The reason stands on its own line, as the prototype draws it: a
        // decision line with the reason appended loses it on a narrow screen
        // (F2).
        if let Some(why) = &decision.why {
            canvas.put_line(
                0,
                row,
                &Line::plain(format!("{:22}", ""))
                    .push(format!("why: {why}"), Style::fg(Color::Dim)),
            );
            row += 1;
        }
    }
    // The count of the decisions by kind (§7.2): the landing review shows it
    // next to the starred lines.
    if !project.decisions.is_empty() {
        row += 1;
        canvas.put_line(0, row, &Line::plain(format!("by kind: {}", counts_of_kind(project))));
        row += 1;
    }
    // The friction reports the agents left (§6.1): an open one is unread.
    row += 1;
    canvas.put_line(0, row, &Line::styled("FRICTION REPORTS FROM AGENTS", Style::bold()));
    row += 1;
    let width = lane_width(project.frictions.iter().map(|friction| friction.lane.as_str()), 6, 1);
    for friction in &project.frictions {
        let (status, color) =
            if friction.closed { ("closed", Color::Dim) } else { ("open", Color::Warn) };
        canvas.put_line(
            0,
            row,
            &Line::plain("  ")
                .push(
                    format!("{} {:<width$}", friction.task, friction.lane),
                    lane_style(project, &friction.lane),
                )
                .push(format!("{status:<8}"), Style::fg(color))
                .push(friction.body.clone(), Style::PLAIN),
        );
        row += 1;
    }
    picks
}

/// The decisions of the project that are not starred, by kind: `2 scope, 1 cut`.
///
/// The starred ones are read in full above; this is the count of the others
/// (§7.2).
fn counts_of_kind(project: &ProjectBoard) -> String {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for decision in &project.decisions {
        if decision.star {
            continue;
        }
        *counts.entry(decision.kind.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(kind, count)| format!("{count} {kind}"))
        .collect::<Vec<_>>()
        .join(", ")
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

/// The third view: the time and the tokens of every task that ran (the
/// prototype).
fn view_tokens(
    project: &ProjectBoard,
    screen: &Screen,
    canvas: &mut Canvas,
    top: usize,
) -> Vec<usize> {
    let mut picks = Vec::new();
    let mut row = top + 1;
    let mut tokens_total = 0;
    let mut cache_total = 0;
    let mut cost_total = 0.0;
    let mut unknown = 0;
    let tasks = token_tasks(project);
    // A project without `[prices]` priced none of its rounds: the cost column
    // then says nothing at all, and the total names the missing prices once,
    // instead of a `?` in every cell and a dollar sum that does not exist
    // (the user, 2026-10-28).
    let priced = tasks.iter().any(|(_, task)| task.cost_usd.is_some());
    let width = lane_width(tasks.iter().map(|(lane, _)| *lane), 6, 1);
    canvas.put_line(
        0,
        top,
        &Line::styled(
            format!(
                "  {:<7}{:<width$}{:>5}{:>6}{:>8}{:>8}{:>9}{:>9}{:>9}",
                "task", "lane", "est", "act", "ratio", "rounds", "tokens", "cache", "cost"
            ),
            Style::fg(Color::Dim),
        ),
    );
    for (index, (lane, task)) in tasks.into_iter().enumerate() {
        let picked = index == screen.selected;
        let estimate = task.est_min.unwrap_or(0);
        let actual = task.actual_min.unwrap_or(0);
        let ratio = if estimate > 0 {
            actual as f64 / estimate as f64
        } else {
            0.0
        };
        // The tokens and the cost are unknown when no round counted them
        // (F5): they are not a measured zero.
        let tokens = task.tokens;
        let cache_read = task.cache_read;
        let cost = task.cost_usd;
        if tokens.is_none() {
            unknown += 1;
        } else {
            tokens_total += tokens.unwrap_or(0);
        }
        if let Some(cache_read) = cache_read {
            cache_total += cache_read;
        }
        // Without prices there is no cost to be unknown: the count of the
        // unknown parts then counts the tokens only.
        if priced {
            if cost.is_none() {
                unknown += 1;
            } else {
                cost_total += cost.unwrap_or(0.0);
            }
        }
        let tokens_text = match tokens {
            Some(tokens) => format!("{}k", tokens / 1000),
            None => "?".to_string(),
        };
        let cache_text = match cache_read {
            Some(cache_read) => format!("{}k", cache_read / 1000),
            None => "?".to_string(),
        };
        let cost_text = match cost {
            Some(cost) => format!("${cost:>6.2}"),
            None if priced => "      ?".to_string(),
            None => String::new(),
        };
        let row_style = Style::PLAIN;
        canvas.put_line(
            0,
            row,
            &mark(picked)
                .push(" ", Style::PLAIN)
                .push(format!("{:<7}", task.id), row_style)
                .push(format!("{lane:<width$}"), lane_style(project, lane))
                .push(format!("{estimate:>4}m{actual:>5}m"), row_style)
                .push(format!("{ratio:>7.1}x"), Style::fg(ratio_color(ratio)))
                .push(
                    format!(
                        "{:>8}{tokens_text:>9}{cache_text:>9}  {cost_text}",
                        task.stored_rounds
                    ),
                    row_style,
                ),
        );
        picks.push(row);
        row += 1;
    }
    let mut total = format!(
        "so far: {:.1}M tokens, {:.1}M cache reads, {}",
        tokens_total as f64 / 1_000_000.0,
        cache_total as f64 / 1_000_000.0,
        if priced { format!("${cost_total:.2}") } else { "no prices set".to_string() }
    );
    if unknown > 0 {
        // The total is what is known: the unknown parts are named (F5).
        total.push_str(&format!(" (known so far; {unknown} unknown)"));
    }
    let rest = vec![Line::styled(total, Style::bold())];
    for line in &rest {
        canvas.put_line(0, row, line);
        row += 1;
    }
    picks
}

/// The tasks of the Time & tokens view, in the order of its rows: the tasks
/// that ran, lane by lane.
pub fn token_tasks(project: &ProjectBoard) -> Vec<(&str, &TaskBoard)> {
    let mut tasks = Vec::new();
    for lane in &project.lanes {
        for task in &lane.tasks {
            if task.actual_min.is_some() {
                tasks.push((lane.lane.as_str(), task));
            }
        }
    }
    tasks
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
    canvas.put_line(
        0,
        top,
        &Line::styled("LANES  (Enter: what this lane did and has left)", Style::bold()),
    );
    let mut picks = Vec::new();
    let mut row = top + 1;
    let width = lane_width(lane_order(project), 5, 0);
    for (index, lane) in project.lanes.iter().enumerate() {
        let picked = index == screen.selected;
        let current = current_task(lane);
        let (middle, middle_style) = match (&lane.idle_text, current) {
            // A lane that helps on the captain's instruction is shown as
            // helping, not as idle (T-323).
            (_, None) if lane.helping.is_some() => (
                lane.helping.clone().unwrap_or_default(),
                Style::fg(Color::Acc),
            ),
            (Some(reason), _) => (format!("idle: {reason}"), Style::fg(Color::Bad)),
            (None, Some(task)) => (
                format!("{} {}", task.id, task.title.chars().take(32).collect::<String>()),
                Style::PLAIN,
            ),
            (None, None) => ("no current task".to_string(), Style::fg(Color::Dim)),
        };
        // The time of the current task against its estimate: a bar, yellow
        // once the task is over its estimate.
        let (measures, measures_style) = match current {
            Some(task) => match (task.actual_min, task.est_min) {
                (Some(actual), Some(estimate)) if estimate > 0 => (
                    format!("{} {actual}/{estimate}m", bar(actual, estimate, 10)),
                    if actual > estimate { Style::fg(Color::Warn) } else { Style::PLAIN },
                ),
                _ => (String::new(), Style::PLAIN),
            },
            None => (String::new(), Style::PLAIN),
        };
        canvas.put_line(
            0,
            row,
            &mark(picked)
                .push(" ", Style::PLAIN)
                .push(
                    format!("{:<width$}", lane.lane),
                    lane_style(project, &lane.lane).and_bold(),
                )
                .push(" ", Style::PLAIN)
                .push(format!("{middle:<44}"), middle_style)
                .push(" ", Style::PLAIN)
                .push(format!("{measures:>20}"), measures_style)
                .push(
                    format!(
                        "  done {} · left {} ({}m)",
                        lane.done_count, lane.left_count, lane.left_min
                    ),
                    Style::fg(Color::Dim),
                ),
        );
        picks.push(row);
        row += 1;
    }
    let mut rest = Vec::new();
    // The overlaps with the integration branch (T-188): information lines.
    // They never wait for the captain.
    if !project.overlaps.is_empty() {
        rest.push(Line::new());
        for overlap in &project.overlaps {
            rest.push(Line::plain(crate::overlap_line(overlap)));
        }
    }
    let looks = needs_a_look(project);
    rest.push(Line::styled(
        format!("NEEDS A LOOK ({})", looks.len()),
        Style::fg(Color::Warn).and_bold(),
    ));
    for (task, lane, kind, why) in &looks {
        rest.push(
            Line::styled(format!("  {task} {lane:<width$} {kind:<14} "), Style::fg(Color::Warn))
                .push(why.clone(), Style::PLAIN),
        );
    }
    rest.push(Line::new());
    rest.push(Line::styled("QUOTA", Style::bold()));
    for reading in &project.quota {
        // A reading older than half an hour says its age (T-383).
        let stale = now - reading.read_at;
        let stale = (stale > 30 * 60).then(|| format!("  (read {} min ago)", stale / 60));
        for window in &reading.windows {
            // A reset today is a clock time; a later one carries its day.
            let reset = window
                .resets_at
                .map(|reset| {
                    if reset - now <= 24 * 3600 {
                        crate::idle::clock(reset)
                    } else {
                        crate::idle::day_clock(reset)
                    }
                })
                .unwrap_or_else(|| "-".into());
            rest.push(Line::styled(
                format!(
                    "  {:<12}{:<6}{} {:>3}% used  resets {reset}{}",
                    reading.provider,
                    window.name,
                    bar(window.used_percent as i64, 100, 14),
                    window.used_percent.round() as i64,
                    stale.as_deref().unwrap_or("")
                ),
                Style::fg(quota_color(window.used_percent)),
            ));
        }
    }
    rest.push(Line::new());
    rest.push(Line::styled("DONE SENTENCE", Style::bold()));
    // The store keeps the clauses of the done sentence, not whether each one
    // is met: every clause is an open one (the prototype's grey `·`).
    for clause in done_clauses(project) {
        rest.push(Line::styled(format!("  · {clause}"), Style::fg(Color::Dim)));
    }
    for line in &rest {
        canvas.put_line(0, row, line);
        row += 1;
    }
    picks
}

/// The task the lane works on now.
fn current_task(lane: &LaneBoard) -> Option<&TaskBoard> {
    lane.tasks
        .iter()
        .find(|task| task.state == gear_model::TaskState::Running)
        .or_else(|| lane.tasks.iter().find(|task| task.state == gear_model::TaskState::Waiting))
        .or_else(|| {
            lane.tasks
                .iter()
                .find(|task| task.state == gear_model::TaskState::Verifying)
        })
}

/// The things the captain should look at: a task over its estimate, a task
/// that was reworked, and a lane that has much more work left than the idle
/// ones beside it.
fn needs_a_look(project: &ProjectBoard) -> Vec<(String, String, String, String)> {
    let mut looks = Vec::new();
    for lane in &project.lanes {
        for task in &lane.tasks {
            if let (Some(actual), Some(estimate)) = (task.actual_min, task.est_min) {
                if estimate > 0 && actual > estimate {
                    looks.push((
                        task.id.clone(),
                        lane.lane.clone(),
                        "over estimate".into(),
                        format!("{actual}/{estimate}m"),
                    ));
                }
            }
            if task.rounds > 1 {
                looks.push((
                    task.id.clone(),
                    lane.lane.clone(),
                    "rework".into(),
                    format!("{} rounds", task.rounds),
                ));
            }
        }
    }
    // A lane with much more work left than the idle ones beside it.
    let idle: Vec<&LaneBoard> = project
        .lanes
        .iter()
        .filter(|lane| lane.idle.is_some())
        .collect();
    if let Some(busy) = project.lanes.iter().max_by_key(|lane| lane.left_min) {
        if busy.left_min > 0 && !idle.iter().any(|lane| lane.lane == busy.lane) {
            let others: Vec<String> = idle
                .iter()
                .map(|lane| format!("{} {}m", lane.lane, lane.left_min))
                .collect();
            looks.push((
                "-".into(),
                busy.lane.clone(),
                "overloaded".into(),
                format!(
                    "{} has {} tasks ({}m) left, {}",
                    busy.lane,
                    busy.left_count,
                    busy.left_min,
                    if others.is_empty() {
                        "the others are idle".to_string()
                    } else {
                        others.join(", ")
                    }
                ),
            ));
        }
    }
    looks
}

/// The clauses of the done sentence of the voyage.
fn done_clauses(project: &ProjectBoard) -> Vec<String> {
    let Some(voyage) = project.voyages.first() else {
        return Vec::new();
    };
    let Some(done) = voyage.done.as_deref() else {
        return Vec::new();
    };
    done.split(';')
        .map(|clause| clause.trim().to_string())
        .filter(|clause| !clause.is_empty())
        .collect()
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
    styled_lane_panel(lane, tasks, Color::Dim).into_iter().map(|line| line.text).collect()
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
pub fn styled_lane_panel(
    lane: &LaneBoard,
    tasks: &[graph::GraphTask],
    color: Color,
) -> Vec<Line> {
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
    for task in mine().filter(|task| matches!(task.stage, graph::Stage::Deps | graph::Stage::Queued))
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
