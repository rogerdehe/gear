//! The frame of the new board (design §8.1): the title, the time and tasks
//! bars, the five tabs, the views and the overlay panel.
//!
//! The prototype of `ops/plan/m4-board` is the contract for the layout, the
//! views, the keys and the wording; the data comes from the store.

use crate::graph;
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
    /// The label of the tab on the tab row.
    pub fn label(self) -> String {
        tab_label(self)
    }
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

/// The row of the tab row: a click on it picks a tab.
pub const TAB_ROW: usize = 3;
/// The first row of a view: a click on it picks a row.
pub const VIEW_ROW: usize = 5;
/// The width the frame is drawn for.
pub const WIDTH: usize = 120;

/// The whole screen: the frame, the view and the panel (when one is open).
pub fn frame(board: &Board, tab: Tab, selected: usize, panel: Option<&[String]>) -> String {
    let mut lines = header(board);
    lines.push(String::new());
    lines.extend(view(board, tab, selected));
    lines.push(String::new());
    lines.push(FOOTER.to_string());
    if let Some(panel) = panel {
        draw_panel(&mut lines, panel);
    }
    lines.join("\n")
}

/// The header of the board: the title, the time bar, the tasks bar and the
/// tabs.
fn header(board: &Board) -> Vec<String> {
    let project = board.projects.first();
    let voyage = project.and_then(|project| project.voyages.first());
    let title = match (project, voyage) {
        (Some(project), Some(voyage)) => format!(
            "GEAR · {} · {}: {}",
            project.project, voyage.milestone, voyage.title
        ),
        (Some(project), None) => format!("GEAR · {}", project.project),
        _ => "GEAR".to_string(),
    };
    let mut lines = vec![pad(&title, 0)];
    if board.engine == crate::EngineState::Down {
        // The engine is down: the next `gear` call starts it (the old frame
        // says so, and the user has to know).
        lines.push(crate::ENGINE_DOWN.to_string());
    }
    // The time bar: how much of the voyage is gone, and how much is left.
    let elapsed = voyage.and_then(|voyage| voyage.elapsed_min);
    let eta = voyage.and_then(|voyage| voyage.eta.as_deref());
    let measured = voyage.map(|voyage| voyage.measured_actual_min).unwrap_or(0);
    let estimate = voyage.map(|voyage| voyage.measured_estimate_min).unwrap_or(0);
    let time = match elapsed {
        Some(elapsed) => format!(
            "time   {} {} of {}",
            bar(measured, estimate, 24),
            hours(elapsed),
            eta.unwrap_or("-")
        ),
        None => format!("time   {} measured, no estimate", hours(measured)),
    };
    lines.push(pad(&time, 0));
    if let Some(voyage) = voyage {
        let projected = match voyage.projected_min {
            Some(minutes) => format!("projected finish {} (slowest lane: {})", hours(minutes), voyage.slowest.as_ref().map(|(id, _)| id.clone()).unwrap_or_else(|| "-".into())),
            None => "projected finish unknown".to_string(),
        };
        lines.push(pad(&projected, 48));
    }
    // The tasks bar: what is done and what is left.
    if let Some(project) = project {
        let totals = &project.totals;
        let left = project.voyages.first().and_then(|voyage| voyage.remaining_estimate_min).unwrap_or(0);
        let tasks = format!(
            "tasks  {} {}/{} done, {} of estimates left",
            bar(totals.done as i64, totals.tasks as i64, 24),
            totals.done,
            totals.tasks,
            hours(left)
        );
        lines.push(pad(&tasks, 0));
    }
    // The tabs: the number and the title of each, as the prototype draws them.
    let mut row = String::new();
    for (index, tab) in Tab::all().iter().enumerate() {
        if index > 0 {
            row.push(' ');
        }
        row.push_str(&tab.label());
    }
    lines.push(row);
    lines
}

/// The label of a tab on the tab row: ` 1 Now `.
pub fn tab_label(tab: Tab) -> String {
    format!(" {} {} ", tab.number(), tab.title())
}

/// The columns of every tab on the tab row: its first and its last column.
pub fn tab_ranges() -> Vec<(u16, u16, Tab)> {
    let mut ranges = Vec::new();
    let mut x = 1u16;
    for tab in Tab::all() {
        let width = tab_label(tab).chars().count() as u16;
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

/// The footer of the board: the keys.
pub const FOOTER: &str =
    "←→ / Tab / 1-5 / click: views · ↑↓ select · Enter: a lane's work / a task's rounds · q quit";

/// The view of the board.
pub fn view(board: &Board, tab: Tab, selected: usize) -> Vec<String> {
    let Some(project) = board.projects.first() else {
        return vec!["no project in the store".to_string()];
    };
    match tab {
        Tab::Now => view_now(project, selected),
        Tab::Graph => graph::lines(&project.graph, &project.graph_lanes, Some(selected)),
        Tab::Workload | Tab::Decisions | Tab::Tokens => vec![format!(
            "{}: the next task",
            tab.title()
        )],
    }
}

/// The first view: the lanes, what needs a look, the quota and the done
/// sentence.
fn view_now(project: &ProjectBoard, selected: usize) -> Vec<String> {
    let mut lines = vec!["LANES  (Enter: what this lane did and has left)".to_string()];
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
                    format!("{}/{estimate}m", actual)
                }
                _ => String::new(),
            },
            None => String::new(),
        };
        lines.push(format!(
            "{mark} {:<5} {:<44} {:>9}  done {} · left {} ({}m)",
            lane.lane,
            middle,
            measures,
            lane.done_count,
            lane.left_count,
            lane.left_min
        ));
    }
    lines.push(String::new());
    let looks = needs_a_look(project);
    lines.push(format!("NEEDS A LOOK ({})", looks.len()));
    for (task, lane, kind, why) in &looks {
        lines.push(format!("  {task} {lane:<5} {kind:<14} {why}"));
    }
    lines.push(String::new());
    lines.push("QUOTA".to_string());
    for reading in &project.quota {
        for window in &reading.windows {
            let reset = window
                .resets_at
                .map(crate::idle::clock)
                .unwrap_or_else(|| "-".into());
            lines.push(format!(
                "  {:<8}{:<6}{} {:>3}% used  resets {reset}",
                reading.provider,
                window.name,
                bar(window.used_percent as i64, 100, 14),
                window.used_percent.round() as i64
            ));
        }
    }
    lines.push(String::new());
    lines.push("DONE SENTENCE".to_string());
    for clause in done_clauses(project) {
        lines.push(format!("  · {clause}"));
    }
    lines
}

/// The task the lane works on now.
fn current_task(lane: &LaneBoard) -> Option<&TaskBoard> {
    lane.tasks
        .iter()
        .find(|task| task.state == gear_model::TaskState::Running)
        .or_else(|| lane.tasks.iter().find(|task| task.state == gear_model::TaskState::Waiting))
        .or_else(|| lane.tasks.iter().find(|task| task.state == gear_model::TaskState::Verifying))
}

/// The things the captain should look at: a task over its estimate, a task
/// that was reworked, and a lane that has more work than the others while
/// they are idle.
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
    let idle: Vec<&LaneBoard> = project.lanes.iter().filter(|lane| lane.idle.is_some()).collect();
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
/// prototype draws it.
fn draw_panel(lines: &mut Vec<String>, panel: &[String]) {
    let width = panel
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(20)
        .max(20)
        + 4;
    let top = VIEW_ROW.min(lines.len());
    let bottom = (top + panel.len() + 1).min(lines.len());
    let left = WIDTH.saturating_sub(width + 1);
    let mut index = top;
    // The box.
    if index < lines.len() {
        let line = pad_to(&lines[index], left);
        lines[index] = format!("{line}┌{}┐", "─".repeat(width));
    }
    index += 1;
    for row in panel {
        if index >= bottom {
            break;
        }
        let line = pad_to(&lines[index], left);
        lines[index] = format!(
            "{line}│ {:<width$} │",
            row.chars().take(width - 2).collect::<String>()
        );
        index += 1;
    }
    if index < lines.len() {
        let line = pad_to(&lines[index], left);
        lines[index] = format!("{line}└{}┘", "─".repeat(width));
    }
}

/// A line of at least `width` characters.
fn pad_to(line: &str, width: usize) -> String {
    let mut out = line.to_string();
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

/// A line with something at a column (the prototype puts the time and the
/// projection side by side).
fn pad(line: &str, column: usize) -> String {
    let mut out = line.to_string();
    while out.chars().count() < column {
        out.push(' ');
    }
    out
}

/// A bar of `width` cells: how much of `total` is `part`.
pub fn bar(part: i64, total: i64, width: usize) -> String {
    if total <= 0 {
        return "░".repeat(width);
    }
    let filled = ((part as f64 / total as f64) * width as f64).round().max(0.0) as usize;
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
                if task.state == gear_model::TaskState::Running { "◉" } else { "◎" },
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
        if task.state.is_terminal() {
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
