//! The captain's plain view of the work (`gear queue`, design §8.3).
//!
//! ```text
//! hull  working T-143 38/60m  · left 3 (125m): T-148 35m, T-153 30m needs T-143, T-160 60m
//! deck  working T-144 22/45m  · left 4 (175m): …
//! keel  idle: next T-150 waits for T-142 (hull, review) · left 1 (40m)
//! mast  idle: nothing left planned · left 0
//! review-1 reviewing T-142 · review-2 standby 3m
//! ```
//!
//! Everything is read from the store, never from the plan file. A lane that
//! is idle says why, through the one function the board uses
//! (`gear_board::idle::describe`), and the reviewers are on the last line.

use gear_board::idle::{describe, idle_reason};
use gear_model::{AttemptState, GearResult, SeatRole, SeatState, TaskState, Ts};
use gear_store::{Store, Tx};

/// How many tasks a lane's line names before it says "…".
const LISTED: usize = 3;

/// The queue of one project (design §8.3).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Queue {
    /// The project the queue is about.
    pub project: String,
    /// The lanes of the project, in seat order.
    pub lanes: Vec<Lane>,
    /// The reviewers (the temp seats), in seat order.
    pub reviewers: Vec<Reviewer>,
}

/// One lane of the queue.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Lane {
    /// The lane's name.
    pub lane: String,
    /// The task the lane works on, with its actual and estimated minutes.
    pub working: Option<Working>,
    /// Why the lane is idle (design §8.2); None while it works.
    pub idle: Option<String>,
    /// The tasks the lane still has.
    pub left: Vec<LeftTask>,
    /// The minutes of estimate the lane has left.
    pub left_min: i64,
}

/// The task a lane works on.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Working {
    /// The task's id.
    pub id: String,
    /// The minutes the task took so far.
    pub actual_min: Option<i64>,
    /// The minutes the task was estimated at.
    pub estimate_min: Option<i64>,
}

/// One task a lane still has.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LeftTask {
    /// The task's id.
    pub id: String,
    /// The minutes the task was estimated at.
    pub estimate_min: Option<i64>,
    /// The dependency the task waits for, when it waits for one.
    pub needs: Option<String>,
}

/// One reviewer (a temp seat).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Reviewer {
    /// The reviewer's name.
    pub name: String,
    /// The task it reviews.
    pub reviewing: Option<String>,
    /// How long it has been standing by.
    pub standby_min: Option<i64>,
}

/// The queue of the caller's project (the captain's view).
///
/// Read-only: nothing is written.
pub fn build(store: &mut Store, project: &str, now: Ts) -> GearResult<Queue> {
    store.read(|tx| queue(tx, project, now))
}

/// The queue of a project, read from the store.
pub fn queue(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Queue> {
    let mut lanes: Vec<Lane> = Vec::new();
    let mut reviewers: Vec<Reviewer> = Vec::new();
    let seats = tx.seats_of_project(project)?;
    let tasks = tx.tasks_of_project(project)?;
    for seat in seats {
        match seat.role {
            SeatRole::Lane => {
                // The lane's own tasks, in the order of the plan.
                let mut own: Vec<_> = tasks
                    .iter()
                    .filter(|task| task.lane.as_deref() == Some(seat.name.as_str()))
                    .filter(|task| task.state != TaskState::Draft)
                    .collect();
                own.sort_by_key(|task| (task.rank, task.id.clone()));
                // The task the lane works on: the one it has an attempt in
                // flight for. The board's own idle rule says the same: a
                // lane with an attempt in flight works, everything else is
                // idle for a reason.
                let working = working_of(tx, project, &seat.id, &own, now)?;
                // What is left: the tasks the lane has not finished.
                let left: Vec<LeftTask> = own
                    .iter()
                    .filter(|task| !task.state.is_terminal())
                    .filter(|task| working.as_ref().is_none_or(|working| working.id != task.id))
                    .map(|task| LeftTask {
                        id: task.id.clone(),
                        estimate_min: task.estimate_min,
                        needs: task
                            .deps
                            .iter()
                            .find(|dep| {
                                tasks
                                    .iter()
                                    .any(|other| &other.id == *dep && other.state != TaskState::Done)
                            })
                            .cloned(),
                    })
                    .collect();
                let left_min = left.iter().filter_map(|task| task.estimate_min).sum();
                // Why the lane is idle (design §8.2): the one function the
                // board uses.
                let idle = match idle_reason(tx, project, &seat.name, now)? {
                    Some(reason) => Some(describe(&reason, tx, project)),
                    None => None,
                };
                lanes.push(Lane {
                    lane: seat.name.clone(),
                    working,
                    idle,
                    left,
                    left_min,
                });
            }
            SeatRole::Temp => {
                // A reviewer works on one task for life (§3.8).
                let reviewing = seat.task_id.clone();
                let standby_min = (seat.state == SeatState::Standby)
                    .then(|| (now - seat.state_since).max(0) / 60);
                reviewers.push(Reviewer {
                    name: seat.name.clone(),
                    reviewing,
                    standby_min,
                });
            }
            _ => {}
        }
    }
    Ok(Queue {
        project: project.to_string(),
        lanes,
        reviewers,
    })
}

/// The task the lane works on: the one it has an attempt in flight for.
fn working_of(
    tx: &Tx<'_>,
    project: &str,
    seat_id: &str,
    own: &[&gear_store::TaskRow],
    now: Ts,
) -> GearResult<Option<Working>> {
    let open = tx.open_attempts_of_seat(seat_id)?;
    let attempt = open.into_iter().find(|attempt| {
        matches!(
            attempt.state,
            AttemptState::Delivered | AttemptState::Started | AttemptState::Stopped | AttemptState::Unknown
        )
    });
    let Some(attempt) = attempt else {
        return Ok(None);
    };
    let task = own.iter().find(|task| task.id == attempt.task);
    Ok(Some(Working {
        id: attempt.task.clone(),
        actual_min: actual_min_of(tx, project, &attempt.task, now)?,
        estimate_min: task.and_then(|task| task.estimate_min),
    }))
}

/// The minutes a task took so far: its first start until its last recorded
/// step (its merge, or the last thing that happened to it). A task that is
/// still running has no last step yet: the clock runs to now.
fn actual_min_of(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    now: Ts,
) -> GearResult<Option<i64>> {
    let times = tx.attempt_times_of_task(project, task)?;
    let start = times.iter().filter_map(|(_, times)| times.started).min();
    let end = times
        .iter()
        .filter_map(|(_, times)| times.merged)
        .max()
        .or_else(|| times.iter().filter_map(|(_, times)| times.last()).max())
        .or(Some(now));
    Ok(match (start, end) {
        (Some(start), Some(end)) if end >= start => Some((end - start) / 60),
        _ => None,
    })
}

/// The queue as the captain reads it (design §8.3).
pub fn render(queue: &Queue) -> String {
    let mut lines: Vec<String> = Vec::new();
    for lane in &queue.lanes {
        lines.push(lane_line(lane));
    }
    // The reviewers are on the last line.
    let reviewers: Vec<String> = queue
        .reviewers
        .iter()
        .map(|reviewer| reviewer_line(reviewer))
        .collect();
    if !reviewers.is_empty() {
        lines.push(reviewers.join(" · "));
    }
    lines.join("\n")
}

/// One lane: what it does now, why it is idle, and what it has left.
fn lane_line(lane: &Lane) -> String {
    let mut line = format!("{}  ", lane.lane);
    match (&lane.working, &lane.idle) {
        (Some(working), _) => {
            line.push_str(&format!(
                "working {} {}/{}m",
                working.id,
                minutes(working.actual_min),
                minutes(working.estimate_min)
            ));
        }
        (None, Some(idle)) => {
            line.push_str(&format!("idle: {idle}"));
        }
        (None, None) => {
            line.push_str("idle: nothing left planned");
        }
    }
    // What the lane has left: the count and the minutes, and the tasks
    // themselves when the lane works (an idle lane's reason already names
    // its next task).
    if lane.left.is_empty() {
        line.push_str(" · left 0");
        return line;
    }
    line.push_str(&format!(
        " · left {} ({}m)",
        lane.left.len(),
        lane.left_min
    ));
    if lane.working.is_some() {
        line.push_str(": ");
        let listed: Vec<String> = lane
            .left
            .iter()
            .take(LISTED)
            .map(|task| match &task.needs {
                Some(needs) => {
                    format!("{} {}m needs {needs}", task.id, minutes(task.estimate_min))
                }
                None => format!("{} {}m", task.id, minutes(task.estimate_min)),
            })
            .collect();
        line.push_str(&listed.join(", "));
        if lane.left.len() > LISTED {
            line.push_str(", …");
        }
    }
    line
}

/// One reviewer: the task it reviews, or how long it has been standing by.
fn reviewer_line(reviewer: &Reviewer) -> String {
    let mut line = reviewer.name.clone();
    match (&reviewer.reviewing, reviewer.standby_min) {
        (Some(task), _) => line.push_str(&format!(" reviewing {task}")),
        (None, Some(minutes)) => line.push_str(&format!(" standby {minutes}m")),
        (None, None) => line.push_str(" standby"),
    }
    line
}

/// A number of minutes, or "-" when it is not known.
fn minutes(value: Option<i64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string())
}
