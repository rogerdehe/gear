//! Task and attempt state changes (design/06). Transaction-level: every
//! function takes the caller's `Tx` (plan apply, `gear send --kind result`, the
//! engine) and re-reads what it checks. Transitions go through
//! `TaskState::can_transition` / `AttemptState::can_transition` →
//! `ErrorCode::BadTransition`; a missing task/attempt → `NoTarget`.

use gear_model::state::require_transition;
use gear_model::{AttemptState, ErrorCode, GearError, GearResult, SeatState, TaskState, Ts};
use gear_store::{AttemptRow, Tx};

/// Create attempt `n = last + 1` for `task` on `seat_id` (state Delivered,
/// `contract_version = adopted_version =` the task's current contract version).
/// Preconditions (else `Forbidden`, detail says which): task Ready; every dep
/// Done; seat state Active or Standby with `generation` = the seat's current
/// generation; the seat has no in-progress attempt. The task stays Ready.
pub fn start_attempt(tx: &Tx<'_>, project: &str, task: &str, seat_id: &str, generation: i64, now: Ts) -> GearResult<AttemptRow> {
    let task_row = tx.get_task(project, task)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no task {task} in project {project}"),
        )
    })?;

    if task_row.state != TaskState::Ready {
        return Err(forbidden(format!(
            "task {task} is {}, only a ready task starts an attempt",
            task_row.state.as_str()
        )));
    }
    for dep in &task_row.deps {
        let dep_state = match tx.get_task(project, dep)? {
            Some(row) => row.state,
            None => {
                return Err(forbidden(format!(
                    "task {task} depends on {dep}, which does not exist"
                )))
            }
        };
        if dep_state != TaskState::Done {
            return Err(forbidden(format!(
                "task {task} depends on {dep}, which is {} and not done",
                dep_state.as_str()
            )));
        }
    }

    let seat = tx.get_seat(seat_id)?.ok_or_else(|| {
        GearError::new(ErrorCode::NoTarget, format!("no seat {seat_id}"))
    })?;
    if seat.generation != generation {
        return Err(forbidden(format!(
            "seat {seat_id} is at generation {}, the caller has {generation}",
            seat.generation
        )));
    }
    if !matches!(seat.state, SeatState::Active | SeatState::Standby) {
        return Err(forbidden(format!(
            "seat {seat_id} is {}, only an active or standby seat starts an attempt",
            seat.state.as_str()
        )));
    }
    if !tx.in_progress_attempts_of_seat(seat_id)?.is_empty() {
        return Err(forbidden(format!(
            "seat {seat_id} already has an attempt in progress"
        )));
    }

    let n = tx
        .attempts_of_task(project, task)?
        .last()
        .map(|attempt| attempt.n)
        .unwrap_or(0)
        + 1;
    let row = AttemptRow {
        project: project.to_string(),
        task: task.to_string(),
        n,
        seat: seat_id.to_string(),
        generation,
        contract_version: task_row.contract_version,
        adopted_version: task_row.contract_version,
        pending_update: None,
        state: AttemptState::Delivered,
        candidate: None,
        report: None,
        safe_point: None,
        started: now,
        updated: now,
    };
    tx.insert_attempt(&row)?;
    Ok(row)
}

/// Delivery confirmed: attempt Delivered → Started, task Ready → Running.
pub fn on_delivery_confirmed(tx: &Tx<'_>, project: &str, task: &str, n: i64, now: Ts) -> GearResult<()> {
    let mut attempt = tx.get_attempt(project, task, n)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no attempt {n} of task {task} in project {project}"),
        )
    })?;
    require_transition(
        AttemptState::can_transition(attempt.state, AttemptState::Started),
        "attempt",
        attempt.state.as_str(),
        AttemptState::Started.as_str(),
    )?;
    let mut task_row = tx.get_task(project, task)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no task {task} in project {project}"),
        )
    })?;
    require_transition(
        TaskState::can_transition(task_row.state, TaskState::Running),
        "task",
        task_row.state.as_str(),
        TaskState::Running.as_str(),
    )?;

    attempt.state = AttemptState::Started;
    attempt.updated = now;
    tx.update_attempt(&attempt)?;
    task_row.state = TaskState::Running;
    task_row.updated = now;
    tx.update_task(&task_row)?;
    Ok(())
}

/// `gear send --kind result`: the exact attempt, under its adopted contract
/// version (else `BadFormat`). Attempt Started/Stopped/Unknown → Submitted with
/// candidate and report; task Running → Verifying.
pub fn submit_result(tx: &Tx<'_>, project: &str, task: &str, n: i64, contract_version: i64, candidate: &str, report: &str, now: Ts) -> GearResult<()> {
    let mut attempt = tx.get_attempt(project, task, n)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no attempt {n} of task {task} in project {project}"),
        )
    })?;
    if contract_version != attempt.adopted_version {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "attempt {n} of task {task} works to contract {}, not {contract_version}",
                attempt.adopted_version
            ),
        ));
    }
    require_transition(
        AttemptState::can_transition(attempt.state, AttemptState::Submitted),
        "attempt",
        attempt.state.as_str(),
        AttemptState::Submitted.as_str(),
    )?;
    let mut task_row = tx.get_task(project, task)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no task {task} in project {project}"),
        )
    })?;
    require_transition(
        TaskState::can_transition(task_row.state, TaskState::Verifying),
        "task",
        task_row.state.as_str(),
        TaskState::Verifying.as_str(),
    )?;

    attempt.state = AttemptState::Submitted;
    attempt.candidate = Some(candidate.to_string());
    attempt.report = Some(report.to_string());
    attempt.updated = now;
    tx.update_attempt(&attempt)?;
    task_row.state = TaskState::Verifying;
    task_row.updated = now;
    tx.update_task(&task_row)?;
    Ok(())
}

fn forbidden(detail: impl std::fmt::Display) -> GearError {
    GearError::new(ErrorCode::Forbidden, detail.to_string())
}

/// An acceptance verdict (design/05 `# Verdicts`, design/06 `settle`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub project: String,
    pub task: String,
    pub attempt: i64,
    pub contract_version: i64,
    pub candidate: String,
    pub pass: bool,
    /// required for pass
    pub evidence: Option<String>,
    /// required for fail
    pub rework: Option<String>,
    pub by_seat: String,
}

/// Settle a Submitted attempt; appends the verdict row.
/// - pass: candidate and contract equal the attempt's; contract = the task's
///   current contract version; evidence given → attempt Accepted, task Done;
/// - fail: candidate and contract equal the attempt's (an older contract is
///   fine); rework given → attempt Rejected, task Ready, `rework_note` = rework.
/// A mismatch or missing field → `BadFormat`; the same verdict again (same
/// task/attempt/outcome/candidate) after it settled → Ok, nothing written;
/// a conflicting one → `BadTransition`.
pub fn settle(tx: &Tx<'_>, v: &Verdict, now: Ts) -> GearResult<()> {
    let _ = (tx, v, now);
    todo!("T-43")
}
