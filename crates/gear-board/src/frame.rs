//! The frame of the new board (design §8.1): the title, the time and tasks
//! bars, the four tabs, the views and the overlay panel.
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
use crate::{Board, LaneBoard, ProjectBoard, TaskBoard};

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

/// The row of the tab row (counted from zero).
pub const TAB_ROW: usize = 3;
/// The row of the view's heading (counted from zero). The first row the user
/// can pick is the one after it.
pub const VIEW_ROW: usize = 5;
/// The first row the user can pick (counted from zero).
pub const FIRST_ROW: usize = VIEW_ROW + 1;

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
    header(&mut canvas, board, screen.tab);

    // The view paints on a tall canvas, the screen's width: the prototype's
    // views put their texts at fixed columns, a later one over an earlier one
    // (README change 13). The screen copies the part the user looks at.
    let top = VIEW_ROW;
    let mut tall = Canvas::new(width, 1000);
    let picks = view(board, screen, &mut tall, top);
    // The selected row stays on the screen: the rows above it scroll away
    // (F8). The offset is the one the mouse hit testing uses.
    let offset = offset_of(&picks, screen.selected, top, height);
    let last = height.saturating_sub(2);
    // The footer's row and the row above it stay the frame's.
    canvas.copy_rows(&tall, top + offset, top, last.saturating_sub(top));
    // The selected row is lit across the width, in one colour (README change
    // 13).
    if let Some(row) = picks.get(screen.selected).map(|row| row.saturating_sub(offset)) {
        if row >= top && row < last {
            canvas.light_row(row);
        }
    }
    canvas.put_styled(1, height - 1, FOOTER, Style::fg(Color::Dim));
    if let Some(panel) = panel {
        draw_panel(&mut canvas, panel);
    }
    canvas
}

/// The offset of the first visible row of a view (the viewport, F8).
///
/// The rows above it are scrolled away, so the mouse hit testing adds the
/// same offset to the row the user clicked. The offset is zero while the
/// selected pick's row is above the footer's spacer, else the smallest offset
/// that shows it.
pub fn offset_of(picks: &[usize], selected: usize, top: usize, height: usize) -> usize {
    let last = height.saturating_sub(2);
    let Some(row) = picks.get(selected).copied() else {
        return 0;
    };
    if row < last {
        0
    } else {
        (row - last + 1).min(row.saturating_sub(top))
    }
}

/// The head of the board: the title, the time and tasks bars and the tabs.
///
/// The rows are the prototype's: the title, the time, the tasks, the tabs. A
/// warning (the engine is down) never moves the tab row.
fn header(canvas: &mut Canvas, board: &Board, tab: Tab) {
    let project = board.projects.first();
    let voyage = project.and_then(|project| project.voyages.first());

    // Row 0: the title and the clock.
    let mut title = match (project, voyage) {
        (Some(project), Some(voyage)) => format!(
            "GEAR · {} · {}: {}",
            project.project, voyage.milestone, voyage.title
        ),
        (Some(project), None) => format!("GEAR · {}", project.project),
        _ => "GEAR".to_string(),
    };
    // A board opened without a project holds the session's projects: the
    // title says that the others are one key away (T-317).
    if board.projects.len() > 1 {
        title.push_str(&format!(
            " · p: next of {} projects",
            board.projects.len()
        ));
    }
    canvas.put_styled(0, 0, &title, Style::bold());
    if board.engine == crate::EngineState::Down {
        // The engine is down: the next `gear` call starts it. It says so on
        // the title's row, so that the tab row stays where it is.
        canvas.put_styled(
            title.chars().count(),
            0,
            "  ! engine down — the next gear call starts it",
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

    // Row 1: the time bar and the projection. Both are green while the
    // projected finish is within the eta, red once it is past it.
    let elapsed = voyage.and_then(|voyage| voyage.elapsed_min);
    let eta_minutes = voyage
        .and_then(|voyage| voyage.eta.as_deref())
        .and_then(duration_minutes);
    let projected_min = voyage.and_then(|voyage| voyage.projected_min);
    let pace = match (projected_min, eta_minutes) {
        (Some(projected), Some(eta)) if projected <= eta => Style::fg(Color::Ok),
        (Some(_), Some(_)) => Style::fg(Color::Bad),
        _ => Style::PLAIN,
    };
    let time = match (elapsed, eta_minutes) {
        (Some(elapsed), Some(eta)) => Line::plain("time   ")
            .push(bar(elapsed, eta, 24), pace)
            .push(format!(" {} of {}", hours(elapsed), hours(eta)), Style::PLAIN),
        _ => Line::plain("time   the voyage's time is unknown"),
    };
    canvas.put_line(0, 1, &time);
    if let Some(voyage) = voyage {
        let projected = match voyage.projected_min {
            Some(minutes) => format!(
                "projected finish {} (slowest lane: {})",
                hours(minutes),
                voyage
                    .slowest_lane
                    .as_ref()
                    .map(|(lane, _)| lane.clone())
                    .unwrap_or_else(|| "-".into())
            ),
            None => "projected finish unknown".to_string(),
        };
        canvas.put_styled(48, 1, &projected, pace);
    }

    // Row 2: the tasks bar and the done sentence.
    if let Some(project) = project {
        let totals = &project.totals;
        let left = project
            .voyages
            .first()
            .and_then(|voyage| voyage.remaining_estimate_min)
            .unwrap_or(0);
        canvas.put(
            0,
            2,
            &format!(
                "tasks  {} {}/{} done, {} of estimates left",
                bar(totals.done as i64, totals.tasks as i64, 24),
                totals.done,
                totals.tasks,
                hours(left)
            ),
        );
        let clauses = done_clauses(project).len();
        canvas.put_styled(
            72,
            2,
            &format!("done sentence {clauses} clauses"),
            Style::fg(Color::Warn),
        );
    }

    // Row 3: the four tabs; the open one is in reverse video, the others
    // dim.
    let mut x = 1;
    for tab_in_row in Tab::all() {
        let label = tab_in_row.label();
        if tab_in_row == tab {
            canvas.put(x, TAB_ROW, &label);
            canvas.reverse(TAB_ROW, x, label.chars().count());
        } else {
            canvas.put_styled(x, TAB_ROW, &label, Style::fg(Color::Dim));
        }
        x += label.chars().count() + 1;
    }
}

/// The columns of every tab on the tab row: its first and its last column.
pub fn tab_ranges() -> Vec<(u16, u16, Tab)> {
    let mut ranges = Vec::new();
    let mut x = 1u16;
    for tab in Tab::all() {
        let width = tab.label().chars().count() as u16;
        ranges.push((x, x + width - 1, tab));
        x += width + 1;
    }
    ranges
}

/// The tab at a column of the tab row.
pub fn tab_at(x: u16) -> Option<Tab> {
    tab_ranges()
        .into_iter()
        .find(|(first, last, _)| x >= *first && x <= *last)
        .map(|(_, _, tab)| tab)
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
        Tab::Tasks => view_tasks(project, screen, canvas, top),
        Tab::Cost => view_tokens(project, screen, canvas, top),
        Tab::Log => view_decisions(project, screen, canvas, top),
    }
}

/// The second view: the work as a graph, painted from the graph's own lines.
fn view_tasks(
    project: &ProjectBoard,
    screen: &Screen,
    canvas: &mut Canvas,
    top: usize,
) -> Vec<usize> {
    // The graph's own highlighter still knows the selection (its dependency
    // marks); the reversed cells it draws are the row the canvas lights on its
    // own, one colour across the width (README change 13).
    let lines = graph::styled_lines(&project.graph, &project.graph_lanes, Some(screen.selected));
    let count = project.graph.len();
    let mut picks = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let row = top + index;
        canvas.put_line(0, row, line);
        // The first line is the head of the columns; then one line per task.
        if (1..=count).contains(&index) {
            picks.push(row);
        }
    }
    picks
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
fn draw_panel(canvas: &mut Canvas, panel: &[Line]) {
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
    let top = VIEW_ROW;
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

/// A duration of the plan (`12h`, `30m`) in minutes.
fn duration_minutes(text: &str) -> Option<i64> {
    let text = text.trim();
    let number: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
    let value: i64 = number.parse().ok()?;
    let unit = text[number.len()..].trim().to_ascii_lowercase();
    match unit.as_str() {
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(value * 60),
        "m" | "min" | "mins" | "minute" | "minutes" => Some(value),
        _ => None,
    }
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
