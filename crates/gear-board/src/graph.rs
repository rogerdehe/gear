//! The graph view of the board (design §8.1, the prototype of
//! `ops/plan/m4-board`).
//!
//! Like `git log --graph`: `dev` on the left, a review column, one column per
//! lane, the newest work on top. A task that was merged into `dev` draws the
//! line of the merge, and the lanes it crosses are marked with `┼`. The
//! selected row lights up what it needs (`◂ needed by`) and what waits for
//! it (`▸ waits for`).
//!
//! The task panel (what `Enter` opens on a row) shows the rounds of the task,
//! what it depends on and what needs it.

use gear_model::{GearResult, TaskState, Ts};
use gear_store::{AttemptRow, AttemptTimes, Tx};

use crate::state_word;
use crate::style::{lane_color, lane_width, Color, Line, Style};

/// The attempts of a task and the times of each of them.
struct History {
    attempts: Vec<AttemptRow>,
    times: Vec<(i64, AttemptTimes)>,
}

impl History {
    /// The history of a task: its attempts and the time of every step.
    fn of(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<History> {
        Ok(History {
            attempts: tx.attempts_of_task(project, task)?,
            times: tx.attempt_times_of_task(project, task)?,
        })
    }

    /// The times of one attempt.
    fn times_of(&self, n: i64) -> AttemptTimes {
        self.times
            .iter()
            .find(|(attempt, _)| *attempt == n)
            .map(|(_, times)| *times)
            .unwrap_or_default()
    }

    /// When the task started, when it was handed in and when it was merged.
    fn span(&self) -> (Option<Ts>, Option<Ts>, Option<Ts>) {
        let start = self.times.iter().filter_map(|(_, times)| times.started).min();
        let handed_in = self.times.iter().filter_map(|(_, times)| times.handed_in).max();
        let merged = self.times.iter().filter_map(|(_, times)| times.merged).max();
        (start, handed_in, merged)
    }
}

/// The stage of a task in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphStage {
    /// The task is queued: the lane has not started it.
    Queued,
    /// The lane works on it.
    Working,
    /// The task is in review.
    Review,
    /// The task is accepted and the merge is on its way.
    Merging,
    /// The task is merged into `dev`.
    Done,
}

impl GraphStage {
    /// The symbol of the stage in the lane's column.
    pub fn symbol(self) -> &'static str {
        match self {
            GraphStage::Queued => "○",
            GraphStage::Working => "◉",
            GraphStage::Review | GraphStage::Merging => "◎",
            GraphStage::Done => "●",
        }
    }

    /// The word of the stage (the panel).
    pub fn word(self) -> &'static str {
        match self {
            GraphStage::Queued => "queued",
            GraphStage::Working => "working",
            GraphStage::Review => "review",
            GraphStage::Merging => "merging",
            GraphStage::Done => "done",
        }
    }
}

/// One task of the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphTask {
    pub id: String,
    /// The lane that owns the task.
    pub lane: String,
    pub stage: GraphStage,
    /// The goal of the task (the contract's goal), as the title.
    pub title: String,
    /// The estimate of the task in minutes.
    pub est_min: Option<i64>,
    /// The time the task took, in minutes.
    pub actual_min: Option<i64>,
    /// How many rounds the task had; more than one means it was reworked.
    pub rounds: usize,
    /// The tasks this one depends on.
    pub deps: Vec<String>,
    /// When the task was merged into `dev`.
    pub finished: Option<Ts>,
}

/// The tasks of the graph, in the order the prototype draws them: the queued
/// first, then the work, then the reviews, then the merged ones (the newest
/// first).
pub fn tasks(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Vec<GraphTask>> {
    let rows = tx.tasks_of_project(project)?;
    let mut tasks = Vec::new();
    for row in &rows {
        if row.state == TaskState::Draft {
            // A task that is not in the plan yet is not in the flow.
            continue;
        }
        if row.state == TaskState::Failed || row.state == TaskState::Cancelled {
            // A task that ended without a result is not a step of the flow.
            continue;
        }
        let history = History::of(tx, project, &row.id)?;
        let title = tx
            .get_contract(project, &row.id, row.contract_version)?
            .map(|contract| contract.goal)
            .unwrap_or_default();
        let (actual, finished) = actual_of(&history, now);
        tasks.push(GraphTask {
            id: row.id.clone(),
            lane: row.lane.clone().unwrap_or_default(),
            stage: stage_of(tx, project, row)?,
            title,
            est_min: row.estimate_min,
            actual_min: actual,
            rounds: history.attempts.len(),
            deps: row.deps.clone(),
            finished,
        });
    }
    Ok(order(tasks))
}

/// The order of the prototype: queued, working, the work that is in review
/// (one group, in the order of the plan), then the merged ones with the
/// newest first.
fn order(mut tasks: Vec<GraphTask>) -> Vec<GraphTask> {
    let stage = |task: &GraphTask| task.stage;
    let mut done: Vec<GraphTask> = Vec::new();
    let mut rest: Vec<GraphTask> = Vec::new();
    for task in tasks.drain(..) {
        if task.stage == GraphStage::Done {
            done.push(task);
        } else {
            rest.push(task);
        }
    }
    done.sort_by_key(|task| task.finished.unwrap_or(0));
    done.reverse();
    let mut ordered: Vec<GraphTask> = Vec::new();
    for wanted in [GraphStage::Queued, GraphStage::Working] {
        ordered.extend(rest.iter().filter(|task| stage(task) == wanted).cloned());
    }
    // The work that waits for a verdict is one group: a task that is being
    // merged and a task that is in review stand next to each other, in the
    // order of the plan.
    ordered.extend(
        rest.iter()
            .filter(|task| matches!(stage(task), GraphStage::Review | GraphStage::Merging))
            .cloned(),
    );
    ordered.extend(done);
    ordered
}

/// The lanes of the project, in seat order: one column each.
pub fn lanes(tx: &Tx<'_>, project: &str) -> GearResult<Vec<String>> {
    Ok(tx
        .seats_of_project(project)?
        .into_iter()
        .filter(|seat| seat.role == gear_model::SeatRole::Lane && seat.state != gear_model::SeatState::Retired)
        .map(|seat| seat.name)
        .collect())
}

/// The stage of a task: its state, and whether it is already accepted.
fn stage_of(
    tx: &Tx<'_>,
    project: &str,
    task: &gear_store::TaskRow,
) -> GearResult<GraphStage> {
    Ok(match task.state {
        TaskState::Done => GraphStage::Done,
        TaskState::Running => GraphStage::Working,
        TaskState::Verifying => {
            // An accepted task waits for its merge.
            let accepted = tx.verdicts_of_task(project, &task.id)?.iter().any(|verdict| verdict.passed);
            if accepted {
                GraphStage::Merging
            } else {
                GraphStage::Review
            }
        }
        _ => GraphStage::Queued,
    })
}

/// The time the task took, and when it was merged.
fn actual_of(history: &History, now: Ts) -> (Option<i64>, Option<Ts>) {
    let (start, _handed_in, merged) = history.span();
    // A task that is not merged yet is measured until now.
    let end = merged.or(Some(now));
    let actual = match (start, end) {
        (Some(start), Some(end)) if end >= start => Some((end - start) / 60),
        _ => None,
    };
    (actual, merged)
}

/// The lines of the graph, as plain text.
///
/// `selected` is the row the user has on: its dependencies and the tasks that
/// wait for it are marked.
pub fn lines(tasks: &[GraphTask], lanes: &[String], selected: Option<usize>) -> Vec<String> {
    styled_lines(tasks, lanes, selected).into_iter().map(|line| line.text).collect()
}

/// The colour of a stage (the prototype's STAGE_COL).
pub fn stage_color(stage: GraphStage) -> Color {
    match stage {
        GraphStage::Done => Color::Ok,
        GraphStage::Merging | GraphStage::Review => Color::Acc,
        GraphStage::Working => Color::Warn,
        GraphStage::Queued => Color::Dim,
    }
}

/// The columns of the graph: where the lanes and the text stand.
///
/// The prototype's columns (a lane every six, the id, the lane and the goal
/// after the lanes) grow with the longest lane name and the longest id, so a
/// long name never runs into the next column.
struct Columns {
    /// The distance between two lane columns.
    step: usize,
    /// The id of the task.
    id_x: usize,
    /// The lane of the task and the width of its field.
    lane_x: usize,
    lane_w: usize,
    /// What happened to the task.
    info_x: usize,
    /// The dependency marks.
    mark_x: usize,
}

impl Columns {
    fn of(tasks: &[GraphTask], lanes: &[String]) -> Columns {
        let names = lanes
            .iter()
            .map(String::as_str)
            .chain(tasks.iter().map(|task| task.lane.as_str()));
        let step = lane_width(lanes.iter().map(String::as_str), 6, 1);
        let lane_w = lane_width(names, 5, 1);
        let id_w = lane_width(tasks.iter().map(|task| task.id.as_str()), 6, 1);
        let id_x = 14 + step * lanes.len() + 1;
        let lane_x = id_x + id_w;
        // The goal: 35 columns, then a space before the info.
        let info_x = lane_x + lane_w + 36;
        Columns { step, id_x, lane_x, lane_w, info_x, mark_x: info_x + 33 }
    }

    /// The column of a lane: the first lane at 14, every next one a step
    /// further.
    fn lane(&self, index: usize) -> usize {
        14 + self.step * index
    }
}

/// The lines of the graph, in the prototype's colours.
pub fn styled_lines(tasks: &[GraphTask], lanes: &[String], selected: Option<usize>) -> Vec<Line> {
    let columns = Columns::of(tasks, lanes);
    let width = columns.mark_x + 60;
    let lane_style = |lane: &str| Style::fg(lane_color(lanes, lane));
    let mut out = Vec::new();

    // The head of the columns: dev, the review column, one column per lane.
    let mut head = Row::new(width);
    head.put(2, "dev", Style::fg(Color::Ok).and_bold());
    head.put(8, "rev", Style::fg(Color::Acc).and_bold());
    for (index, lane) in lanes.iter().enumerate() {
        head.put(columns.lane(index), lane, lane_style(lane).and_bold());
    }
    out.push(head.finish());

    let selected_task = selected.and_then(|index| tasks.get(index));
    for (index, task) in tasks.iter().enumerate() {
        let picked = selected == Some(index);
        let mut row = Row::new(width);
        // The line of dev runs down the whole graph.
        row.put(3, "│", Style::fg(Color::Ok));
        let x = columns.lane(lane_index(lanes, &task.lane)) + 1;
        if matches!(task.stage, GraphStage::Review | GraphStage::Merging) {
            // The task is in review: the line from the review column to it.
            row.put(9, "◎", Style::fg(Color::Acc).and_bold());
            row.put(10, &"─".repeat(x.saturating_sub(10)), Style::fg(Color::Acc));
        }
        // A lane's line runs while the lane still has tasks below this row.
        for (lane_index, lane) in lanes.iter().enumerate() {
            if tasks[index + 1..].iter().any(|other| &other.lane == lane) {
                row.put(columns.lane(lane_index) + 1, "│", lane_style(lane));
            }
        }
        row.put(x, task.stage.symbol(), Style::fg(stage_color(task.stage)).and_bold());
        if task.stage == GraphStage::Done {
            // The task was merged into dev: the line of the merge.
            row.put(3, "●", Style::fg(Color::Ok).and_bold());
            row.put(4, &"─".repeat(x.saturating_sub(4)), Style::fg(Color::Ok));
            // The lanes the merge crosses.
            for (lane_index, lane) in lanes.iter().enumerate() {
                let crossing = columns.lane(lane_index) + 1;
                if crossing < x && tasks[index + 1..].iter().any(|other| &other.lane == lane) {
                    row.put(crossing, "┼", lane_style(lane));
                }
            }
        }
        row.put(columns.id_x, &task.id, lane_style(&task.lane).reversed_if(picked));
        row.put(
            columns.lane_x,
            &format!("{:<w$}{:<35}", task.lane, cut(&task.title, 34), w = columns.lane_w),
            Style::PLAIN.reversed_if(picked),
        );
        let info_style = match task.stage {
            GraphStage::Done => Style::fg(Color::Dim),
            stage => Style::fg(stage_color(stage)),
        };
        row.put(columns.info_x, &info(task), info_style);
        if task.stage == GraphStage::Done {
            row.put(9, "✓", Style::fg(Color::Ok).and_bold());
        }
        if task.rounds > 1 {
            // The task was reworked.
            row.put(8, "↺", Style::fg(Color::Warn).and_bold());
        }
        // What the selection means for this row: the dependencies of the
        // selected task, and the tasks that wait for it.
        let mark = match selected_task {
            Some(selected) if selected.deps.contains(&task.id) => {
                Some(format!("◂ needed by {}", selected.id))
            }
            Some(selected) if task.deps.contains(&selected.id) => {
                Some(format!("▸ waits for {}", selected.id))
            }
            _ => None,
        };
        match mark {
            Some(mark) => row.put(columns.mark_x, &mark, Style::fg(Color::Warn).and_bold()),
            None if !task.deps.is_empty() => row.put(
                columns.mark_x,
                &format!("needs {}", task.deps.join(",")),
                Style::fg(Color::Dim),
            ),
            None => {}
        }
        out.push(row.finish());
    }
    out.push(Line::new());
    out.push(Line::styled(
        "○ queued  ◉ working  ◎ in review (rev column)  ↺ reworked  ✓● reviewed and merged into dev   select a row: its deps light up",
        Style::fg(Color::Dim),
    ));
    out
}

/// The index of a lane in the columns; a lane that is not a column (a task of
/// a lane that is retired) goes to the last column.
fn lane_index(lanes: &[String], lane: &str) -> usize {
    lanes.iter().position(|name| name == lane).unwrap_or(lanes.len().saturating_sub(1))
}

/// The text of the info column: what happened to the task.
fn info(task: &GraphTask) -> String {
    let estimate = task.est_min.unwrap_or(0);
    let actual = task.actual_min.unwrap_or(0);
    match task.stage {
        GraphStage::Done => {
            let mut info = match task.finished {
                Some(finished) => format!("merged {} · ", clock(finished)),
                None => String::new(),
            };
            info.push_str(&format!("{actual}/{estimate}m"));
            if task.rounds > 1 {
                info.push_str(&format!(" · {} rounds", task.rounds));
            }
            info
        }
        GraphStage::Working => format!("working {actual}/{estimate}m"),
        GraphStage::Review => "in review".to_string(),
        GraphStage::Merging => "merging".to_string(),
        GraphStage::Queued => format!("queued · {estimate}m"),
    }
}

/// The task panel: the rounds of the task, what it depends on, what needs
/// it.
pub fn panel(tx: &Tx<'_>, project: &str, task: &str, now: Ts) -> GearResult<Vec<String>> {
    Ok(styled_panel(tx, project, task, now)?.into_iter().map(|line| line.text).collect())
}

/// The colour of a task's state in a panel: the stage colours of the graph.
fn state_color(state: TaskState) -> Color {
    match state {
        TaskState::Done => Color::Ok,
        TaskState::Running => Color::Warn,
        TaskState::Verifying => Color::Acc,
        _ => Color::Dim,
    }
}

/// The task panel, in the prototype's colours.
pub fn styled_panel(tx: &Tx<'_>, project: &str, task: &str, now: Ts) -> GearResult<Vec<Line>> {
    let Some(row) = tx.get_task(project, task)? else {
        return Ok(vec![Line::plain(format!("{task} is not a task of {project}"))]);
    };
    let history = History::of(tx, project, task)?;
    let order = lanes(tx, project)?;
    let mut lines: Vec<Line> = Vec::new();
    let lane = row.lane.clone().unwrap_or_else(|| "-".to_string());
    let stage = match row.state {
        TaskState::Done => "done",
        TaskState::Running => "working",
        TaskState::Verifying => "in review",
        TaskState::Waiting => "waiting",
        TaskState::Ready => "queued",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
        TaskState::Draft => "draft",
    };
    lines.push(Line::styled(
        format!("{task} · lane {lane} · {stage}"),
        Style::fg(lane_color(&order, &lane)).and_bold(),
    ));
    let title = tx
        .get_contract(project, task, row.contract_version)?
        .map(|contract| contract.goal)
        .unwrap_or_default();
    if !title.is_empty() {
        lines.push(Line::plain(title));
    }
    lines.push(Line::new());
    let estimate = row.estimate_min.unwrap_or(0);
    let actual = actual_of(&history, now).0.unwrap_or(0);
    let over = estimate > 0 && actual > estimate;
    lines.push(Line::styled(
        format!("estimate {estimate}m · actual {actual}m"),
        if over { Style::fg(Color::Warn) } else { Style::PLAIN },
    ));
    lines.push(Line::new());
    lines.push(Line::styled("ROUNDS", Style::bold()));
    let rounds = tx.rounds_of_task(project, task)?;
    if !rounds.is_empty() {
        // The rounds of the store (§4): the time on the clock, the tokens and
        // the verdict of a review.
        for round in &rounds {
            let minutes = round.active_s / 60;
            let tokens = round.input.unwrap_or(0)
                + round.output.unwrap_or(0)
                + round.cache_read.unwrap_or(0)
                + round.cache_write.unwrap_or(0);
            let mut line = Line::plain(format!(
                "  {:<5} {:<7} {:<6} {:>3}m   {}k tokens",
                round.role,
                format!("attempt {}", round.attempt),
                round.cli,
                minutes,
                tokens / 1000
            ));
            if round.role == "review" {
                // How the review ended: it is still running when there is no
                // verdict yet.
                line = match tx.get_review_verdict(project, task, round.attempt)? {
                    Some(verdict) if verdict.passed => line.push("  → pass", Style::fg(Color::Ok)),
                    Some(_) => line.push("  → fail: rework", Style::fg(Color::Warn)),
                    None => line.push("  (running)", Style::fg(Color::Acc)),
                };
            }
            lines.push(line);
        }
    } else if history.attempts.is_empty() {
        lines.push(Line::styled("  not started", Style::fg(Color::Dim)));
    } else {
        // No rounds yet: the attempts of the task say what happened.
        for attempt in &history.attempts {
            let role = if attempt.n == 1 { "dev" } else { "rework" };
            let cli = cli_of(tx, &attempt.seat);
            let times = history.times_of(attempt.n);
            let minutes = match (times.started, times.handed_in) {
                (Some(started), Some(handed_in)) if handed_in >= started => {
                    (handed_in - started) / 60
                }
                _ => 0,
            };
            lines.push(Line::plain(format!(
                "  {role} {:<7} {cli:<6} {minutes:>3}m",
                format!("{}", attempt.n)
            )));
            if let Some(verdict) = tx.get_review_verdict(project, task, attempt.n)? {
                let (outcome, color) =
                    if verdict.passed { ("pass", Color::Ok) } else { ("fail: rework", Color::Warn) };
                lines.push(
                    Line::plain(format!(
                        "  review {:<7} {:<6}          ",
                        format!("{}", attempt.n),
                        cli_of(tx, &verdict.by_seat)
                    ))
                    .push(format!("→ {outcome}"), Style::fg(color)),
                );
            }
        }
    }
    lines.push(Line::new());
    lines.push(Line::styled("DEPENDS ON", Style::bold()));
    let mut depends = Vec::new();
    for dep in &row.deps {
        depends.push(dependency_line(tx, project, dep)?);
    }
    if depends.is_empty() {
        lines.push(Line::styled("  nothing", Style::fg(Color::Dim)));
    } else {
        lines.extend(depends);
    }
    lines.push(Line::new());
    lines.push(Line::styled("NEEDED BY", Style::bold()));
    let mut needed = Vec::new();
    for other in tx.tasks_of_project(project)? {
        if other.deps.iter().any(|dep| dep == task) {
            needed.push(Line::styled(
                format!(
                    "  {} {:<5} {:<30}  ({})",
                    other.id,
                    other.lane.clone().unwrap_or_else(|| "-".to_string()),
                    cut(&goal_of(tx, project, &other)?, 30),
                    state_word(other.state)
                ),
                Style::fg(state_color(other.state)),
            ));
        }
    }
    if needed.is_empty() {
        lines.push(Line::styled("  nothing", Style::fg(Color::Dim)));
    } else {
        lines.extend(needed);
    }
    Ok(lines)
}

/// One line of the "depends on" list.
fn dependency_line(tx: &Tx<'_>, project: &str, dep: &str) -> GearResult<Line> {
    let Some(row) = tx.get_task(project, dep)? else {
        return Ok(Line::styled(format!("  {dep} (unknown)"), Style::fg(Color::Dim)));
    };
    Ok(Line::styled(
        format!(
            "  {} {:<5} {:<30}  ({})",
            row.id,
            row.lane.clone().unwrap_or_else(|| "-".to_string()),
            cut(&goal_of(tx, project, &row)?, 30),
            state_word(row.state)
        ),
        Style::fg(state_color(row.state)),
    ))
}

/// The goal of a task (its title).
fn goal_of(tx: &Tx<'_>, project: &str, task: &gear_store::TaskRow) -> GearResult<String> {
    Ok(tx.get_contract(project, &task.id, task.contract_version)?
        .map(|contract| contract.goal)
        .unwrap_or_default())
}

/// The agent kind of a seat (the CLI that ran it).
fn cli_of(tx: &Tx<'_>, seat: &str) -> String {
    tx.get_seat(seat)
        .ok()
        .flatten()
        .map(|seat| seat.profile)
        .unwrap_or_else(|| "-".to_string())
}

/// The time of day of a moment, as the user's clock sees it.
fn clock(until: Ts) -> String {
    let seconds = until as libc::time_t;
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    let local = unsafe { libc::localtime_r(&seconds, &mut parts) };
    let (hour, minute) = if local.is_null() {
        (((until / 3600) % 24), ((until % 3600) / 60))
    } else {
        (parts.tm_hour as i64, parts.tm_min as i64)
    };
    format!("{hour:02}:{minute:02}")
}

/// The first `max` characters of a text.
fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// One line of the graph: characters at fixed columns, each in its style.
struct Row {
    cells: Vec<(char, Style)>,
}

impl Row {
    fn new(width: usize) -> Row {
        Row { cells: vec![(' ', Style::PLAIN); width] }
    }

    /// Write a text at a column, in a style.
    fn put(&mut self, x: usize, text: &str, style: Style) {
        for (offset, character) in text.chars().enumerate() {
            if x + offset < self.cells.len() {
                self.cells[x + offset] = (character, style);
            }
        }
    }

    /// The line, without the plain spaces at its end.
    fn finish(mut self) -> Line {
        while matches!(self.cells.last(), Some((' ', style)) if !style.shows_on_space()) {
            self.cells.pop();
        }
        let mut line = Line::new();
        for (character, style) in self.cells {
            line = line.push(character.to_string(), style);
        }
        line
    }
}
