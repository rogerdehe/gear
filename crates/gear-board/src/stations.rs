//! The voyage's stations: which are reached, which is now, which is next, and
//! how far each is (T-334, M5 S3; backlog "Stages of a long voyage on the
//! board").
//!
//! The store keeps the stations and each task's station; this module computes
//! their progress. A task counts as merged only when one of its attempts has a
//! merge time: a verdict marks a task Done before its merge lands
//! (`crates/gear-engine/src/waits.rs`), and a station is not reached before its
//! work is in the integration branch.

use std::collections::BTreeMap;

use gear_model::{GearResult, TaskState, Ts};
use gear_store::{TaskRow, Tx};

/// Where a station stands in the voyage's course.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StationState {
    /// Every task of it is merged.
    Reached,
    /// The first station that is not reached: the work of the voyage now.
    Now,
    /// The station after the one that is now.
    Next,
    /// Everything further.
    Later,
}

/// One station of the voyage and how far it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationProgress {
    /// The station's id, as the plan names it (`S1`, `S2`, …).
    pub id: String,
    /// What reaching it needs, in the user's words.
    pub needs: String,
    /// Where it stands in the course.
    pub state: StationState,
    /// The voyage's tasks that belong to it.
    pub tasks: usize,
    /// How many of them are merged.
    pub merged: usize,
    /// The minutes of estimate its unmerged tasks have left (an unknown
    /// estimate adds 0).
    pub left_min: i64,
    /// When the station was reached (T-355): the latest merge of its tasks, in
    /// whole minutes from the voyage's departure. None for a station that is
    /// not reached, and for every station of a voyage with no departure on
    /// record.
    pub reached_at_min: Option<i64>,
}

/// The voyage's stations, in plan order, with their progress.
///
/// The voyage's scope is its tasks whose state is not Draft, Cancelled or
/// Failed (the rule of `crates/gear-app/src/station.rs`).
pub fn progress(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
) -> GearResult<Vec<StationProgress>> {
    let stations = tx.voyage_stations(project, milestone)?;
    if stations.is_empty() {
        return Ok(Vec::new());
    }
    // The voyage's departure: reached minutes count from it (T-355).
    let departure = tx
        .get_voyage(project, milestone)?
        .and_then(|voyage| voyage.departure);
    let mut progress: Vec<StationProgress> = stations
        .into_iter()
        .map(|station| StationProgress {
            id: station.id,
            needs: station.needs,
            state: StationState::Later,
            tasks: 0,
            merged: 0,
            left_min: 0,
            reached_at_min: None,
        })
        .collect();
    // The latest merge of each station's tasks: the moment it was reached.
    let mut latest: BTreeMap<String, Ts> = BTreeMap::new();
    for task in scope(tx, project, milestone)? {
        let merged_at = merged_at(tx, project, &task.id)?;
        let Some(station) = tx.task_station(project, &task.id)? else {
            continue;
        };
        let Some(entry) = progress.iter_mut().find(|entry| entry.id == station) else {
            continue;
        };
        entry.tasks += 1;
        match merged_at {
            Some(at) => {
                entry.merged += 1;
                let latest_at = latest.entry(station).or_insert(at);
                *latest_at = (*latest_at).max(at);
            }
            None => {
                entry.left_min += crate::voyage::estimate_of(tx, project, &task)?
                    .unwrap_or(0);
            }
        }
    }
    // A station with no task yet is not reached — nor can one be whose work is
    // not merged. The first station that is not reached is now, the next one
    // is next, and everything further is later.
    let mut now = false;
    let mut next = false;
    for entry in &mut progress {
        if entry.tasks > 0 && entry.merged == entry.tasks {
            entry.state = StationState::Reached;
        } else if !now {
            entry.state = StationState::Now;
            now = true;
        } else if !next {
            entry.state = StationState::Next;
            next = true;
        } else {
            entry.state = StationState::Later;
        }
    }
    // A reached station says when the last of its work merged, in minutes from
    // the departure (T-355).
    for entry in &mut progress {
        if entry.state != StationState::Reached {
            continue;
        }
        entry.reached_at_min = latest
            .get(&entry.id)
            .zip(departure)
            .map(|(at, departure)| (at - departure).max(0) / 60);
    }
    Ok(progress)
}

/// Where the voyage stands in one line: the station that is now, what reaching
/// it needs and how much is left there and in the whole voyage.
pub fn course(tx: &Tx<'_>, project: &str, milestone: &str) -> GearResult<String> {
    let progress = progress(tx, project, milestone)?;
    let left = left_in_voyage(tx, project, milestone)?;
    if progress.is_empty() {
        return Ok(format!("{left} tasks left in the voyage"));
    }
    let total = progress.len();
    let reached = progress
        .iter()
        .filter(|station| station.state == StationState::Reached)
        .count();
    match progress.iter().find(|station| station.state == StationState::Now) {
        Some(now) => Ok(format!(
            "station {} ({reached} of {total} reached): {} · {} tasks left in {} · {left} tasks left in the voyage",
            now.id,
            now.needs,
            now.tasks - now.merged,
            now.id
        )),
        None => Ok(format!(
            "all {total} stations reached · {left} tasks left in the voyage"
        )),
    }
}

/// The one text every captain output uses: the voyage's goal, its bar and
/// where the voyage stands. None when the voyage is missing or its `done` is
/// empty.
pub fn goal_text(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
) -> GearResult<Option<String>> {
    let Some(voyage) = tx.get_voyage(project, milestone)? else {
        return Ok(None);
    };
    let Some(done) = voyage.done.filter(|done| !done.trim().is_empty()) else {
        return Ok(None);
    };
    let bar = voyage.bar.unwrap_or_default();
    let course = course(tx, project, milestone)?;
    Ok(Some(format!(
        "Your goal, voyage {milestone}: done: {} · bar: {} · {course}. Judge every decision against it.",
        done.trim(),
        bar.trim()
    )))
}

/// The tasks of the voyage that count: not a draft, not cancelled, not failed.
fn scope(tx: &Tx<'_>, project: &str, milestone: &str) -> GearResult<Vec<TaskRow>> {
    Ok(tx
        .tasks_of_project(project)?
        .into_iter()
        .filter(|task| task.milestone.as_deref() == Some(milestone))
        .filter(|task| {
            !matches!(
                task.state,
                TaskState::Draft | TaskState::Cancelled | TaskState::Failed
            )
        })
        .collect())
}

/// Whether the task is merged: one of its attempts has a merge time.
fn is_merged(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<bool> {
    Ok(merged_at(tx, project, task)?.is_some())
}

/// When the task merged: the latest of its attempts' merge times.
fn merged_at(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<Option<Ts>> {
    Ok(tx
        .attempt_times_of_task(project, task)?
        .iter()
        .filter_map(|(_, times)| times.merged)
        .max())
}

/// The scope tasks of the voyage that are not merged yet.
fn left_in_voyage(tx: &Tx<'_>, project: &str, milestone: &str) -> GearResult<usize> {
    let mut left = 0;
    for task in scope(tx, project, milestone)? {
        if !is_merged(tx, project, &task.id)? {
            left += 1;
        }
    }
    Ok(left)
}
