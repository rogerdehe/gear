//! Task and attempt state changes (design/06). Transaction-level: every
//! function takes the caller's `Tx` (plan apply, `gear send --kind result`, the
//! engine) and re-reads what it checks. Transitions go through
//! `TaskState::can_transition` / `AttemptState::can_transition` →
//! `ErrorCode::BadTransition`; a missing task/attempt → `NoTarget`.

use gear_model::state::require_transition;
use gear_model::{
    AttemptState, ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, SeatState,
    TaskState, Ts,
};
use gear_store::{AttemptRow, NewMessage, NewOp, Tx, VerdictRow};

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

/// The lane's view adopts the contract update it **showed** (T-8, design/06
/// `adopt_contract`, review R1/R2): `shown` is the version the page displayed
/// (`ViewRecord.shown_contract`). The attempt moves to exactly that version —
/// never a newer one the lane did not see — and a newer pending update stays
/// pending for its own notice. Returns the version adopted, or `None`.
///
/// A handed-in (Submitted) or finished attempt never changes its adopted
/// contract: its candidate is bound to the version it was submitted under, and
/// the verdict follows that version (R2). A page that was built before a
/// submission and recorded after it adopts nothing.
pub fn adopt_contract(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    n: i64,
    shown: i64,
    now: Ts,
) -> GearResult<Option<i64>> {
    let mut attempt = tx.get_attempt(project, task, n)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no attempt {n} of task {task} in project {project}"),
        )
    })?;
    // A handed-in or finished attempt keeps the version it was submitted under.
    if attempt.state == AttemptState::Submitted || attempt.state.is_terminal() {
        return Ok(None);
    }
    let Some(pending) = attempt.pending_update else {
        return Ok(None);
    };
    // Only a version the page actually showed can be adopted, and never one
    // older than what the attempt already works to.
    if shown <= attempt.adopted_version {
        return Ok(None);
    }
    attempt.adopted_version = shown;
    // A newer update the lane has not seen yet stays pending for its notice.
    attempt.pending_update = (pending > shown).then_some(pending);
    attempt.updated = now;
    tx.update_attempt(&attempt)?;
    Ok(Some(shown))
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
    // The candidate is bound to the contract it was submitted under: a pending
    // update is not adopted any more (R2), so it must not stay pending either.
    attempt.pending_update = None;
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
    let mut attempt = tx.get_attempt(&v.project, &v.task, v.attempt)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!(
                "no attempt {} of task {} in project {}",
                v.attempt, v.task, v.project
            ),
        )
    })?;

    // The same verdict again settles nothing; a conflicting one is a move the
    // attempt cannot make any more.
    for settled in tx.verdicts_of_task(&v.project, &v.task)? {
        if settled.attempt != v.attempt {
            continue;
        }
        if settled.passed == v.pass && settled.candidate == v.candidate {
            return Ok(());
        }
        return Err(GearError::new(
            ErrorCode::BadTransition,
            format!(
                "attempt {} of task {} was already settled with a different verdict",
                v.attempt, v.task
            ),
        ));
    }

    // A pass carries evidence, a fail carries the rework reason; never both.
    let fields_ok = match (v.pass, v.evidence.is_some(), v.rework.is_some()) {
        (true, true, false) | (false, false, true) => true,
        _ => false,
    };
    if !fields_ok {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            if v.pass {
                "a pass verdict needs `evidence` and no `rework`".to_string()
            } else {
                "a fail verdict needs `rework` and no `evidence`".to_string()
            },
        ));
    }

    if attempt.candidate.as_deref() != Some(v.candidate.as_str()) {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "attempt {} of task {} submitted candidate {:?}, not {:?}",
                v.attempt, v.task, attempt.candidate, v.candidate
            ),
        ));
    }
    // A pass may also name the task's current contract when the attempt
    // worked to an older one: the captain attests (with review evidence) that
    // the candidate meets the current contract. Needed while contract updates
    // can miss a lane mid-attempt (lesson L-43).
    let current_contract = tx.get_task(&v.project, &v.task)?.map(|t| t.contract_version);
    let attested = v.pass && Some(v.contract_version) == current_contract;
    if v.contract_version != attempt.adopted_version && !attested {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "attempt {} of task {} works to contract {}, not {}",
                v.attempt, v.task, attempt.adopted_version, v.contract_version
            ),
        ));
    }

    let mut task_row = tx.get_task(&v.project, &v.task)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no task {} in project {}", v.task, v.project),
        )
    })?;
    // A pass must be against the contract the task carries now; a fail may
    // settle an older one.
    if v.pass && v.contract_version != task_row.contract_version {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "a pass needs the task's current contract {}, not {}",
                task_row.contract_version, v.contract_version
            ),
        ));
    }

    let (attempt_state, attempt_name) = if v.pass {
        (AttemptState::Accepted, "accepted")
    } else {
        (AttemptState::Rejected, "rejected")
    };
    require_transition(
        AttemptState::can_transition(attempt.state, attempt_state),
        "attempt",
        attempt.state.as_str(),
        attempt_name,
    )?;
    let (task_state, task_name) = if v.pass {
        (TaskState::Done, "done")
    } else {
        (TaskState::Ready, "ready")
    };
    require_transition(
        TaskState::can_transition(task_row.state, task_state),
        "task",
        task_row.state.as_str(),
        task_name,
    )?;

    tx.insert_verdict(&VerdictRow {
        project: v.project.clone(),
        task: v.task.clone(),
        attempt: v.attempt,
        contract_version: v.contract_version,
        candidate: v.candidate.clone(),
        passed: v.pass,
        evidence: v.evidence.clone(),
        by_seat: v.by_seat.clone(),
        created: now,
    })?;

    attempt.state = attempt_state;
    attempt.updated = now;
    tx.update_attempt(&attempt)?;

    task_row.state = task_state;
    if !v.pass {
        task_row.rework_note = v.rework.clone();
    }
    task_row.updated = now;
    tx.update_task(&task_row)?;
    Ok(())
}

/// Settle a handed-in attempt from the review verdict stored for it (T-13):
/// the reviewer's verdict settles the task, the captain is informed.
///
/// - **pass**: attempt Accepted, task Done, and an Intended `merge` op for the
///   engine (T-12) that carries the task branch and the candidate;
/// - **fail**: attempt Rejected, task Ready with `rework_note` = the review's
///   findings, ranked one before the other ready tasks of its lane, so the
///   rework comes before new work (design/06);
/// - a **pass whose contract moved on** (the attempt worked to an older
///   contract than the task carries now) cannot complete the current contract
///   (design/06): it settles as a rework on the current contract, with the
///   move spelled out in the rework note;
/// - either way every review item of that attempt in the captain's inbox is
///   resolved and one **information line** (a Reply, state Info) says what
///   happened; the captain does not have to decide anything.
///
/// The verdict must exist (`NoTarget`) and the attempt must be handed in:
/// a verdict that already settled its attempt (Accepted/Rejected), or one
/// whose task is already finished (done, failed, cancelled), is obsolete and a
/// no-op (`Ok`, nothing written) — a second call, a replayed tick or a review
/// that arrives after a cancellation is safe (review R-67).
pub fn settle_review(tx: &Tx<'_>, project: &str, task: &str, attempt: i64, now: Ts) -> GearResult<()> {
    // The verdict is the whole point: without one there is nothing to settle.
    let review = tx
        .get_review_verdict(project, task, attempt)?
        .ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!("no review verdict for attempt {attempt} of task {task} in project {project}"),
            )
        })?;
    let mut attempt_row = tx.get_attempt(project, task, attempt)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no attempt {attempt} of task {task} in project {project}"),
        )
    })?;
    // A verdict settles its attempt once; a later call changes nothing.
    if attempt_row.state.is_terminal() {
        return Ok(());
    }
    let mut task_row = tx.get_task(project, task)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no task {task} in project {project}"),
        )
    })?;
    // A review of a task that is already finished (cancelled, failed, or done
    // by another path) is obsolete: the task keeps its state, the attempt is
    // closed so the lane is not blocked by it, and the review items are
    // resolved. It is never an error — one obsolete review must not stop the
    // engine's tick (R-67 P1).
    if task_row.state.is_terminal() {
        if attempt_row.state == AttemptState::Submitted {
            attempt_row.state = AttemptState::Rejected;
            attempt_row.updated = now;
            tx.update_attempt(&attempt_row)?;
        }
        let line = format!(
            "{task}: review arrived on a {} task — attempt {attempt} closed, nothing settled",
            task_row.state.as_str()
        );
        resolve_review_items(tx, project, task, attempt, &line, now)?;
        return Ok(());
    }

    // A pass accepts the candidate for the contract the attempt worked to; it
    // cannot complete a contract the task has moved on to (design/06: an
    // old-version result cannot complete a new contract). Such a pass settles
    // as a rework on the current contract.
    let stale = review.passed && attempt_row.adopted_version != task_row.contract_version;
    let accepted = review.passed && !stale;

    let (attempt_state, attempt_name, task_state, task_name) = if accepted {
        (AttemptState::Accepted, "accepted", TaskState::Done, "done")
    } else {
        (AttemptState::Rejected, "rejected", TaskState::Ready, "ready")
    };
    require_transition(
        AttemptState::can_transition(attempt_row.state, attempt_state),
        "attempt",
        attempt_row.state.as_str(),
        attempt_name,
    )?;
    require_transition(
        TaskState::can_transition(task_row.state, task_state),
        "task",
        task_row.state.as_str(),
        task_name,
    )?;

    // The attempt is settled first: the merge op below must never exist for an
    // attempt that is still open.
    attempt_row.state = attempt_state;
    attempt_row.updated = now;
    tx.update_attempt(&attempt_row)?;

    // The acceptance verdict is the task's history (design/06 `settle`): the
    // reviewer's verdict is recorded like any other, so a later captain
    // `settle` of the same attempt meets it and changes nothing. A stale pass
    // is recorded as what it settled to — a rejection of that attempt.
    tx.insert_verdict(&VerdictRow {
        project: project.to_string(),
        task: task.to_string(),
        attempt,
        contract_version: attempt_row.adopted_version,
        candidate: attempt_row.candidate.clone().unwrap_or_default(),
        passed: accepted,
        evidence: accepted.then(|| review.findings.clone()),
        by_seat: review.by_seat.clone(),
        created: now,
    })?;

    task_row.state = task_state;
    task_row.updated = now;
    if accepted {
        task_row.rework_note = None;
    } else {
        task_row.rework_note = Some(if stale {
            format!(
                "the review passed attempt {attempt} on contract v{}, but the task moved to v{}: an old-version result cannot complete the current contract. Rework the candidate to v{} and hand it in again. Reviewer: {} — {}",
                attempt_row.adopted_version,
                task_row.contract_version,
                task_row.contract_version,
                review.by_seat,
                review.findings
            )
        } else {
            findings_pointer(task, &review.findings)
        });
        // Rework before new work: one place ahead of the lane's other ready
        // tasks (design/06).
        task_row.rank = rework_rank(tx, &task_row)?;
    }
    tx.update_task(&task_row)?;

    // A pass is merged by the engine (T-12): the op is Intended here and the
    // merge op's executor knows how to run it. Booked to the lane's seat (the
    // branch is the lane's), or to the captain for a task without a lane seat.
    let mut merge_op = None;
    if accepted {
        let anchor = match task_row.lane.as_deref() {
            Some(lane) => tx.get_seat(&format!("{project}:{lane}"))?,
            None => None,
        };
        let anchor = match anchor {
            Some(seat) => seat,
            None => tx.captain_of(project)?.ok_or_else(|| {
                GearError::new(
                    ErrorCode::NoTarget,
                    format!("project {project} has no captain to book the merge op"),
                )
            })?,
        };
        let branch = task_row
            .lane
            .as_deref()
            .map(|lane| format!("{lane}/{}", task.to_lowercase().replace('-', "")));
        let op = tx.insert_op(&NewOp {
            project: project.to_string(),
            seat: anchor.id,
            generation: anchor.generation,
            kind: OpKind::Merge,
            task: Some(task.to_string()),
            attempt: Some(attempt),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({
                "branch": branch,
                "candidate": attempt_row.candidate,
            }),
            created: now,
        })?;
        merge_op = Some(op.id);
    }

    // The captain is told, not asked: every review item of this attempt stops
    // being an action item and one information line says what happened.
    let lane = task_row.lane.as_deref().unwrap_or("its lane");
    let line = match (&merge_op, stale) {
        (Some(op), _) => format!(
            "{task}: review pass — attempt {attempt} accepted, task done, merge queued ({op})"
        ),
        (None, true) => format!(
            "{task}: review passed attempt {attempt} on contract v{}, but the task is on v{} — attempt rejected, rework on the current contract, task ready at the head of {lane}'s queue",
            attempt_row.adopted_version, task_row.contract_version
        ),
        (None, false) => format!(
            "{task}: review fail — attempt {attempt} rejected, task ready at the head of {lane}'s queue with the findings"
        ),
    };
    resolve_review_items(tx, project, task, attempt, &line, now)?;
    Ok(())
}

/// The rank that puts `task` first in its lane's queue: one before the head of
/// the lane's other ready tasks (design/06: rework before new work). A task
/// without a lane, or a lane with no other ready task, keeps its rank.
fn rework_rank(tx: &Tx<'_>, task: &gear_store::TaskRow) -> GearResult<i64> {
    let Some(lane) = task.lane.as_deref() else {
        return Ok(task.rank);
    };
    let head = tx
        .tasks_of_project(&task.project)?
        .into_iter()
        .filter(|other| other.id != task.id)
        .filter(|other| other.lane.as_deref() == Some(lane))
        .filter(|other| other.state == TaskState::Ready)
        .map(|other| other.rank)
        .min();
    Ok(head.map(|rank| rank - 1).unwrap_or(task.rank))
}

/// Close **every** review item of `task`/`attempt` in the captain's inbox (it is
/// handled now) and leave one information line for the captain: a Reply in the
/// first item's thread, or a fresh line when no item is there any more. A
/// duplicate review submission must not survive as an action item (R-67 P1).
fn resolve_review_items(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    attempt: i64,
    line: &str,
    now: Ts,
) -> GearResult<()> {
    let Some(captain) = tx.captain_of(project)? else {
        // No captain, nothing to tell; the settlement itself stands.
        return Ok(());
    };
    let items: Vec<gear_store::MessageRow> = tx
        .actionable_for(project, &captain.id, now)?
        .into_iter()
        .filter(|message| {
            message.kind == MsgKind::Review
                && message.task.as_deref() == Some(task)
                && message.attempt == Some(attempt)
        })
        .collect();
    for message in &items {
        tx.update_message_state(
            project,
            &message.id,
            MsgState::Resolved,
            Some(line),
            None,
            None,
            now,
        )?;
    }
    let (thread, reply_to) = match items.first() {
        Some(message) => (message.thread.clone(), Some(message.id.clone())),
        None => (format!("{task}:review"), None),
    };
    tx.insert_message(&NewMessage {
        project: project.to_string(),
        thread,
        task: Some(task.to_string()),
        attempt: Some(attempt),
        sender: captain.id.clone(),
        sender_gen: captain.generation,
        recipient: captain.id.clone(),
        kind: MsgKind::Reply,
        urgent: false,
        body: line.to_string(),
        reply_to,
        needs_ack: false,
        state: MsgState::Info,
        created: now,
    })?;
    Ok(())
}

/// The rework note for a failed review: reviewers hand in a path on their
/// review branch (`ops/reviews/T-<n>.md`); the lane must be told where to read
/// it, because the review worktree is gone by then (lesson L-49).
fn findings_pointer(task: &str, findings: &str) -> String {
    let path = findings.trim();
    if path.ends_with(".md") && !path.contains(char::is_whitespace) {
        let branch = format!("review/{}", task.to_lowercase().replace('-', ""));
        format!("Review findings: `git show origin/{branch}:{path}` — fix what it lists, then hand in again.")
    } else {
        findings.to_string()
    }
}

#[cfg(test)]
mod findings_pointer_tests {
    use super::findings_pointer;

    #[test]
    fn a_path_becomes_a_command_on_the_review_branch() {
        assert_eq!(
            findings_pointer("T-12", "ops/reviews/T-12.md"),
            "Review findings: `git show origin/review/t12:ops/reviews/T-12.md` — fix what it lists, then hand in again."
        );
        assert_eq!(findings_pointer("T-3", "rows drift right"), "rows drift right");
    }
}
