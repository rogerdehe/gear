//! Task and attempt state changes (design/06). Transaction-level: every
//! function takes the caller's `Tx` (plan apply, `gear send --kind result`, the
//! engine) and re-reads what it checks. Transitions go through
//! `TaskState::can_transition` / `AttemptState::can_transition` →
//! `ErrorCode::BadTransition`; a missing task/attempt → `NoTarget`.

use gear_model::state::require_transition;
use gear_model::{
    AttemptState, ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, OpState, SeatState,
    TaskState, Ts,
};
use gear_store::{AttemptRow, AttemptStep, NewMessage, NewOp, Tx, VerdictRow};

/// Create attempt `n = last + 1` for `task` on `seat_id` (state Delivered,
/// `contract_version = adopted_version =` the task's current contract version).
/// Preconditions (else `Forbidden`, detail says which): task Ready; every dep
/// Done; seat state Active or Standby with `generation` = the seat's current
/// generation; the seat has no in-progress attempt. The task stays Ready.
pub fn start_attempt(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    seat_id: &str,
    generation: i64,
    now: Ts,
) -> GearResult<AttemptRow> {
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

    let seat = tx
        .get_seat(seat_id)?
        .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {seat_id}")))?;
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
    // T-458: an attempt whose task is in a declared wait is parked — a lane
    // that reported blocked takes its next startable task (M5: the attempt
    // stayed Started, so the engine assigned nothing for about 40 minutes).
    // The parked attempt comes back when its wait ends.
    for attempt in tx.in_progress_attempts_of_seat(seat_id)? {
        let waited = tx
            .get_task(project, &attempt.task)?
            .is_some_and(|task| task.wait_on.is_some());
        if !waited {
            return Err(forbidden(format!(
                "seat {seat_id} already has an attempt in progress"
            )));
        }
    }

    let n = tx
        .attempts_of_task(project, task)?
        .last()
        .map(|attempt| attempt.n)
        .unwrap_or(0)
        + 1;
    let row = AttemptRow {
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
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
    // The delivery has its own time (T-38): it is the moment the attempt
    // reached the lane, and no later step may move it.
    tx.set_attempt_time(project, task, n, AttemptStep::Delivered, now)?;
    Ok(row)
}

/// The seat's generation advances (a restart, a retry, a session step): the
/// attempt the seat is working on — `Delivered` or `Started` — moves to the
/// new generation with the seat, so the lane can still hand it in (T-217).
///
/// A lane that is restarted while its attempt waits for the lane moves on
/// with the attempt; without this the hand-in of the new generation is
/// refused ("not owned by this seat and generation") and the attempt is stuck
/// until a human moves it by hand.
///
/// Only the seat's own attempts move, and only the ones that are open in the
/// older generation: an attempt of another seat, or one that is already
/// settled, is untouched.
pub fn carry_attempt_generation(
    tx: &Tx<'_>,
    seat_id: &str,
    from_generation: i64,
    to_generation: i64,
    now: Ts,
) -> GearResult<()> {
    if from_generation == to_generation {
        return Ok(());
    }
    for mut attempt in tx.in_progress_attempts_of_seat(seat_id)? {
        if attempt.generation != from_generation {
            continue;
        }
        if !matches!(
            attempt.state,
            AttemptState::Delivered | AttemptState::Started
        ) {
            continue;
        }
        attempt.generation = to_generation;
        attempt.updated = now;
        tx.update_attempt(&attempt)?;
    }
    Ok(())
}

/// Delivery confirmed: attempt Delivered → Started, task Ready → Running.
pub fn on_delivery_confirmed(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    n: i64,
    now: Ts,
) -> GearResult<()> {
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
    // The work has begun (T-38): the dev time of the round runs from here.
    tx.set_attempt_time(project, task, n, AttemptStep::Started, now)?;
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

/// The candidate of the attempt before `n` — the work a rework must build on
/// (design §10.7, row B03-08). `None` for a first attempt: there is no
/// earlier work to keep.
pub fn rework_base_candidate(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    n: i64,
) -> GearResult<Option<String>> {
    if n <= 1 {
        return Ok(None);
    }
    Ok(tx
        .get_attempt(project, task, n - 1)?
        .and_then(|attempt| attempt.candidate))
}

/// Refuse a rework hand-in whose candidate does not contain the candidate of
/// the attempt before it (design §10.7, row B03-08): a rework keeps the work
/// the reviewer read, so the new candidate carries the old one. The refusal
/// names that commit — the one the lane has to merge in.
///
/// Git answers outside the store transaction (the worktree is the lane's), so
/// the caller runs this before it writes anything.
pub fn require_rework_contains(
    worktree: &std::path::Path,
    previous: &str,
    candidate: &str,
) -> GearResult<()> {
    if crate::base::contains(worktree, previous, candidate)? {
        return Ok(());
    }
    Err(GearError::new(
        ErrorCode::BadFormat,
        format!(
            "the rework candidate {candidate} does not contain {previous}: a rework keeps the work \
             the reviewer read (merge {previous} into your branch)"
        ),
    ))
}

/// `gear send --kind result`: the exact attempt, under its adopted contract
/// version (else `BadFormat`). Attempt Started/Stopped/Unknown → Submitted with
/// candidate and report; task Running → Verifying.
pub fn submit_result(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    n: i64,
    contract_version: i64,
    candidate: &str,
    report: &str,
    now: Ts,
) -> GearResult<()> {
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
    // The candidate must be a new one (T-118): a candidate that already failed
    // is not a rework. A lane that hands the same one in again reads its first
    // report, finds nothing to fix and loops (trial B02-06). The refusal names
    // the attempt that failed on it.
    for failed in tx.attempts_of_task(project, task)? {
        if failed.state != AttemptState::Rejected {
            continue;
        }
        if failed.candidate.as_deref() != Some(candidate) {
            continue;
        }
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{task}: candidate {candidate} already failed on attempt {}: rework it and hand in a \
                 different candidate (commit your rework and hand in its sha: git rev-parse HEAD)",
                failed.n
            ),
        ));
    }
    // A result is also a check-in: the owner may finish before a delivery
    // view is recorded. Both transitions happen in this transaction.
    if attempt.state == AttemptState::Delivered {
        on_delivery_confirmed(tx, project, task, n, now)?;
        attempt = tx
            .get_attempt(project, task, n)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, "attempt disappeared"))?;
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
    // The hand-in must name the task's *current* contract (T-63): the reviewer
    // is sent the contract the hand-in was made against, so a lane that read an
    // older version has to read the task again and adopt the current one
    // before it hands in. Nothing is written when this refuses.
    if contract_version != task_row.contract_version {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{task} is on contract v{}, not the v{contract_version} this hand-in names: \
                 run `gear` and `gear --show {task}` to work to v{}, then hand in again",
                task_row.contract_version, task_row.contract_version
            ),
        ));
    }
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
    // A restart or compaction can change the binding since check-in.
    if let Some(seat) = tx.get_seat(&attempt.seat)? {
        if let Some(binding) = tx.get_binding(&seat.id, seat.generation)? {
            tx.refresh_attempt_session(
                project,
                task,
                n,
                binding.session_ref.as_deref().filter(|s| !s.is_empty()),
                &binding.kind,
            )?;
        }
    }
    // The hand-in has its own time (T-38): the dev time ends here, the review
    // time begins.
    tx.set_attempt_time(project, task, n, AttemptStep::HandedIn, now)?;
    task_row.state = TaskState::Verifying;
    task_row.updated = now;
    tx.update_task(&task_row)?;
    Ok(())
}

/// The project's `[conventions]` as the store holds them (T-252): a project
/// without a config, or with one that does not parse, keeps GEAR's own
/// defaults — the branch names and the voyage folder here are a hint for the
/// captain and the merge, never a gate.
pub(crate) fn conventions_of(tx: &Tx<'_>, project: &str) -> crate::config::Conventions {
    tx.get_project(project)
        .ok()
        .flatten()
        .and_then(|row| crate::config::parse(&row.config_toml).ok())
        .map(|config| config.conventions)
        .unwrap_or_default()
}

fn forbidden(detail: impl std::fmt::Display) -> GearError {
    GearError::new(ErrorCode::Forbidden, detail.to_string())
}

/// The `# Verdicts` line that settles what a captain's item is about, when a
/// verdict — not `gear reply … --resolve` — is what settles it (T-90).
///
/// Two items need one: a review fail (the attempt is still handed in, the
/// task still Verifying) and a failed merge (the attempt was accepted, its
/// work is not in the integration branch, and the task must go back for a
/// rework). Everything else is answered by a reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictHint {
    /// The task's milestone (`M3`), for the voyage file the line goes in;
    /// `None` when the task has none.
    pub milestone: Option<String>,
    /// The voyage's plan file the line goes in (T-257): the project's
    /// `[conventions].voyage_dir` with the milestone filled in, `gear/voyages/M3/plan.md`
    /// by default and `gear/voyages/<M>/plan.md` for a task without milestone.
    pub plan: String,
    /// `T-<n> attempt=<a> contract=<c> candidate=<sha> fail rework="…"` with
    /// the real values. The rework is the captain's words.
    pub line: String,
    /// The task whose merge failed (B5): the captain retries it with a
    /// `# Staff` `- remerge T-<n>` line, not with a verdict. `None` when the
    /// item is settled by a verdict.
    pub remerge: Option<String>,
}

/// The verdict that settles `message`, with the real values filled in
/// (T-90); `None` when a reply is the right answer.
pub fn verdict_hint(
    tx: &Tx<'_>,
    project: &str,
    message: &gear_store::MessageRow,
) -> GearResult<Option<VerdictHint>> {
    // Only an action item needs settling: an information line (a pass, a
    // bookkeeping result) is closed by a reply and settles nothing anyway.
    if !matches!(message.state, MsgState::Open | MsgState::Waiting) {
        return Ok(None);
    }
    let (Some(task), Some(attempt)) = (message.task.as_deref(), message.attempt) else {
        return Ok(None);
    };
    let Some(task_row) = tx.get_task(project, task)? else {
        return Ok(None);
    };
    // A cancelled task is settled: a late item about it (a merge that failed
    // after the cancel) has nothing left to decide and closes by a reply.
    if task_row.state == TaskState::Cancelled {
        return Ok(None);
    }
    let Some(attempt_row) = tx.get_attempt(project, task, attempt)? else {
        return Ok(None);
    };
    let Some(candidate) = attempt_row.candidate.clone() else {
        return Ok(None);
    };
    let handed_in =
        attempt_row.state == AttemptState::Submitted && task_row.state == TaskState::Verifying;
    // A review fail: the item is the reviewer's verdict on this attempt (a
    // fail; a pass is information), and the attempt is still handed in.
    let review_fail = handed_in
        && message.kind == MsgKind::Review
        && tx
            .get_review_verdict(project, task, attempt)?
            .is_some_and(|verdict| !verdict.passed);
    // The engine's decision item for one (a fail without blocking findings,
    // or a rework over the cap): the captain decides, the task stays
    // Verifying.
    let captain_decision = handed_in && message.thread == format!("{task}:cap");
    // The merge ops of this task and attempt, in a state: a merge that failed
    // and one that went through are both about the same attempt.
    let merge_in = |state: gear_model::OpState| -> GearResult<bool> {
        Ok(tx.ops_in_state(project, state)?.iter().any(|op| {
            op.kind == OpKind::Merge
                && op.task.as_deref() == Some(task)
                && op.attempt == Some(attempt)
        }))
    };
    // A failed merge: the attempt passed and its work is not in the integration
    // branch. It holds only while **no** merge of that attempt has gone through
    // (T-109: a remerge that landed closes the question, whatever the old
    // Failed op still says), and once the captain's reopening fail is recorded
    // the item is an ordinary one — the historical op stays, but it has been
    // decided.
    let merge_failed = !reopened(tx, project, task, attempt, &candidate)?
        && !merge_in(gear_model::OpState::Confirmed)?
        && merge_in(gear_model::OpState::Failed)?;
    // Any other item — an ordinary question about the same task — is answered
    // and closed by a reply, and settles nothing.
    if !review_fail && !captain_decision && !merge_failed {
        return Ok(None);
    }
    let voyage_dir = conventions_of(tx, project).voyage_dir;
    Ok(Some(VerdictHint {
        milestone: task_row.milestone.clone(),
        plan: gear_model::layout::plan_file(
            &voyage_dir,
            project,
            task_row.milestone.as_deref().unwrap_or("<M>"),
        ),
        line: format!(
            "{task} attempt={attempt} contract={} candidate={candidate} fail rework=\"…\"",
            attempt_row.adopted_version
        ),
        // A failed merge is retried with `- remerge T-<n>` (B5): the work
        // passed, the merge did not — a rework is the other way round.
        remerge: if merge_failed {
            Some(task.to_string())
        } else {
            None
        },
    }))
}

/// Has the captain already sent this attempt back after a failed merge (T-90)?
/// The reopening fail is a fail verdict on the attempt's candidate while the
/// attempt keeps its accepted state.
fn reopened(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    attempt: i64,
    candidate: &str,
) -> GearResult<bool> {
    Ok(tx.verdicts_of_task(project, task)?.iter().any(|verdict| {
        verdict.attempt == attempt && !verdict.passed && verdict.candidate == candidate
    }))
}

/// The time the work of `task` was merged, if it is merged (design §10.7,
/// row B03-01): the attempt that landed it, and the time it landed.
///
/// `None` when the task has no merged attempt yet.
pub fn merged_at(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<Option<Ts>> {
    let mut merged = None;
    for (_, times) in tx.attempt_times_of_task(project, task)? {
        if let Some(at) = times.merged {
            merged = Some(at);
        }
    }
    Ok(merged)
}

/// The time the work of `task` was merged, or a refusal naming why not
/// (design §10.7, row B03-01): a pass that names the task whose merge landed
/// this work is only settled when that task is merged.
///
/// The named task must be **Done** and its **merge op confirmed**: a task
/// that is not done has not landed anything, and a merge time without a
/// confirmed merge op is a leftover of an earlier merge (T-205, the review of
/// T-165).
pub fn require_merged_at(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<Ts> {
    let Some(row) = tx.get_task(project, task)? else {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!("no task {task} in project {project}"),
        ));
    };
    if row.state != TaskState::Done {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{task} is not done yet (it is {}): a pass may name it as the merge that landed this work only when its own merge landed it",
                row.state.as_str()
            ),
        ));
    }
    // The attempt that landed the work, and the time it landed.
    let mut merged: Option<(i64, Ts)> = None;
    for (n, times) in tx.attempt_times_of_task(project, task)? {
        if let Some(at) = times.merged {
            merged = Some((n, at));
        }
    }
    let Some((attempt, at)) = merged else {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{task} is not merged yet: a pass may name it as the merge that landed this work only when it is merged"
            ),
        ));
    };
    // The merge op of that attempt must be confirmed: the merge is what
    // landed the work, and a merge time without it is not a merge.
    let confirmed = tx
        .ops_in_state(project, OpState::Confirmed)?
        .into_iter()
        .any(|op| {
            op.kind == OpKind::Merge
                && op.task.as_deref() == Some(task)
                && op.attempt == Some(attempt)
        });
    if !confirmed {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{task} has no confirmed merge for attempt {attempt}: a pass may name it as the merge that landed this work only when its own merge landed it"
            ),
        ));
    }
    Ok(at)
}

/// The work is merged by the named task's merge (design §10.7, row B03-01):
/// the attempt carries that merge's time, and the fact is kept so the board
/// and the report can say which merge landed it.
///
/// Written once: the attempt's merge time is the first one (the store keeps
/// it) and the attribution names the same merge, so a replay of the verdict
/// changes nothing.
fn mark_integrated_by(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    attempt: i64,
    by: &str,
    merged_at: Ts,
) -> GearResult<()> {
    tx.set_attempt_time(project, task, attempt, AttemptStep::Merged, merged_at)?;
    tx.set_meta(&format!("integrated-by:{project}:{task}"), by)?;
    Ok(())
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
    /// The task whose merge landed this work (design §10.7, row B03-01):
    /// `integrated-by=T-m`. The settle decides what it means.
    pub integrated_by: Option<String>,
    pub by_seat: String,
}

/// Settle a Submitted attempt; appends the verdict row.
/// - pass: candidate and contract equal the attempt's; contract = the task's
///   current contract version; evidence given → attempt Accepted, task Done;
/// - fail: candidate and contract equal the attempt's (an older contract is
///   fine); rework given → attempt Rejected, task Ready, `rework_note` = rework;
///   on an attempt that already passed whose task is Done (a failed merge,
///   T-90) the task reopens for a rework on a new attempt and the accepted
///   attempt keeps its state.
/// A mismatch or missing field → `BadFormat`; the same verdict again (same
/// task/attempt/outcome/candidate) after it settled → Ok, nothing written;
/// a conflicting one → `BadTransition`.
pub fn settle(tx: &Tx<'_>, v: &Verdict, now: Ts) -> GearResult<()> {
    let mut attempt = tx
        .get_attempt(&v.project, &v.task, v.attempt)?
        .ok_or_else(|| {
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
            // The same verdict again settles nothing; but a pass that names
            // the merge that landed the work (design §10.7, row B03-01) still
            // has to be marked: the attribution arrives after the attempt was
            // accepted — the normal path after an auto-settled review or a
            // failed merge. The marking is written once, so the replay of the
            // same plan changes nothing.
            if v.pass {
                if let Some(by) = v.integrated_by.as_deref() {
                    let merged_at = require_merged_at(tx, &v.project, by)?;
                    mark_integrated_by(tx, &v.project, &v.task, v.attempt, by, merged_at)?;
                }
            }
            return Ok(());
        }
        // The one conflict a captain may make (T-90): a pass that already
        // settled this attempt, followed by the fail that reopens the task
        // because its merge failed. The attempt stays accepted below.
        if settled.passed
            && !v.pass
            && settled.candidate == v.candidate
            && attempt.state == AttemptState::Accepted
        {
            continue;
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
    let current_contract = tx
        .get_task(&v.project, &v.task)?
        .map(|t| t.contract_version);
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
    // A pass may name the task whose merge landed this work (design §10.7,
    // row B03-01): the work is already on the integration branch, so this
    // task is marked merged by that merge instead of waiting for a merge of
    // its own. The named task must be merged already; the git side of the
    // rule (the candidate's tree is in the integration branch) is answered
    // by the caller, before the write.
    let integrated_at = match v.integrated_by.as_deref() {
        Some(by) if v.pass => Some((by.to_string(), require_merged_at(tx, &v.project, by)?)),
        Some(_) => None,
        None => None,
    };
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

    // A failed merge (T-90): the attempt passed, but its work is not in the
    // integration branch. The captain's fail sends the task back — the
    // attempt keeps its accepted state (it did pass), the task reopens for a
    // rework on a new attempt.
    let reopen =
        !v.pass && attempt.state == AttemptState::Accepted && task_row.state == TaskState::Done;

    let (attempt_state, attempt_name) = if v.pass {
        (AttemptState::Accepted, "accepted")
    } else {
        (AttemptState::Rejected, "rejected")
    };
    if !reopen {
        require_transition(
            AttemptState::can_transition(attempt.state, attempt_state),
            "attempt",
            attempt.state.as_str(),
            attempt_name,
        )?;
    }
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

    if !reopen {
        attempt.state = attempt_state;
        attempt.updated = now;
        tx.update_attempt(&attempt)?;
        // The captain decided (T-38): the wait for the captain ends here.
        tx.set_attempt_time(&v.project, &v.task, v.attempt, AttemptStep::Settled, now)?;
    }

    task_row.state = task_state;
    if !v.pass {
        task_row.rework_note = v.rework.clone();
    }
    task_row.updated = now;
    tx.update_task(&task_row)?;

    // The work is merged by the named task's merge: the attempt carries that
    // merge's time, and the fact is kept so the board and the report can say
    // which merge landed it (design §10.7, row B03-01).
    if let Some((by, merged_at)) = integrated_at {
        mark_integrated_by(tx, &v.project, &v.task, v.attempt, &by, merged_at)?;
    }
    // T-505: an audit passes over the integration branch it checked, so its
    // pass is a merge with no merge op: the attempt carries the merge time, and
    // the tasks that depend on the audit start as they do after a merge.
    if v.pass && tx.task_audit(&v.project, &v.task)?.is_some() {
        tx.set_attempt_time(
            &v.project,
            &v.task,
            v.attempt,
            AttemptStep::Merged,
            now,
        )?;
    }
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
/// - either way every actionable review item of that attempt in the captain's inbox is
///   resolved and one **information line** (System, state Info) says what
///   happened; the captain does not have to decide anything.
///
/// The verdict must exist (`NoTarget`) and the attempt must be handed in:
/// a verdict that already settled its attempt (Accepted/Rejected), or one
/// whose task is already finished (done, failed, cancelled), is obsolete and a
/// no-op (`Ok`, nothing written) — a second call, a replayed tick or a review
/// that arrives after a cancellation is safe (review R-67).
pub fn settle_review(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    attempt: i64,
    now: Ts,
) -> GearResult<()> {
    // The verdict is the whole point: without one there is nothing to settle.
    let review = tx
        .get_review_verdict(project, task, attempt)?
        .ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!(
                    "no review verdict for attempt {attempt} of task {task} in project {project}"
                ),
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
            // The attempt is closed (T-38): it has its settle time even
            // though nobody had to decide anything.
            tx.set_attempt_time(project, task, attempt, AttemptStep::Settled, now)?;
        }
        let line = format!(
            "{task}: review arrived on a {} task — attempt {attempt} closed, nothing settled",
            task_row.state.as_str()
        );
        resolve_review_items(tx, project, task, attempt, &line, now)?;
        return Ok(());
    }

    // T-505: an audit attempt is ruled by the captain, never settled here. A
    // pass records the audit as merged at the candidate it checked, and a fail
    // puts the task back on Ready so the captain can add the fix tasks to its
    // deps; both are plan `# Verdicts` lines.
    if tx.task_audit(project, task)?.is_some() {
        let line = format!(
            "{task}: audit review is in — the captain rules (the task stays Verifying)"
        );
        resolve_review_items(tx, project, task, attempt, &line, now)?;
        tell_captain_cap(tx, project, task, attempt, &line, now)?;
        return Ok(());
    }

    // The findings the reviewer named as blocking, and the ones it only logged
    // (T-23).
    let blocking_ids = blocking_ids(tx, project, task, attempt)?;
    let (blocking, logged) = split_findings(&review.findings, &blocking_ids);

    // (2) A fail that names no blocking finding is never converted into a
    // pass: the captain decides, and the task stays Verifying (M-295).
    if !review.passed && blocking.is_empty() {
        let line = format!(
            "{task}: fail without blocking findings — the captain decides (the task stays Verifying)"
        );
        resolve_review_items(tx, project, task, attempt, &line, now)?;
        tell_captain_cap(tx, project, task, attempt, &line, now)?;
        return Ok(());
    }

    // A pass accepts the candidate for the contract the attempt worked to; it
    // cannot complete a contract the task has moved on to (design/06: an
    // old-version result cannot complete a new contract). Such a pass settles
    // as a rework on the current contract.
    let stale = review.passed && attempt_row.adopted_version != task_row.contract_version;
    let accepted = review.passed && !stale;
    let rework = !accepted;

    // The rework rounds are capped (T-23): past the cap the captain decides.
    // The task stays Verifying, the attempt stays handed in, and one action
    // item goes to the captain's inbox.
    if rework && rework_rounds_over_cap(tx, project, task, attempt)? {
        let line = format!(
            "{task}: review round {attempt} is over the rework cap — the captain decides (the task stays Verifying)"
        );
        resolve_review_items(tx, project, task, attempt, &line, now)?;
        tell_captain_cap(tx, project, task, attempt, &line, now)?;
        return Ok(());
    }

    let (attempt_state, attempt_name, task_state, task_name) = if accepted {
        (AttemptState::Accepted, "accepted", TaskState::Done, "done")
    } else {
        (
            AttemptState::Rejected,
            "rejected",
            TaskState::Ready,
            "ready",
        )
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
    // The attempt is settled (T-38): the wait for the captain ends here.
    tx.set_attempt_time(project, task, attempt, AttemptStep::Settled, now)?;

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
            // Only the blocking findings go back to the lane (T-23); the rest
            // are information for the captain. The findings arrive as text
            // (T-21 reads the reviewer's file), so the note is that text.
            blocking.join("\n")
        });
        // A rework round's budget grows with the blocking findings it carries
        // (T-312): 8 minutes per blocking finding, never below the old rule of
        // half the estimate, at least 10. A stale pass has no findings to
        // count and keeps the old rule; a captain's own `estimate=` still
        // overrides this after its settle (plan.rs).
        task_row.rework_estimate_min = if stale {
            None
        } else {
            let estimate = task_row
                .estimate_min
                .unwrap_or(gear_store::DEFAULT_ROUND_BUDGET_MIN);
            Some(((estimate + 1) / 2).max(10).max(8 * blocking.len() as i64))
        };
        // Rework before new work: one place ahead of the lane's other ready
        // tasks (design/06).
        task_row.rank = rework_rank(tx, &task_row)?;
    }
    tx.update_task(&task_row)?;
    // The `fresh:` mark of a task is about its first round (T-481): a task
    // that is done carries none.
    if accepted {
        tx.clear_task_fresh(project, task)?;
    }

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
        let branch = match task_row.lane.as_deref() {
            Some(lane) => Some(crate::base::task_branch(
                &conventions_of(tx, project),
                project,
                lane,
                task,
            )),
            None => None,
        };
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
    let mut line = match (&merge_op, stale) {
        (Some(op), _) => format!(
            "{task}: review pass — attempt {attempt} accepted, task done, merge queued ({op})"
        ),
        (None, true) => format!(
            "{task}: review passed attempt {attempt} on contract v{}, but the task is on v{} — attempt rejected, rework on the current contract, task ready at the head of {lane}'s queue",
            attempt_row.adopted_version, task_row.contract_version
        ),
        (None, false) => format!(
            "{task}: review fail — attempt {attempt} rejected, task ready at the head of {lane}'s queue with the blocking findings"
        ),
    };
    // The findings the reviewer only logged: one information item for the
    // captain (T-23).
    if !logged.is_empty() {
        line.push_str("\nLogged (not blocking):");
        for finding in &logged {
            line.push_str("\n  ");
            line.push_str(finding);
        }
    }
    resolve_review_items(tx, project, task, attempt, &line, now)?;
    Ok(())
}

/// The ids the reviewer named as blocking (T-23).
fn blocking_ids(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    attempt: i64,
) -> GearResult<std::collections::BTreeSet<String>> {
    let Some(ids) = tx.review_blocking(project, task, attempt)? else {
        return Ok(std::collections::BTreeSet::new());
    };
    Ok(ids
        .split(',')
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect())
}

/// Split the findings of a review into the blocking ones (the lines whose id
/// the reviewer named) and the ones it only logged (T-23). A finding's id is
/// the first word of its line (`F1: …`, `P1: …`).
fn split_findings(
    findings: &str,
    blocking: &std::collections::BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let mut blocked = Vec::new();
    let mut logged = Vec::new();
    for line in findings.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let id = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches(':');
        if blocking.contains(id) {
            blocked.push(line.to_string());
        } else {
            logged.push(line.to_string());
        }
    }
    (blocked, logged)
}

/// Whether the task's rework rounds are over the cap (T-23):
/// `[conventions].review_max_rounds` (default 3) from the project config. The
/// round is the attempt the review settles.
fn rework_rounds_over_cap(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    attempt: i64,
) -> GearResult<bool> {
    let cap = tx
        .get_project(project)?
        .and_then(|row| crate::config::parse(&row.config_toml).ok())
        .map(|config| config.conventions.review_max_rounds)
        .unwrap_or(3);
    let round = tx.attempts_of_task(project, task)?.len() as u32;
    Ok(round > cap || attempt as u32 > cap)
}

/// One action item for the captain: the task's rework rounds are over the cap
/// (T-23). One per task: the tick must not fill the inbox with the same news.
fn tell_captain_cap(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    attempt: i64,
    line: &str,
    now: Ts,
) -> GearResult<()> {
    let Some(captain) = tx.captain_of(project)? else {
        return Ok(());
    };
    let already = tx
        .actionable_for(project, &captain.id, now)?
        .into_iter()
        .any(|message| message.body == line);
    if already {
        return Ok(());
    }
    tx.insert_message(&NewMessage {
        meta: None,
        project: project.to_string(),
        thread: format!("{task}:cap"),
        task: Some(task.to_string()),
        attempt: Some(attempt),
        sender: captain.id.clone(),
        sender_gen: captain.generation,
        recipient: captain.id.clone(),
        kind: MsgKind::Question,
        urgent: false,
        body: line.to_string(),
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created: now,
    })?;
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

/// Close **every actionable** review item of `task`/`attempt` in the captain's inbox (it is
/// handled now) and leave one silent System/Info line for the captain in the
/// first item's thread, or a fresh line when no item is there any more. A
/// duplicate review submission must not survive as an action item (R-67 P1).
///
/// The plan's apply uses it too (T-187): a `# Verdicts` line that settles an
/// attempt closes the review item of that attempt, so the captain does not
/// have to resolve it by hand.
pub fn resolve_review_items(
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
        meta: None,
        project: project.to_string(),
        thread,
        task: Some(task.to_string()),
        attempt: Some(attempt),
        sender: "engine".into(),
        sender_gen: 0,
        recipient: captain.id.clone(),
        kind: MsgKind::System,
        urgent: false,
        body: line.to_string(),
        reply_to,
        needs_ack: false,
        state: MsgState::Info,
        created: now,
    })?;
    Ok(())
}
