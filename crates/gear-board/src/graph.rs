//! The graph view of the board (design §8.1, the prototype of
//! `gear/voyages/M4/board`).
//!
//! Like `git log --graph`: `dev` on the left, a review column, one column per
//! lane, the newest work on top. A task that was merged into `dev` draws the
//! line of the merge, and the lanes it crosses are marked with `┼`. The
//! selected row lights up what it needs (`◂ needed by`) and what waits for
//! it (`▸ waits for`).
//!
//! The task panel (what `Enter` opens on a row) shows the rounds of the task,
//! what it depends on and what needs it.

use gear_model::{
    AttemptState, GearResult, MsgKind, MsgState, OpKind, OpState, SeatRole, SeatState, TaskState,
    Ts,
};
use gear_store::{AttemptRow, AttemptTimes, OpRow, RoundRow, Tx};

use crate::state_word;
use crate::style::{lane_color, lane_width, Color, Line, Style};

/// The attempts of a task and the times of each of them.
struct History {
    attempts: Vec<AttemptRow>,
    times: Vec<(i64, AttemptTimes)>,
    rounds: Vec<RoundRow>,
}

impl History {
    /// The history of a task: its attempts, the time of every step and its
    /// rounds.
    fn of(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<History> {
        Ok(History {
            attempts: tx.attempts_of_task(project, task)?,
            times: tx.attempt_times_of_task(project, task)?,
            rounds: tx.rounds_of_task(project, task)?,
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
        let start = self
            .times
            .iter()
            .filter_map(|(_, times)| times.started)
            .min();
        let handed_in = self
            .times
            .iter()
            .filter_map(|(_, times)| times.handed_in)
            .max();
        let merged = self
            .times
            .iter()
            .filter_map(|(_, times)| times.merged)
            .max();
        (start, handed_in, merged)
    }
}

/// The stage of a task in the graph: one of the sixteen steps of a task, in
/// time order (the prototype's STAGE, SYM, STAGE_COL and OPEN).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// It waits for a task it depends on.
    Deps,
    /// It is queued: the lane has not started it.
    Queued,
    /// It was delivered to its lane and nothing has started yet.
    Delivered,
    /// The lane works on it.
    Working,
    /// It waits for the captain's answer to a question.
    Asking,
    /// It is handed in and waits for a reviewer.
    Handed,
    /// A reviewer works on it.
    Review,
    /// It waits for the captain's verdict.
    Verdict,
    /// It was sent back and its rework is queued.
    SentBack,
    /// The lane reworks it.
    Rework,
    /// It passed and waits to merge.
    Passed,
    /// The merge is running.
    Merging,
    /// The merge failed and it waits for the captain.
    MergeFailed,
    /// It is merged into `dev`.
    Merged,
    /// It failed: its rounds are used up.
    Failed,
    /// It was moved to another milestone.
    Moved,
}

impl Stage {
    /// The symbol of the stage in the lane's column (the prototype's SYM).
    pub fn symbol(self) -> &'static str {
        match self {
            Stage::Deps | Stage::Queued => "○",
            Stage::Delivered => "◌",
            Stage::Working | Stage::Asking => "◉",
            Stage::Handed | Stage::Review | Stage::Verdict => "◎",
            Stage::SentBack | Stage::Rework => "↺",
            Stage::Passed | Stage::Merging => "◍",
            Stage::MergeFailed | Stage::Failed => "✗",
            Stage::Merged => "●",
            Stage::Moved => "→",
        }
    }

    /// The colour of the stage (the prototype's STAGE_COL).
    pub fn color(self) -> Color {
        match self {
            Stage::Deps | Stage::Queued | Stage::Delivered | Stage::Moved => Color::Dim,
            Stage::Working | Stage::Asking | Stage::Verdict | Stage::Rework => Color::Warn,
            Stage::Handed | Stage::Review | Stage::Passed | Stage::Merging => Color::Acc,
            Stage::SentBack | Stage::MergeFailed | Stage::Failed => Color::Bad,
            Stage::Merged => Color::Ok,
        }
    }

    /// Whether the stage is open (the prototype's OPEN): the task is on its
    /// way between its delivery and its merge.
    pub fn is_open(self) -> bool {
        matches!(
            self,
            Stage::Delivered
                | Stage::Working
                | Stage::Asking
                | Stage::Handed
                | Stage::Review
                | Stage::Verdict
                | Stage::SentBack
                | Stage::Rework
                | Stage::Passed
                | Stage::Merging
                | Stage::MergeFailed
        )
    }

    /// What the stage says of the task, in words (the prototype's STAGE).
    pub fn words(task: &GraphTask) -> String {
        Stage::words_of(
            task.stage,
            &task.deps,
            task.rounds,
            task.moved_to.as_deref(),
        )
    }

    /// The words of a stage from the facts it needs, without a [`GraphTask`]:
    /// the board's lanes and the captain's home page say the same words as the
    /// graph (T-347).
    pub fn words_of(
        stage: Stage,
        deps: &[String],
        rounds: usize,
        moved_to: Option<&str>,
    ) -> String {
        match stage {
            Stage::Deps => format!(
                "waits for {} to merge",
                if deps.is_empty() {
                    "-".to_string()
                } else {
                    deps.len().to_string()
                }
            ),
            Stage::Queued => "queued".to_string(),
            Stage::Delivered => "delivered, not started".to_string(),
            Stage::Working => "working".to_string(),
            Stage::Asking => "waits for the captain's answer".to_string(),
            Stage::Handed => "handed in, waits for a reviewer".to_string(),
            Stage::Review => "in review".to_string(),
            Stage::Verdict => "waits for the captain's verdict".to_string(),
            Stage::SentBack => "sent back, rework queued".to_string(),
            Stage::Rework => format!("reworking (round {rounds})"),
            Stage::Passed => "passed, waits to merge".to_string(),
            Stage::Merging => "merging (gate running)".to_string(),
            Stage::MergeFailed => "merge failed, waits for the captain".to_string(),
            Stage::Merged => "merged".to_string(),
            Stage::Failed => "failed (rounds used up)".to_string(),
            Stage::Moved => format!("moved to {}", moved_to.unwrap_or("-")),
        }
    }
}

/// One task of the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphTask {
    pub id: String,
    /// The queue position of the task in the plan, ascending (the store's
    /// rank).
    pub rank: i64,
    /// The lane that owns the task.
    pub lane: String,
    pub stage: Stage,
    /// The milestone the task was moved to (None here).
    pub moved_to: Option<String>,
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
    /// When the lane first started the task: the earliest `started` of its
    /// attempts.
    pub started: Option<Ts>,
}

/// The tasks of the graph, in the order the prototype draws them: the queued
/// first, then the work, then the reviews, then the merged ones (the newest
/// first).
pub fn tasks(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Vec<GraphTask>> {
    tasks_of(tx, project, now, |_| true)
}

/// The graph of the tasks the board shows (T-471): a board shows one voyage,
/// so the rows of the other voyages are dropped before their history is read
/// instead of after. The result is [`tasks`] filtered by `shown`, with the
/// same rows and the same order.
pub fn tasks_shown(
    tx: &Tx<'_>,
    project: &str,
    now: Ts,
    shown: &std::collections::BTreeSet<&str>,
) -> GearResult<Vec<GraphTask>> {
    tasks_of(tx, project, now, |id| shown.contains(id))
}

/// The rows of the graph, keeping the ones `keep` names: a board reads the
/// history of the rows it shows and of no other row.
fn tasks_of(
    tx: &Tx<'_>,
    project: &str,
    now: Ts,
    keep: impl Fn(&str) -> bool,
) -> GearResult<Vec<GraphTask>> {
    let rows = tx.tasks_of_project(project)?;
    let mut tasks = Vec::new();
    for row in &rows {
        if !keep(row.id.as_str()) {
            continue;
        }
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
            rank: row.rank,
            lane: row.lane.clone().unwrap_or_default(),
            stage: stage_of(tx, project, row)?,
            moved_to: None,
            title,
            est_min: row.estimate_min,
            actual_min: actual,
            rounds: history.attempts.len(),
            deps: row.deps.clone(),
            finished,
            started: history.span().0,
        });
    }
    Ok(order(tasks))
}

/// The order of the prototype (its `order_of`): the tasks that wait for a
/// dependency and the queued ones first, then the open ones (one group, in the
/// order of the plan), then the merged ones with the newest first.
fn order(mut tasks: Vec<GraphTask>) -> Vec<GraphTask> {
    let mut done: Vec<GraphTask> = Vec::new();
    let mut rest: Vec<GraphTask> = Vec::new();
    for task in tasks.drain(..) {
        if task.stage == Stage::Merged {
            done.push(task);
        } else {
            rest.push(task);
        }
    }
    done.sort_by_key(|task| task.finished.unwrap_or(0));
    done.reverse();
    let mut ordered: Vec<GraphTask> = Vec::new();
    ordered.extend(
        rest.iter()
            .filter(|task| matches!(task.stage, Stage::Deps | Stage::Queued))
            .cloned(),
    );
    ordered.extend(rest.iter().filter(|task| task.stage.is_open()).cloned());
    ordered.extend(done);
    ordered
}

/// One row of the git-style graph: the cells of the drawing at their columns,
/// the task the row is about (a row that is not about a task has none) and the
/// text after the drawing, which starts at `info_x`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    pub cells: Vec<(usize, char, Style)>,
    /// The task of the row: the row of a waiting task, the stage row of an
    /// open one, the row of a merge; a fork row has none.
    pub task: Option<String>,
    /// The text after the drawing: the id, the title, what the task is doing
    /// and its time.
    pub text: String,
    /// The style of the text.
    pub style: Style,
}

/// The rows of the git-style graph (the prototype's `view_graph`), newest on
/// top: `dev` on the left, each task a branch of its lane that leaves dev
/// where its work starts and joins it where it merges, and a task that has not
/// started yet hangs dashed in its lane, naming what it waits for.
///
/// A time is in minutes from `departure`. The drawing stands in columns (a lane
/// every six from 9) and the text after it starts at `info_x`; `tw` is the
/// width of its title column.
pub fn git_rows(
    tasks: &[GraphTask],
    lanes: &[String],
    departure: Ts,
    width: usize,
) -> Vec<GraphRow> {
    // The plan's order: a task's rank is its queue position.
    let mut plan: Vec<&GraphTask> = tasks.iter().collect();
    plan.sort_by_key(|task| task.rank);
    let col = |lane: &str| 9 + 6 * lane_index(lanes, lane);
    let last = lanes.len().saturating_sub(1);
    let info_x = 9 + 6 * last + 6;
    let tw = (width as i64 - info_x as i64 - 52).clamp(30, 56) as usize;
    let lane_style = |task: &GraphTask| Style::fg(lane_color(lanes, &task.lane));
    let minutes = |at: Ts| (at - departure) / 60;
    // The tasks whose branch is open: work started and not merged yet.
    let mut live: Vec<&str> = plan
        .iter()
        .filter(|task| task.started.is_some() && task.finished.is_none())
        .map(|task| task.id.as_str())
        .collect();
    // The lines of the open branches, down the rows, but the one of `skip`.
    let verticals = |live: &[&str], skip: Option<&str>| -> Vec<(usize, char, Style)> {
        plan.iter()
            .filter(|task| live.contains(&task.id.as_str()) && Some(task.id.as_str()) != skip)
            .map(|task| (col(&task.lane), '│', lane_style(task)))
            .collect()
    };
    let mut rows: Vec<GraphRow> = Vec::new();
    // The tasks that have not started hang in their lanes, the last to run
    // first: the plan's order reversed, so a task at the head of the queue sits
    // just above the running ones and the graph reads newest on top like git
    // (T-361 contract v2, the captain's M-5627).
    let waiting: Vec<&GraphTask> = plan
        .iter()
        .copied()
        .filter(|task| task.started.is_none() && task.stage != Stage::Moved)
        .collect();
    for task in waiting.iter().rev() {
        let x = col(&task.lane);
        let style = lane_style(task);
        let mut cells = verticals(&live, None);
        cells.push((x, '○', style.and_bold()));
        rows.push(GraphRow {
            cells,
            task: Some(task.id.clone()),
            text: git_info(task, tw),
            style: Style::fg(Color::Dim),
        });
        let mut cells = verticals(&live, None);
        cells.push((x, '┆', style));
        rows.push(GraphRow {
            cells,
            task: Some(task.id.clone()),
            text: String::new(),
            style: Style::PLAIN,
        });
    }
    // The starts and the merges, the newest first; equal minutes keep the
    // plan's order (a stable sort).
    let mut events: Vec<(i64, u8, &GraphTask)> = Vec::new();
    for task in &plan {
        if let Some(started) = task.started {
            events.push((minutes(started), 0, task));
        }
        if let Some(finished) = task.finished {
            events.push((minutes(finished), 1, task));
        }
    }
    events.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    for (at, kind, task) in events {
        let x = col(&task.lane);
        let style = lane_style(task);
        let skip = Some(task.id.as_str());
        if kind == 1 {
            // The merge: the branch joins dev.
            let mut cells = verticals(&live, skip);
            cells.push((3, '●', Style::fg(Color::Ok).and_bold()));
            cells.extend((4..x).map(|column| (column, '─', style)));
            cells.push((x, '╯', style));
            live.push(task.id.as_str());
            let mut text = git_info(task, tw);
            if let Some(finished) = task.finished {
                text.push_str(&format!("  merged {}", clock(finished)));
            }
            rows.push(GraphRow {
                cells,
                task: Some(task.id.clone()),
                text,
                style: Style::PLAIN,
            });
        } else {
            // The start: the branch leaves dev. A task that started and has
            // not merged yet stands on its branch first — by its times, as the
            // open set is (a task moved to another lane keeps its earliest
            // start while its stage is queued again).
            if task.started.is_some() && task.finished.is_none() {
                let mut cells = verticals(&live, skip);
                cells.push((
                    x,
                    symbol_of(task.stage),
                    Style::fg(task.stage.color()).and_bold(),
                ));
                rows.push(GraphRow {
                    cells,
                    task: Some(task.id.clone()),
                    text: git_info(task, tw),
                    style: Style::bold(),
                });
            }
            let mut cells = verticals(&live, skip);
            cells.push((3, '├', Style::fg(Color::Ok)));
            cells.extend((4..x).map(|column| (column, '─', style)));
            cells.push((x, '╮', style));
            live.retain(|id| *id != task.id.as_str());
            let mut text = format!("{:7}starts +{}", "", hm(at));
            if !task.deps.is_empty() {
                text.push_str(&format!(" after {}", task.deps.join(", ")));
            }
            rows.push(GraphRow {
                cells,
                task: None,
                text,
                style: Style::fg(Color::Dim),
            });
        }
    }
    rows
}

/// One row of the Tasks view's deps column (T-494): the cells of the
/// dependency lines at their columns, and the number of lines left out of the
/// column on this row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepsRow {
    pub cells: Vec<(usize, char, Style)>,
    pub more: usize,
}

/// The deps column of the Tasks view (T-494): for every task not yet merged,
/// a line from the row that names it to the row of every direct dependency it
/// waits for that is shown and not merged. Only the direct dependencies are
/// drawn: a dependency of a dependency gets its own line from that task, never
/// a line from the first one. A dependency on a merged task draws nothing.
///
/// The lines stand in `deps_x + column`, one column each; two lines share a
/// column only when their rows do not touch, so two lines that meet on a row
/// stay two lines. `drawn` is the number of columns the caller gives them; a
/// line that finds no column is counted in [`DepsRow::more`]. The lines of
/// `selected` are lit.
pub fn deps_rows(
    rows: &[GraphRow],
    tasks: &[GraphTask],
    deps_x: usize,
    drawn: usize,
    selected: Option<&str>,
) -> Vec<DepsRow> {
    let mut out: Vec<DepsRow> = rows
        .iter()
        .map(|_| DepsRow {
            cells: Vec::new(),
            more: 0,
        })
        .collect();
    let edges = edges_of(rows, tasks);
    let (placed, omitted, _) = place(&edges, drawn);
    for (column, start, end, task_id) in placed {
        let lit = selected == Some(task_id);
        let style = if lit {
            Style::fg(Color::Warn).and_bold()
        } else {
            Style::fg(Color::Dim)
        };
        for row in start..=end {
            let glyph = if row == start {
                '┌'
            } else if row == end {
                '└'
            } else {
                '│'
            };
            out[row].cells.push((deps_x + column, glyph, style));
        }
    }
    for (row, count) in omitted {
        out[row].more = count;
    }
    out
}

/// The number of columns the deps column needs for these rows (T-494): the
/// lines' minimum layout, before the caller caps it.
pub fn deps_needed(rows: &[GraphRow], tasks: &[GraphTask]) -> usize {
    let edges = edges_of(rows, tasks);
    place(&edges, usize::MAX).2
}

/// The lines of the deps column: `(task row, dependency row, task id)` for
/// every task not yet merged and every direct dependency of it that is shown
/// and not merged.
fn edges_of<'a>(rows: &[GraphRow], tasks: &'a [GraphTask]) -> Vec<(usize, usize, &'a str)> {
    // The row that names a shown task: the first row with the task and a text.
    let mut at: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        if let Some(task) = &row.task {
            if !row.text.is_empty() {
                at.entry(task.as_str()).or_insert(index);
            }
        }
    }
    // A dependency that is merged (or not in the shown graph) is not drawn.
    let merged = |id: &str| {
        tasks
            .iter()
            .find(|task| task.id == id)
            .is_none_or(|task| task.finished.is_some() || task.stage == Stage::Merged)
    };
    let mut edges: Vec<(usize, usize, &str)> = Vec::new();
    for task in tasks {
        if task.finished.is_some() || task.stage == Stage::Merged {
            continue;
        }
        let Some(&task_row) = at.get(task.id.as_str()) else {
            continue;
        };
        for dep in &task.deps {
            if merged(dep) {
                continue;
            }
            let Some(&dep_row) = at.get(dep.as_str()) else {
                continue;
            };
            if dep_row == task_row {
                continue;
            }
            edges.push((task_row, dep_row, task.id.as_str()));
        }
    }
    // The plan's order is the task's; the rows order the lines.
    edges.sort_by_key(|(task_row, dep_row, _)| (*task_row, *dep_row));
    edges
}

/// Place the lines in columns: each line gets the first column whose lines do
/// not touch its rows (a shared row would make two lines look like one), and
/// a line that finds no free column is counted on its task's row. Returns the
/// placed lines, the omitted counts per row, and the number of columns used.
fn place<'a>(
    edges: &[(usize, usize, &'a str)],
    cap: usize,
) -> (
    Vec<(usize, usize, usize, &'a str)>,
    std::collections::BTreeMap<usize, usize>,
    usize,
) {
    let mut spans: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut placed = Vec::new();
    let mut omitted: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    for &(task_row, dep_row, task_id) in edges {
        let start = task_row.min(dep_row);
        let end = task_row.max(dep_row);
        let column = (0..cap).find(|column| {
            spans
                .get(*column)
                .is_none_or(|spans| spans.iter().all(|(s, e)| end < *s || start > *e))
        });
        let Some(column) = column else {
            *omitted.entry(task_row).or_insert(0) += 1;
            continue;
        };
        while spans.len() <= column {
            spans.push(Vec::new());
        }
        spans[column].push((start, end));
        placed.push((column, start, end, task_id));
    }
    (placed, omitted, spans.len())
}

/// A span of minutes as the graph says it: the hours always, then the minutes.
fn hm(minutes: i64) -> String {
    format!("{}h{:02}m", minutes / 60, minutes % 60)
}

/// The one character a stage draws in its lane.
fn symbol_of(stage: Stage) -> char {
    stage.symbol().chars().next().unwrap_or(' ')
}

/// The text of a graph row after the drawing: the id, the title, what the task
/// is doing and its time (the prototype's `info` in `view_graph`).
fn git_info(task: &GraphTask, tw: usize) -> String {
    let estimate = task.est_min.unwrap_or(0);
    let actual = task.actual_min.unwrap_or(0);
    let time = if actual > 0 {
        format!("{actual}/{estimate}m")
    } else {
        format!("{estimate}m")
    };
    let rounds = if task.rounds > 1 {
        format!(" · {} rounds", task.rounds)
    } else {
        String::new()
    };
    format!(
        "{:<7}{:<tw$}  {:<36}{time}{rounds}",
        task.id,
        cut(&task.title, tw),
        cut(&Stage::words(task), 36),
        tw = tw
    )
}

/// The lanes of the project, in seat order: one column each.
pub fn lanes(tx: &Tx<'_>, project: &str) -> GearResult<Vec<String>> {
    Ok(tx
        .seats_of_project(project)?
        .into_iter()
        .filter(|seat| {
            seat.role == gear_model::SeatRole::Lane && seat.state != gear_model::SeatState::Retired
        })
        .map(|seat| seat.name)
        .collect())
}

/// The stage of a task, from the facts the store already has: the first rule
/// that holds decides. The board's lane lines read the same stage (T-347).
pub(crate) fn stage_of(
    tx: &Tx<'_>,
    project: &str,
    task: &gear_store::TaskRow,
) -> GearResult<Stage> {
    // Done and failed are endings of the task itself.
    match task.state {
        TaskState::Done => return Ok(Stage::Merged),
        TaskState::Failed => return Ok(Stage::Failed),
        _ => {}
    }
    // The attempt the task is on: the one with the highest n; an abandoned
    // one counts as none.
    let attempts = tx.attempts_of_task(project, &task.id)?;
    let current = attempts
        .iter()
        .max_by_key(|attempt| attempt.n)
        .filter(|attempt| attempt.state != AttemptState::Abandoned);
    if let Some(a) = current {
        // The newest attempt came back rejected: the rework waits for the lane.
        let newer = attempts.iter().any(|other| other.n > a.n);
        if a.state == AttemptState::Rejected && !newer {
            return Ok(Stage::SentBack);
        }
        if task.state == TaskState::Verifying {
            let passed = tx
                .verdicts_of_task(project, &task.id)?
                .iter()
                .any(|verdict| verdict.attempt == a.n && verdict.passed);
            if passed {
                // An accepted task waits for its merge: the merge op says
                // where it is.
                return Ok(
                    match newest_merge_op(tx, project, &task.id)?.map(|op| op.state) {
                        Some(OpState::Failed) => Stage::MergeFailed,
                        Some(OpState::Executing | OpState::Submitted | OpState::Unknown) => {
                            Stage::Merging
                        }
                        _ => Stage::Passed,
                    },
                );
            }
            if tx.get_review_verdict(project, &task.id, a.n)?.is_some() {
                // The reviewer handed its verdict in: the captain decides.
                return Ok(Stage::Verdict);
            }
            let held = tx.seats_of_project(project)?.iter().any(|seat| {
                seat.role == SeatRole::Temp
                    && seat.state == SeatState::Active
                    && seat.task_id.as_deref() == Some(task.id.as_str())
            });
            if held {
                // A reviewer holds the task.
                return Ok(Stage::Review);
            }
            let quota_temps = tx
                .get_project(project)?
                .map(|row| row.quota_temps)
                .unwrap_or(0);
            if quota_temps == 0 {
                // No reviewer can come: the captain decides.
                return Ok(Stage::Verdict);
            }
            return Ok(Stage::Handed);
        }
        if matches!(
            a.state,
            AttemptState::Started | AttemptState::Stopped | AttemptState::Unknown
        ) {
            let asked = tx
                .messages_from_since(project, &a.seat, a.started)?
                .iter()
                .any(|msg| {
                    msg.kind == MsgKind::Question
                        && msg.state == MsgState::Open
                        && msg.task.as_deref() == Some(task.id.as_str())
                });
            if asked {
                return Ok(Stage::Asking);
            }
        }
        if a.n > 1 && a.state == AttemptState::Delivered {
            // The rework was delivered and nothing has started yet.
            return Ok(Stage::SentBack);
        }
        if a.n > 1
            && matches!(
                a.state,
                AttemptState::Started | AttemptState::Stopped | AttemptState::Unknown
            )
        {
            return Ok(Stage::Rework);
        }
        match a.state {
            AttemptState::Delivered => return Ok(Stage::Delivered),
            AttemptState::Started | AttemptState::Stopped | AttemptState::Unknown => {
                return Ok(Stage::Working);
            }
            AttemptState::Submitted => return Ok(Stage::Handed),
            AttemptState::Accepted => return Ok(Stage::Passed),
            _ => {}
        }
    }
    // No attempt yet: the task waits for a task it depends on.
    if current.is_none() {
        for dep in &task.deps {
            let merged = tx
                .get_task(project, dep)?
                .map(|row| row.state == TaskState::Done)
                .unwrap_or(false);
            if !merged {
                return Ok(Stage::Deps);
            }
        }
    }
    Ok(Stage::Queued)
}

/// The newest merge op of a task, over every state of the ops: by its time,
/// then by the number after `O-`.
fn newest_merge_op(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<Option<OpRow>> {
    let mut newest: Option<OpRow> = None;
    for state in OpState::ALL {
        for op in tx.ops_in_state(project, *state)? {
            if op.kind != OpKind::Merge || op.task.as_deref() != Some(task) {
                continue;
            }
            let newer = match &newest {
                None => true,
                Some(current) => {
                    (op.created, op_number(&op.id)) > (current.created, op_number(&current.id))
                }
            };
            if newer {
                newest = Some(op);
            }
        }
    }
    Ok(newest)
}

/// The number of an id (`O-12` is 12).
fn op_number(id: &str) -> i64 {
    id.split_once('-')
        .and_then(|(_, number)| number.parse().ok())
        .unwrap_or(0)
}

/// The time the lane worked on the task, and when it was merged.
fn actual_of(history: &History, now: Ts) -> (Option<i64>, Option<Ts>) {
    let (start, _handed_in, merged) = history.span();
    // The work time, not the clock since the first delivery (T-348).
    if let Some(worked) = crate::worked_s_of(&history.rounds) {
        return (Some(worked / 60), merged);
    }
    // A task that was never worked on is measured until it was merged, or
    // until now when it is not merged yet.
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
    styled_lines(tasks, lanes, selected)
        .into_iter()
        .map(|line| line.text)
        .collect()
}

/// The colour of a stage (the prototype's STAGE_COL).
pub fn stage_color(stage: Stage) -> Color {
    stage.color()
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
        Columns {
            step,
            id_x,
            lane_x,
            lane_w,
            info_x,
            mark_x: info_x + 33,
        }
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
        // The review column: the task stands with a reviewer or with the
        // merge, and the line reaches its lane.
        let in_review = matches!(
            task.stage,
            Stage::Handed
                | Stage::Review
                | Stage::Verdict
                | Stage::Passed
                | Stage::Merging
                | Stage::MergeFailed
        );
        if in_review {
            row.put(
                9,
                task.stage.symbol(),
                Style::fg(task.stage.color()).and_bold(),
            );
            row.put(
                10,
                &"─".repeat(x.saturating_sub(10)),
                Style::fg(task.stage.color()),
            );
        }
        // A lane's line runs while the lane still has tasks below this row.
        for (lane_index, lane) in lanes.iter().enumerate() {
            if tasks[index + 1..].iter().any(|other| &other.lane == lane) {
                row.put(columns.lane(lane_index) + 1, "│", lane_style(lane));
            }
        }
        row.put(
            x,
            task.stage.symbol(),
            Style::fg(task.stage.color()).and_bold(),
        );
        if task.stage == Stage::Merged {
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
        row.put(
            columns.id_x,
            &task.id,
            lane_style(&task.lane).reversed_if(picked),
        );
        row.put(
            columns.lane_x,
            &format!(
                "{:<w$}{:<35}",
                task.lane,
                cut(&task.title, 34),
                w = columns.lane_w
            ),
            Style::PLAIN.reversed_if(picked),
        );
        let info_style = match task.stage {
            Stage::Merged => Style::fg(Color::Dim),
            stage => Style::fg(stage.color()),
        };
        row.put(columns.info_x, &info(task), info_style);
        if task.stage == Stage::Merged {
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
        "○ not started  ◌ delivered  ◉ working  ◎ in review (rev column)  ◍ passed / merging  ↺ reworked  ✗ failed  ✓● merged into dev   select a row: its deps light up",
        Style::fg(Color::Dim),
    ));
    out
}

/// The index of a lane in the columns; a lane that is not a column (a task of
/// a lane that is retired) goes to the last column.
fn lane_index(lanes: &[String], lane: &str) -> usize {
    lanes
        .iter()
        .position(|name| name == lane)
        .unwrap_or(lanes.len().saturating_sub(1))
}

/// The text of the info column: what happened to the task.
fn info(task: &GraphTask) -> String {
    let estimate = task.est_min.unwrap_or(0);
    let actual = task.actual_min.unwrap_or(0);
    let time = if actual > 0 {
        format!("{actual}/{estimate}m")
    } else {
        format!("{estimate}m")
    };
    let rounds = if task.rounds > 1 {
        format!(" · {} rounds", task.rounds)
    } else {
        String::new()
    };
    match task.stage {
        Stage::Merged => {
            let merged = match task.finished {
                Some(finished) => format!("merged {} · ", clock(finished)),
                None => String::new(),
            };
            format!("{merged}{time}{rounds}")
        }
        Stage::Deps
        | Stage::Queued
        | Stage::Delivered
        | Stage::Working
        | Stage::Asking
        | Stage::Handed
        | Stage::Review
        | Stage::Verdict
        | Stage::SentBack
        | Stage::Rework
        | Stage::Passed
        | Stage::Merging
        | Stage::MergeFailed
        | Stage::Failed
        | Stage::Moved => format!("{} · {time}", Stage::words(task)),
    }
}

/// The task panel: the rounds of the task, what it depends on, what needs
/// it.
pub fn panel(tx: &Tx<'_>, project: &str, task: &str, now: Ts) -> GearResult<Vec<String>> {
    Ok(styled_panel(tx, project, task, now)?
        .into_iter()
        .map(|line| line.text)
        .collect())
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
        return Ok(vec![Line::plain(format!(
            "{task} is not a task of {project}"
        ))]);
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
        if over {
            Style::fg(Color::Warn)
        } else {
            Style::PLAIN
        },
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
                let (outcome, color) = if verdict.passed {
                    ("pass", Color::Ok)
                } else {
                    ("fail: rework", Color::Warn)
                };
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
        return Ok(Line::styled(
            format!("  {dep} (unknown)"),
            Style::fg(Color::Dim),
        ));
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
    Ok(tx
        .get_contract(project, &task.id, task.contract_version)?
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
        Row {
            cells: vec![(' ', Style::PLAIN); width],
        }
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
