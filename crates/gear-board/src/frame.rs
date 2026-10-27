//! The frame of the new board (design §8.1): the title, the time and tasks
//! bars, the five tabs, the views and the overlay panel.
//!
//! The prototype of `ops/plan/m4-board` is the contract for the layout, the
//! views, the keys and the wording; the data comes from the store.
//!
//! The frame is drawn on a **canvas** of the size of the terminal: every row
//! is cut at the right edge, the footer keeps the last row, and the panel is
//! drawn over the view (as the prototype draws its window over the board).

use crate::graph;
use crate::voyage::LaneProjection;
use crate::idle::IdleReason;
use crate::{Board, LaneBoard, ProjectBoard, TaskBoard};

/// The five views of the board: the tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// The lanes, what they do now and what they have left.
    Now,
    /// The work as a graph.
    Graph,
    /// The work done and left, per lane.
    Workload,
    /// The decisions of the captain.
    Decisions,
    /// The time and the tokens, per task.
    Tokens,
}

impl Tab {
    /// The number of the tab (the keys 1 to 5).
    pub fn number(self) -> u8 {
        match self {
            Tab::Now => 1,
            Tab::Graph => 2,
            Tab::Workload => 3,
            Tab::Decisions => 4,
            Tab::Tokens => 5,
        }
    }

    /// The title of the tab.
    pub fn title(self) -> &'static str {
        match self {
            Tab::Now => "Now",
            Tab::Graph => "Graph",
            Tab::Workload => "Workload",
            Tab::Decisions => "Decisions",
            Tab::Tokens => "Time & tokens",
        }
    }

    /// The label of the tab on the tab row: ` 1 Now `.
    pub fn label(self) -> String {
        format!(" {} {} ", self.number(), self.title())
    }

    /// The tab of a number (the keys 1 to 5).
    pub fn of_number(number: u8) -> Option<Tab> {
        match number {
            1 => Some(Tab::Now),
            2 => Some(Tab::Graph),
            3 => Some(Tab::Workload),
            4 => Some(Tab::Decisions),
            5 => Some(Tab::Tokens),
            _ => None,
        }
    }

    /// The next tab (the right arrow).
    pub fn next(self) -> Tab {
        match self {
            Tab::Now => Tab::Graph,
            Tab::Graph => Tab::Workload,
            Tab::Workload => Tab::Decisions,
            Tab::Decisions => Tab::Tokens,
            Tab::Tokens => Tab::Now,
        }
    }

    /// The tab before (the left arrow).
    pub fn previous(self) -> Tab {
        match self {
            Tab::Now => Tab::Tokens,
            Tab::Graph => Tab::Now,
            Tab::Workload => Tab::Graph,
            Tab::Decisions => Tab::Workload,
            Tab::Tokens => Tab::Decisions,
        }
    }

    /// Every tab, in the order of the row.
    pub fn all() -> [Tab; 5] {
        [Tab::Now, Tab::Graph, Tab::Workload, Tab::Decisions, Tab::Tokens]
    }
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
pub const FOOTER: &str = "←→ / Tab / 1-5: views · ↑↓ select · Enter: a lane or a task · q quit";

/// A screen of characters: the frame, the view and the panel are drawn on it.
pub struct Canvas {
    width: usize,
    height: usize,
    cells: Vec<char>,
    /// The parts of the screen the terminal shows in reverse video (the tab
    /// that is open).
    reversed: Vec<(usize, usize, usize)>,
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
            reversed: Vec::new(),
        }
    }

    /// Write a text at a column and a row; the text is cut at the edge.
    pub fn put(&mut self, x: usize, y: usize, text: &str) {
        if y >= self.height {
            return;
        }
        for (offset, character) in text.chars().enumerate() {
            let column = x + offset;
            if column >= self.width {
                break;
            }
            self.cells[y * self.width + column] = character;
        }
    }

    /// Show a part of the screen in reverse video (the tab that is open).
    pub fn reverse(&mut self, y: usize, x: usize, len: usize) {
        if y < self.height && x < self.width {
            self.reversed.push((y, x, len.min(self.width - x)));
        }
    }

    /// The screen as text: the rows, each cut at the edge.
    ///
    /// The parts in reverse video (the tab that is open) are wrapped in the
    /// terminal's reverse video, as the prototype shows them.
    pub fn text(&self) -> String {
        let mut rows: Vec<String> = Vec::with_capacity(self.height);
        for row in 0..self.height {
            rows.push(self.row_text(row));
        }
        rows.join("\n")
    }

    /// One row of the screen.
    fn row_text(&self, row: usize) -> String {
        let cells = &self.cells[row * self.width..(row + 1) * self.width];
        let reversed = |column: usize| {
            self.reversed
                .iter()
                .any(|(r, x, len)| *r == row && column >= *x && column < *x + *len)
        };
        let mut out = String::new();
        let mut column = 0usize;
        while column < self.width {
            let start = column;
            let is_reversed = reversed(column);
            while column < self.width && reversed(column) == is_reversed {
                column += 1;
            }
            let text: String = cells[start..column].iter().collect();
            if is_reversed {
                out.push_str("\u{1b}[7m");
                out.push_str(&text);
                out.push_str("\u{1b}[0m");
            } else {
                out.push_str(&text);
            }
        }
        out.trim_end().to_string()
    }
}

/// The whole screen: the frame, the view and the panel.
pub fn frame(
    board: &Board,
    tab: Tab,
    selected: usize,
    panel: Option<&[String]>,
    size: (usize, usize),
) -> String {
    let (width, height) = size;
    let mut canvas = Canvas::new(width, height);
    header(&mut canvas, board, tab);

    let view = view(board, tab, selected);
    // The view's heading sits on its own row; the rows the user can pick come
    // after it, and the footer keeps the last row of the screen.
    canvas.put(0, VIEW_ROW, &view.heading);
    let first = FIRST_ROW;
    let last = height.saturating_sub(2);
    let visible = last.saturating_sub(first);
    // The selected row stays on the screen.
    let scroll = selected.saturating_sub(visible.saturating_sub(1));
    for (index, line) in view.rows.iter().enumerate() {
        let row = first + index;
        if row >= last {
            break;
        }
        let row = row.saturating_sub(scroll);
        if row < first {
            continue;
        }
        canvas.put(0, row, line);
    }
    for (index, line) in view.rest.iter().enumerate() {
        let row = first + view.rows.len() + index;
        if row >= last {
            break;
        }
        canvas.put(0, row, line);
    }
    canvas.put(0, height - 1, FOOTER);
    if let Some(panel) = panel {
        draw_panel(&mut canvas, panel);
    }
    canvas.text()
}

/// The head of the board: the title, the time and tasks bars and the tabs.
///
/// The rows are the prototype's: the title, the time, the tasks, the tabs. A
/// warning (the engine is down) never moves the tab row.
fn header(canvas: &mut Canvas, board: &Board, tab: Tab) {
    let project = board.projects.first();
    let voyage = project.and_then(|project| project.voyages.first());

    // Row 0: the title and the clock.
    let title = match (project, voyage) {
        (Some(project), Some(voyage)) => format!(
            "GEAR · {} · {}: {}",
            project.project, voyage.milestone, voyage.title
        ),
        (Some(project), None) => format!("GEAR · {}", project.project),
        _ => "GEAR".to_string(),
    };
    canvas.put(0, 0, &title);
    if board.engine == crate::EngineState::Down {
        // The engine is down: the next `gear` call starts it. It says so on
        // the title's row, so that the tab row stays where it is.
        canvas.put(0, 0, &format!("{title}  ! engine down — the next gear call starts it"));
    }
    let clock = clock_now();
    canvas.put(canvas.width.saturating_sub(clock.chars().count()), 0, &clock);

    // Row 1: the time bar and the projection.
    let elapsed = voyage.and_then(|voyage| voyage.elapsed_min);
    let eta_minutes = voyage
        .and_then(|voyage| voyage.eta.as_deref())
        .and_then(duration_minutes);
    let time = match (elapsed, eta_minutes) {
        (Some(elapsed), Some(eta)) => format!(
            "time   {} {} of {}",
            bar(elapsed, eta, 24),
            hours(elapsed),
            hours(eta)
        ),
        _ => "time   the voyage's time is unknown".to_string(),
    };
    canvas.put(0, 1, &time);
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
        canvas.put(48, 1, &projected);
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
        canvas.put(72, 2, &format!("done sentence {clauses} clauses"));
    }

    // Row 3: the five tabs; the open one is in reverse video.
    let mut x = 1;
    for tab_in_row in Tab::all() {
        let label = tab_in_row.label();
        canvas.put(x, TAB_ROW, &label);
        if tab_in_row == tab {
            canvas.reverse(TAB_ROW, x, label.chars().count());
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

/// A view of the board: its heading, the rows the user can pick and the rest.
pub struct ViewRows {
    pub heading: String,
    pub rows: Vec<String>,
    pub rest: Vec<String>,
}

/// The view of the board.
pub fn view(board: &Board, tab: Tab, selected: usize) -> ViewRows {
    let Some(project) = board.projects.first() else {
        return ViewRows {
            heading: "no project in the store".to_string(),
            rows: Vec::new(),
            rest: Vec::new(),
        };
    };
    match tab {
        Tab::Now => view_now(project, selected),
        Tab::Graph => {
            let lines = graph::lines(&project.graph, &project.graph_lanes, Some(selected));
            let count = project.graph.len();
            let heading = lines.first().cloned().unwrap_or_default();
            let rows = lines
                .get(1..=count)
                .map(|rows| rows.to_vec())
                .unwrap_or_default();
            let rest = lines.get(count + 1..).map(|rest| rest.to_vec()).unwrap_or_default();
            ViewRows { heading, rows, rest }
        }
        Tab::Workload => view_workload(project, selected),
        Tab::Decisions | Tab::Tokens => ViewRows {
            heading: format!("{}: the next task", tab.title()),
            rows: Vec::new(),
            rest: Vec::new(),
        },
    }
}

/// The third view: the work done and left, per lane, and the burn-up.
fn view_workload(project: &ProjectBoard, selected: usize) -> ViewRows {
    // The voyage the Workload view measures: the active one, else the latest
    // departed voyage that is finished (the measurements of a landed voyage
    // are still true, F5).
    let voyage = project.workload.as_ref();
    let mut rows = Vec::new();
    let mut done_total = 0;
    let mut left_total = 0;
    for (index, lane) in project.lanes.iter().enumerate() {
        // The lane's own measurements (the voyage report).
        let measured = voyage
            .and_then(|voyage| voyage.lanes.iter().find(|l| l.lane == lane.lane));
        let empty = LaneProjection {
            lane: lane.lane.clone(),
            done_min: 0,
            done_estimate_min: 0,
            left_min: 0,
            left_known: true,
            done_count: 0,
            left_count: 0,
            pace: None,
            projected_min: None,
        };
        let lane = measured.unwrap_or(&empty);
        let mark = if index == selected { "›" } else { " " };
        // The minutes of estimate (the prototype's view counts the estimates).
        done_total += lane.done_estimate_min;
        left_total += lane.left_min;
        let done_cells = (lane.done_estimate_min / 8).max(0) as usize;
        let left_cells = (lane.left_min / 8).max(0) as usize;
        rows.push(format!(
            "{mark} {:<5} {}{:<width$}done {} ({}m) · left {} ({}m)",
            lane.lane,
            "█".repeat(done_cells.min(14)),
            "░".repeat(left_cells.min(14)),
            lane.done_count,
            lane.done_estimate_min,
            lane.left_count,
            lane.left_min,
            width = 14usize.saturating_sub(done_cells.min(14))
        ));
    }
    let mut rest = Vec::new();
    let projected = voyage
        .and_then(|voyage| voyage.projected_min)
        .map(hours)
        .unwrap_or_else(|| "-".into());
    rest.push(format!(
        "all lanes: done {done_total}m · left {left_total}m · at the measured pace the last lane ends at {projected}"
    ));
    rest.push(String::new());
    rest.push("BURN-UP  (minutes of estimate merged, per voyage hour)".to_string());
    let burn = voyage.map(|voyage| voyage.burn_up.clone()).unwrap_or_default();
    if !burn.is_empty() {
        let top = burn.iter().copied().max().unwrap_or(0).max(done_total + left_total);
        for row in (1..=8).rev() {
            let level = top as f64 * row as f64 / 8.0;
            let line: String = burn
                .iter()
                .map(|minutes| {
                    if *minutes as f64 >= level {
                        " ██ "
                    } else {
                        "    "
                    }
                })
                .collect();
            rest.push(line.to_string());
        }
        rest.push(format!("    ─ total {}m", done_total + left_total));
        let labels: String = (0..burn.len()).map(|hour| format!(" h{hour:<2}")).collect();
        rest.push(labels);
    }
    ViewRows {
        heading: "WORK DONE AND LEFT, BY LANE  (minutes of estimate)".to_string(),
        rows,
        rest,
    }
}

/// The first view: the lanes, what needs a look, the quota and the done
/// sentence.
fn view_now(project: &ProjectBoard, selected: usize) -> ViewRows {
    let mut rows = Vec::new();
    for (index, lane) in project.lanes.iter().enumerate() {
        let mark = if index == selected { "›" } else { " " };
        let current = current_task(lane);
        let middle = match (&lane.idle_text, current) {
            (Some(reason), _) => format!("idle: {reason}"),
            (None, Some(task)) => format!(
                "{} {}",
                task.id,
                task.title.chars().take(32).collect::<String>()
            ),
            (None, None) => "no current task".to_string(),
        };
        let measures = match current {
            Some(task) => match (task.actual_min, task.est_min) {
                (Some(actual), Some(estimate)) if estimate > 0 => {
                    format!("{actual}/{estimate}m")
                }
                _ => String::new(),
            },
            None => String::new(),
        };
        rows.push(format!(
            "{mark} {:<5} {:<44} {:>9}  done {} · left {} ({}m)",
            lane.lane,
            middle,
            measures,
            lane.done_count,
            lane.left_count,
            lane.left_min
        ));
    }
    let mut rest = Vec::new();
    let looks = needs_a_look(project);
    rest.push(format!("NEEDS A LOOK ({})", looks.len()));
    for (task, lane, kind, why) in &looks {
        rest.push(format!("  {task} {lane:<5} {kind:<14} {why}"));
    }
    rest.push(String::new());
    rest.push("QUOTA".to_string());
    for reading in &project.quota {
        for window in &reading.windows {
            let reset = window
                .resets_at
                .map(crate::idle::clock)
                .unwrap_or_else(|| "-".into());
            rest.push(format!(
                "  {:<12}{:<6}{} {:>3}% used  resets {reset}",
                reading.provider,
                window.name,
                bar(window.used_percent as i64, 100, 14),
                window.used_percent.round() as i64
            ));
        }
    }
    rest.push(String::new());
    rest.push("DONE SENTENCE".to_string());
    for clause in done_clauses(project) {
        rest.push(format!("  · {clause}"));
    }
    ViewRows {
        heading: "LANES  (Enter: what this lane did and has left)".to_string(),
        rows,
        rest,
    }
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
fn draw_panel(canvas: &mut Canvas, panel: &[String]) {
    let width = panel
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(20)
        .max(20)
        .min(canvas.width.saturating_sub(4));
    if width < 8 {
        return;
    }
    let left = canvas.width.saturating_sub(width + 1);
    let top = VIEW_ROW;
    // The box: a top border, the rows, a bottom border.
    canvas.put(left, top, &format!("┌{}┐", "─".repeat(width)));
    for (index, row) in panel.iter().enumerate() {
        let line = top + 1 + index;
        if line + 1 >= canvas.height {
            break;
        }
        canvas.put(
            left,
            line,
            &format!("│ {:<width$} │", row.chars().take(width - 2).collect::<String>()),
        );
    }
    let bottom = (top + panel.len() + 1).min(canvas.height - 1);
    canvas.put(left, bottom, &format!("└{}┘", "─".repeat(width)));
}

/// The lane's panel: what it did, what it does now and what it has left.
pub fn lane_panel(lane: &LaneBoard) -> Vec<String> {
    let mut rows = vec![format!("lane {}", lane.lane), String::new(), "DONE".to_string()];
    for task in &lane.tasks {
        if task.state == gear_model::TaskState::Done {
            rows.push(format!(
                "  ● {} {:<30} {}/{}m",
                task.id,
                task.title.chars().take(30).collect::<String>(),
                task.actual_min.unwrap_or(0),
                task.est_min.unwrap_or(0)
            ));
        }
    }
    rows.push(String::new());
    rows.push("NOW".to_string());
    for task in &lane.tasks {
        if matches!(
            task.state,
            gear_model::TaskState::Running | gear_model::TaskState::Verifying
        ) {
            rows.push(format!(
                "  {} {} {:<30} {}",
                if task.state == gear_model::TaskState::Running {
                    "◉"
                } else {
                    "◎"
                },
                task.id,
                task.title.chars().take(30).collect::<String>(),
                task.state.as_str()
            ));
        }
    }
    rows.push(String::new());
    rows.push("LEFT, in order".to_string());
    let mut left = 0;
    let mut minutes = 0;
    for task in &lane.tasks {
        // The tasks that run or wait for their review are in NOW: the panel
        // shows them once (the prototype's LEFT lists the queued work only).
        if task.state.is_terminal()
            || matches!(
                task.state,
                gear_model::TaskState::Running | gear_model::TaskState::Verifying
            )
        {
            continue;
        }
        left += 1;
        minutes += task.est_min.unwrap_or(0);
        rows.push(format!(
            "  ○ {} {:<30} {}m",
            task.id,
            task.title.chars().take(30).collect::<String>(),
            task.est_min.unwrap_or(0)
        ));
    }
    rows.push(format!("left: {left} tasks, {minutes}m of estimates"));
    rows
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

/// The clock of the board, as the user's clock sees it.
fn clock_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0);
    crate::idle::clock(now)
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
