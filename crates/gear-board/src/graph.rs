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

/// The order of the prototype: queued, working, in review, then the merged
/// ones with the newest first.
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
    for wanted in [
        GraphStage::Queued,
        GraphStage::Working,
        GraphStage::Review,
        GraphStage::Merging,
    ] {
        ordered.extend(rest.iter().filter(|task| stage(task) == wanted).cloned());
    }
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

/// The lines of the graph.
///
/// `selected` is the row the user has on: its dependencies and the tasks that
/// wait for it are marked.
pub fn lines(tasks: &[GraphTask], lanes: &[String], selected: Option<usize>) -> Vec<String> {
    let text_x = 14 + 6 * lanes.len() + 1;
    let width = text_x + 120;
    let mut out = Vec::new();

    // The head of the columns: dev, the review column, one column per lane.
    let mut head = Row::new(width);
    head.put(2, "dev");
    head.put(8, "rev");
    for (index, lane) in lanes.iter().enumerate() {
        head.put(column_x(index), lane);
    }
    out.push(head.finish());

    let selected_task = selected.and_then(|index| tasks.get(index));
    for (index, task) in tasks.iter().enumerate() {
        let mut row = Row::new(width);
        // The line of dev runs down the whole graph.
        row.put(3, "│");
        let x = column_x(lane_index(lanes, &task.lane)) + 1;
        if matches!(task.stage, GraphStage::Review | GraphStage::Merging) {
            // The task is in review: the line from the review column to it.
            row.put(9, "◎");
            row.put(10, &"─".repeat(x.saturating_sub(10)));
        }
        // A lane's line runs while the lane still has tasks below this row.
        for (lane_index, lane) in lanes.iter().enumerate() {
            if tasks[index + 1..].iter().any(|other| &other.lane == lane) {
                row.put(column_x(lane_index) + 1, "│");
            }
        }
        row.put(x, task.stage.symbol());
        if task.stage == GraphStage::Done {
            // The task was merged into dev: the line of the merge.
            row.put(3, "●");
            row.put(4, &"─".repeat(x.saturating_sub(4)));
            // The lanes the merge crosses.
            for (lane_index, lane) in lanes.iter().enumerate() {
                let crossing = column_x(lane_index) + 1;
                if crossing < x && tasks[index + 1..].iter().any(|other| &other.lane == lane) {
                    row.put(crossing, "┼");
                }
            }
        }
        row.put(text_x, &task.id);
        row.put(
            text_x + 6,
            &format!("{:<5}{:<35}", task.lane, cut(&task.title, 34)),
        );
        row.put(text_x + 47, &info(task));
        if task.stage == GraphStage::Done {
            row.put(9, "✓");
        }
        if task.rounds > 1 {
            // The task was reworked.
            row.put(8, "↺");
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
            Some(mark) => row.put(text_x + 80, &mark),
            None if !task.deps.is_empty() => {
                row.put(text_x + 80, &format!("needs {}", task.deps.join(",")))
            }
            None => {}
        }
        out.push(row.finish());
    }
    out.push(String::new());
    out.push(
        "○ queued  ◉ working  ◎ in review (rev column)  ↺ reworked  ✓● reviewed and merged into dev   select a row: its deps light up"
            .to_string(),
    );
    out
}

/// The column of a lane: the first lane at 14, every next one six further.
fn column_x(index: usize) -> usize {
    14 + 6 * index
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
    let Some(row) = tx.get_task(project, task)? else {
        return Ok(vec![format!("{task} is not a task of {project}")]);
    };
    let history = History::of(tx, project, task)?;
    let mut lines = Vec::new();
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
    lines.push(format!("{task} · lane {lane} · {stage}"));
    let title = tx
        .get_contract(project, task, row.contract_version)?
        .map(|contract| contract.goal)
        .unwrap_or_default();
    if !title.is_empty() {
        lines.push(title);
    }
    lines.push(String::new());
    let estimate = row.estimate_min.unwrap_or(0);
    let actual = actual_of(&history, now).0.unwrap_or(0);
    lines.push(format!("estimate {estimate}m · actual {actual}m"));
    lines.push(String::new());
    lines.push("ROUNDS".to_string());
    let rounds = tx.rounds_of_task(project, task)?;
    if !rounds.is_empty() {
        // The rounds of the store (§4): the time on the clock and the tokens.
        for round in &rounds {
            let minutes = round.active_s / 60;
            let tokens = round.input.unwrap_or(0)
                + round.output.unwrap_or(0)
                + round.cache_read.unwrap_or(0)
                + round.cache_write.unwrap_or(0);
            lines.push(format!(
                "  {:<5} {:<7} {:<6} {:>3}m   {}k tokens",
                round.role,
                format!("attempt {}", round.attempt),
                round.cli,
                minutes,
                tokens / 1000
            ));
        }
    } else if history.attempts.is_empty() {
        lines.push("  not started".to_string());
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
            lines.push(format!(
                "  {role} {:<7} {cli:<6} {minutes:>3}m",
                format!("{}", attempt.n)
            ));
            if let Some(verdict) = tx.get_review_verdict(project, task, attempt.n)? {
                let outcome = if verdict.passed { "pass" } else { "fail: rework" };
                lines.push(format!(
                    "  review {:<7} {:<6}          → {outcome}",
                    format!("{}", attempt.n),
                    cli_of(tx, &verdict.by_seat)
                ));
            }
        }
    }
    lines.push(String::new());
    lines.push("DEPENDS ON".to_string());
    let mut depends = Vec::new();
    for dep in &row.deps {
        depends.push(dependency_line(tx, project, dep)?);
    }
    if depends.is_empty() {
        lines.push("  nothing".to_string());
    } else {
        lines.extend(depends);
    }
    lines.push(String::new());
    lines.push("NEEDED BY".to_string());
    let mut needed = Vec::new();
    for other in tx.tasks_of_project(project)? {
        if other.deps.iter().any(|dep| dep == task) {
            needed.push(format!(
                "  {} {:<5} {:<30}  ({})",
                other.id,
                other.lane.clone().unwrap_or_else(|| "-".to_string()),
                cut(&goal_of(tx, project, &other)?, 30),
                state_word(other.state)
            ));
        }
    }
    if needed.is_empty() {
        lines.push("  nothing".to_string());
    } else {
        lines.extend(needed);
    }
    Ok(lines)
}

/// One line of the "depends on" list.
fn dependency_line(tx: &Tx<'_>, project: &str, dep: &str) -> GearResult<String> {
    let Some(row) = tx.get_task(project, dep)? else {
        return Ok(format!("  {dep} (unknown)"));
    };
    Ok(format!(
        "  {} {:<5} {:<30}  ({})",
        row.id,
        row.lane.clone().unwrap_or_else(|| "-".to_string()),
        cut(&goal_of(tx, project, &row)?, 30),
        state_word(row.state)
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

/// One line of the graph: characters at fixed columns.
struct Row {
    cells: Vec<char>,
}

impl Row {
    fn new(width: usize) -> Row {
        Row { cells: vec![' '; width] }
    }

    /// Write a text at a column.
    fn put(&mut self, x: usize, text: &str) {
        for (offset, character) in text.chars().enumerate() {
            if x + offset < self.cells.len() {
                self.cells[x + offset] = character;
            }
        }
    }

    /// The line, without the spaces at its end.
    fn finish(self) -> String {
        self.cells.into_iter().collect::<String>().trim_end().to_string()
    }
}
