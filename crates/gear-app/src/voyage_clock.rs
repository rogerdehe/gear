//! The voyage clock under the user's hand: a pause does not count, and a
//! voyage can depart again.
//!
//! A departure is set once by the plan and never moved by a captain (T-22,
//! T-473): the eta is the bar the voyage is measured against. Two exceptions
//! belong to the user, through `gear admin`:
//!
//! - `resume`: the hours a project spent paused move its departed voyages'
//!   departures forward by the same span, so a pause never counts as elapsed.
//! - `redepart`: a voyage starts its clock again now, with its scope as it
//!   stands, as if it departed today. Its time stations ring again from zero.

use gear_model::{GearError, GearResult, TaskState, Ts};
use gear_store::Tx;

/// When the pause started, from the pause value (`<ts> <reason>`).
fn paused_at(value: &str) -> Option<Ts> {
    value.split_whitespace().next()?.parse().ok()
}

/// Whether a voyage still sails: it has a task not yet finished.
fn sailing(tx: &Tx<'_>, project: &str, milestone: &str) -> GearResult<bool> {
    Ok(tx
        .tasks_of_project(project)?
        .iter()
        .any(|task| task.milestone.as_deref() == Some(milestone) && !task.state.is_terminal()))
}

/// Lift a project's pause. Each voyage still sailing has its departure moved
/// forward by the time it spent paused (from the later of the pause and its
/// departure). Returns the voyages moved, with the minutes.
pub fn resume(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Vec<(String, i64)>> {
    let mut moved = Vec::new();
    let since = tx.project_paused(project)?.as_deref().and_then(paused_at);
    if let Some(since) = since {
        for voyage in tx.voyages_of_project(project)? {
            let Some(departure) = voyage.departure else {
                continue;
            };
            let span = now - since.max(departure);
            if span <= 0 || !sailing(tx, project, &voyage.milestone)? {
                continue;
            }
            tx.move_voyage_departure(project, &voyage.milestone, departure + span)?;
            moved.push((voyage.milestone.clone(), span / 60));
        }
    }
    tx.set_meta(&gear_store::Tx::project_paused_key(project), "")?;
    let detail = moved
        .iter()
        .map(|(milestone, minutes)| format!("{milestone} +{minutes} min"))
        .collect::<Vec<_>>()
        .join(", ");
    tx.audit(now, Some(project), "admin", "resume", &detail)?;
    Ok(moved)
}

/// Start a voyage's clock again now: its departure, its scope at departure
/// (task count and estimate, as `depart: now` records them) and its time
/// stations start from today. Returns the tasks and estimate minutes.
pub fn redepart(tx: &Tx<'_>, project: &str, milestone: &str, now: Ts) -> GearResult<(i64, i64)> {
    let voyage = tx.get_voyage(project, milestone)?.ok_or_else(|| {
        GearError::new(
            gear_model::ErrorCode::NoTarget,
            format!("project {project:?} has no voyage {milestone:?}"),
        )
    })?;
    let tasks: Vec<_> = tx
        .tasks_of_project(project)?
        .into_iter()
        .filter(|task| task.milestone.as_deref() == Some(milestone))
        .collect();
    if tasks.is_empty() {
        return Err(GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("voyage {milestone} has no task: it departs with its plan's `depart: now`"),
        ));
    }
    let mut estimate = 0;
    for task in &tasks {
        estimate += gear_board::voyage::estimate_of(tx, project, task)?.unwrap_or(0);
    }
    tx.redepart_voyage(project, milestone, now, tasks.len() as i64, estimate)?;
    // The time stations count from the departure: the hour counter and the
    // eta quarters start again. The completion count stays where it is, so
    // the done tasks do not ring again.
    let key = format!("station:{project}:{milestone}");
    tx.delete_meta(&format!("{key}:course"))?;
    for marker in tx.meta_keys_with_prefix(&format!("{key}:eta:"))? {
        tx.delete_meta(&marker)?;
    }
    let done = tasks.iter().filter(|t| t.state == TaskState::Done).count();
    tx.set_meta(&format!("{key}:done"), &(done / 3).to_string())?;
    let old = voyage
        .departure
        .map(|at| at.to_string())
        .unwrap_or_else(|| "-".into());
    tx.audit(
        now,
        Some(project),
        "admin",
        "redepart",
        &format!(
            "{milestone}: departure {old} -> {now}, {} tasks, {estimate} min",
            tasks.len()
        ),
    )?;
    Ok((tasks.len() as i64, estimate))
}
