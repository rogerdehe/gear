//! Continue / assign decisions (design/09). Pure: no store, no Herdr. The
//! engine builds a [`LaneView`] per lane seat each tick and turns the
//! [`Action`] into ops (08).

use crate::delivery::FailedDelivery;
use crate::observe::SeatObs;
use gear_herdr::{AgentStatus, ScreenFlag};
use gear_model::{AttemptState, MsgKind, MsgState, OpKind, SeatRole, SeatState, TaskState, Ts};
use gear_model::GearResult;
use gear_store::{AttemptRow, NewMessage, OpRow, SeatRow, TaskRow, Tx};

#[derive(Debug, Clone, PartialEq)]
pub struct LaneView {
    /// This tick's observation (seat row inside); None = observation failed.
    pub obs: Option<SeatObs>,
    /// The seat's current non-terminal attempt, if any.
    pub attempt: Option<AttemptRow>,
    /// Any deliver/doorbell op for this seat in Intended/Executing/Submitted/Unknown.
    pub pending_op: Option<OpRow>,
    pub last_deliver_at: Option<Ts>,
    /// Continues sent since the last verifiable progress (design/09).
    pub nudges_since_progress: u32,
    /// This lane's Ready tasks in rank order (readiness already checked).
    pub ready_tasks: Vec<TaskRow>,
    /// design/06 "safe conditions for taking the next item" hold for a Submitted attempt.
    pub safe_for_next: bool,
    /// An in-place model switch is under way (T-33, F6): no prompt may land in
    /// the agent while its picker or dialog is open.
    pub switch_pending: bool,
    /// The last delivery to this seat that Herdr accepted and the lane never
    /// confirmed (T-97). It is delivered once more in a new agent session — or
    /// at once when a switch swallowed it — never every tick.
    pub failed_delivery: Option<FailedDelivery>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchCfg {
    pub cooldown_s: i64,
    /// Continues without verifiable progress before escalating (default 5).
    pub nudge_budget: u32,
}

impl Default for DispatchCfg {
    fn default() -> Self {
        DispatchCfg { cooldown_s: 90, nudge_budget: 5 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None { reason: String },
    /// Same task, same attempt.
    Continue { task: String },
    /// First ready task.
    Assign { task: String },
    /// The task's contract moved on; the lane adopts it at its next view.
    Adopt { task: String, version: i64 },
    /// Tell the captain; stop nudging.
    Escalate { task: String, reason: String },
}

/// The decision order of design/09 "Decision order", first match wins:
/// 1. seat state not Active/Standby/Retiring → None;
/// 2. `obs` None → None;
/// 3. `pending_op` present → None (never resend while a delivery is unsettled);
/// 4. agent not idle (`obs.idle` false) → None;
/// 5. screen flag RateLimit/Quota/Auth → None;
/// 6. `now - last_deliver_at < cooldown_s` → None;
/// 7. attempt Started/Stopped/Delivered → Continue, or Escalate when
///    `nudges_since_progress >= nudge_budget` (a Delivered attempt was
///    confirmed but the agent never started, so it is nudged like a stopped
///    one);
///    a Started attempt gets no delivery at all (design §10.3: one packet per
///    round);
///    attempt Submitted → Assign the first ready task if `safe_for_next`, else None;
///    attempt Unknown → None (an unknown outcome is never retried);
///    no attempt (or a terminal one, which the view contract excludes) →
///    Assign the first ready task, else None;
///    seat Retiring never Assigns (Continue allowed);
///    temp seats (role Temp) never Assign.
/// `reason` texts are free-form but non-empty.
pub fn decide(v: &LaneView, cfg: &DispatchCfg, now: Ts) -> Action {
    // 1–2. The seat must be one that may take work, and must have been
    //      observed at all.
    let Some(obs) = &v.obs else {
        return none("observation failed this tick");
    };
    let seat = &obs.seat;
    if !matches!(seat.state, SeatState::Active | SeatState::Standby | SeatState::Retiring) {
        return none("the seat may not take work in its current state");
    }
    // 3. A delivery that is not settled yet blocks everything.
    if v.pending_op.is_some() {
        return none("a delivery for this seat is still unsettled");
    }
    // 3b. A model switch is typing into this agent (T-33, F6): its picker,
    //     its variant dialog and the bar check own the pane until it confirms.
    if v.switch_pending {
        return none("a model switch is under way for this seat");
    }
    // 4. Never type into a working agent. A blocked one is not working: it
    //    may be handed work, and the delivery waits for the dialog to clear
    //    instead of failing every tick (T-68).
    let blocked = obs.status == Some(AgentStatus::Blocked);
    if !obs.idle && !blocked {
        return none("the agent is not idle");
    }
    // 5. A rate limit / quota / auth wall is not nudged through.
    if obs.flags.iter().any(|flag| {
        matches!(flag, ScreenFlag::RateLimit | ScreenFlag::Quota | ScreenFlag::Auth)
    }) {
        return none("the screen shows a ratelimit, quota or auth problem");
    }
    // 6. Respected since the last delivery (the design's 90 s default).
    if let Some(last) = v.last_deliver_at {
        if now - last < cfg.cooldown_s {
            return none("cooldown since the last delivery");
        }
    }
    // 7. Continue the current attempt, or assign the next ready task.
    let may_assign = seat.state != SeatState::Retiring && seat.role != SeatRole::Temp;
    match &v.attempt {
        Some(attempt) => match attempt.state {
            // Started/Stopped: the agent worked and stopped. Delivered: the
            // delivery was confirmed but the agent never started (R-31) — it
            // is nudged the same way instead of waiting forever.
            AttemptState::Started | AttemptState::Stopped | AttemptState::Delivered => {
                // T-97: a delivery that was never confirmed — the assignment
                // or a continue — is not typed into the same agent session
                // again. It goes once more when the pane reports a new session
                // (a hand restart) or the seat has a new generation (a restart
                // GEAR did), and at once when a model switch swallowed it.
                if let Some(failed) = &v.failed_delivery {
                    if failed.task == attempt.task && failed.attempt == attempt.n {
                        let new_session = obs.session != failed.session;
                        let new_generation = seat.generation != failed.generation;
                        if !(failed.retry_now || new_session || new_generation) {
                            return none(
                                "the delivery was not confirmed in this agent session; it goes again in a new one",
                            );
                        }
                    }
                }
                // A contract update the lane has not adopted yet is named once,
                // before any continue (T-8, design/06). The marker holds the
                // newest version, so two bumps still give one delivery.
                if let Some(version) = attempt.pending_update {
                    if version > attempt.adopted_version {
                        return Action::Adopt {
                            task: attempt.task.clone(),
                            version,
                        };
                    }
                }
                // Design §10.3: an attempt that already Started got its packet
                // and the lane checked in — the engine never delivers it a
                // second one. The lane that stops working is reached by the
                // overrun question of design §5.2, not by a second packet.
                if attempt.state == AttemptState::Started {
                    return none("the attempt already started; no second packet");
                }
                if v.nudges_since_progress >= cfg.nudge_budget {
                    Action::Escalate {
                        task: attempt.task.clone(),
                        reason: format!(
                            "no verifiable progress after {} continues",
                            v.nudges_since_progress
                        ),
                    }
                } else {
                    Action::Continue { task: attempt.task.clone() }
                }
            }
            AttemptState::Submitted => {
                // Do not re-implement a submitted task; a second task only at
                // a safe point (design/06).
                if !v.safe_for_next {
                    return none("the submitted attempt is not at a safe point");
                }
                assign_first(&v.ready_tasks, may_assign)
            }
            // An unknown outcome is never retried, and a second task would
            // break "one task per seat" (design/08).
            AttemptState::Unknown => none("the current attempt's outcome is unknown"),
            // A terminal attempt belongs to a finished task; the seat is free.
            AttemptState::Accepted | AttemptState::Rejected | AttemptState::Abandoned => {
                assign_first(&v.ready_tasks, may_assign)
            }
        },
        None => assign_first(&v.ready_tasks, may_assign),
    }
}

/// A `None` action with a reason (design/09 asks for one).
fn none(reason: &str) -> Action {
    Action::None { reason: reason.to_string() }
}

/// A delivery's text, with the project standard's line when the project has
/// one (T-84).
fn with_standard(text: String, standard: &Option<String>) -> String {
    match standard {
        Some(line) => format!("{text}\n{line}"),
        None => text,
    }
}

/// The first ready task, if this seat may take one at all.
fn assign_first(ready: &[TaskRow], may_assign: bool) -> Action {
    if !may_assign {
        return none("a retiring or temp seat never takes a new assignment");
    }
    match ready.first() {
        Some(task) => Action::Assign { task: task.id.clone() },
        None => none("no ready task for this lane"),
    }
}

/// A decided delivery, ready to become a Deliver op (08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliverAction {
    pub project: String,
    pub seat: String,
    pub generation: i64,
    pub task: String,
    pub attempt: i64,
    pub text: String,
}

/// design/09 "Delivery content", exactly:
/// `Continue <task> (contract v<version>). Run `gear` first (…). Task packet: <packet>. Report progress/blocked/done with gear send.`
pub fn continue_text(task: &str, version: i64, packet: &str) -> String {
    format!(
        "Continue {task} (contract v{version}). Run `gear` first (it records that you got this). Task packet: {packet}. Report progress/blocked/done with gear send."
    )
}

/// design/09 "Delivery content", exactly:
/// `New task <task> (contract v<version>). Branch: <branch> (in your worktree). Run `gear` first (…). Task packet: <packet>. Read it before starting; report with gear send. After your result, end your turn: …`
pub fn assign_text(task: &str, version: i64, branch: &str, packet: &str) -> String {
    format!(
        "New task {task} (contract v{version}). Branch: {branch} (in your worktree). Run `gear` first (it records that you got this). Task packet: {packet}. Read it before starting; report with gear send. After your result, end your turn: the next task comes when you are idle."
    )
}

/// The one line of a contract update (T-8), exactly as the contract puts it:
/// `T-<n> contract changed to v<k>: run `gear` and `gear --show T-<n>``.
pub fn contract_changed_text(task: &str, version: i64) -> String {
    format!("{task} contract changed to v{version}: run `gear` and `gear --show {task}`")
}

/// Build the [`LaneView`] of one lane seat from the store (read-only `tx`):
/// - `attempt`: the seat's open attempt (`open_attempts_of_seat`, first); if its
///   task is not Running/Verifying (e.g. Waiting) the view has **no attempt and
///   no ready tasks** — the continue preconditions of design/09 live here;
/// - `pending_op`: first of `slot_ops(seat)`;
/// - `last_deliver_at`: `created` of `last_op(seat, Deliver)`;
/// - `ready_tasks`: tasks of the project with `lane == seat.name`, state Ready,
///   priority ≠ `wont`, every dep Done — in rank order;
/// - `nudges_since_progress` 0; `safe_for_next` true when every open attempt
///   is handed in (Submitted): review runs in the background (never-stop).
pub fn lane_view(tx: &Tx<'_>, seat: &SeatRow, obs: Option<SeatObs>, now: Ts) -> GearResult<LaneView> {
    let _ = now; // the design/09 cooldown uses last_deliver_at, not this tick's time

    // The open attempt, but only while its task may still be worked on: a
    // Waiting task (or one that is not Running/Verifying) blocks both the
    // continue and any assign (design/09, R-31).
    // Never-stop (PRD §0, lesson L-44): a handed-in attempt (Submitted, its
    // candidate pushed) waits for review in the background and is a safe
    // point by definition. The attempt the lane is working on now — any open
    // one that is not Submitted — comes first; only when every open attempt is
    // handed in does the view carry a Submitted one, and then the next task
    // may be assigned.
    let mut open = tx.open_attempts_of_seat(&seat.id)?;
    open.sort_by_key(|a| a.state == AttemptState::Submitted);
    let safe_for_next = !open.is_empty() && open.iter().all(|a| a.state == AttemptState::Submitted);
    let attempt = match open.into_iter().next() {
        Some(attempt) => {
            let task = tx.get_task(&seat.project, &attempt.task)?;
            if task.as_ref().is_some_and(|task| task.wait_on.is_some()) {
                return Ok(empty_view(obs));
            }
            let continues = task
                .map(|task| match task.state {
                    TaskState::Running | TaskState::Verifying => true,
                    // A delivery that was never confirmed never started the
                    // task: the attempt is the seat's open one all the same,
                    // and the delivery goes again (T-97).
                    TaskState::Ready => attempt.state == AttemptState::Delivered,
                    _ => false,
                })
                .unwrap_or(false);
            if !continues {
                return Ok(empty_view(obs));
            }
            Some(attempt)
        }
        None => None,
    };

    Ok(LaneView {
        obs,
        attempt,
        pending_op: tx.slot_ops(&seat.id)?.into_iter().next(),
        last_deliver_at: tx.last_op(&seat.id, OpKind::Deliver)?.map(|op| op.created),
        nudges_since_progress: 0,
        ready_tasks: ready_tasks_of(tx, seat)?,
        safe_for_next,
        // The plan is durable in meta (T-33): a switch in progress is not
        // forgotten while the quota line is gone.
        switch_pending: tx
            .get_meta(&format!("engine.switch.{}", seat.id))?
            .map(|value| !value.is_empty())
            .unwrap_or(false),
        failed_delivery: crate::delivery::failed(tx, &seat.id)?,
    })
}

/// A view with nothing to do: no attempt, no pending delivery, no ready task.
fn empty_view(obs: Option<SeatObs>) -> LaneView {
    LaneView {
        obs,
        attempt: None,
        pending_op: None,
        last_deliver_at: None,
        nudges_since_progress: 0,
        ready_tasks: Vec::new(),
        safe_for_next: false,
        switch_pending: false,
        failed_delivery: None,
    }
}

/// This lane's Ready tasks in rank order: not `wont`, every dependency Done.
pub(crate) fn ready_tasks_of(tx: &Tx<'_>, seat: &SeatRow) -> GearResult<Vec<TaskRow>> {
    use std::collections::BTreeSet;
    let tasks = tx.tasks_of_project(&seat.project)?;
    let done: BTreeSet<String> = tasks
        .iter()
        .filter(|task| task.state == TaskState::Done)
        .map(|task| task.id.clone())
        .collect();
    Ok(tasks
        .into_iter()
        .filter(|task| task.lane.as_deref() == Some(seat.name.as_str()))
        .filter(|task| task.state == TaskState::Ready)
        .filter(|task| task.priority != "wont")
        .filter(|task| task.deps.iter().all(|dep| done.contains(dep)))
        .collect())
}

/// Where a lane reads a task packet: the CLI shows the rendered packet.
pub fn packet_ref(task: &str) -> String {
    format!("`gear --show {task}`")
}

/// The task branch a lane works on (17 §3): `<lane>/t<n>` (no slug in slice 2).
pub fn task_branch(lane: &str, task: &str) -> String {
    format!("{lane}/{}", task.to_lowercase().replace('-', ""))
}

/// One decision per Lane-role seat of `project` (other roles skipped), in
/// seat-id order: `lane_view(tx, seat, obs, now)` with that seat's entry from
/// `observed` (None when absent) → `decide`.
pub fn plan_dispatch(tx: &Tx<'_>, project: &str, observed: &[SeatObs], cfg: &DispatchCfg, now: Ts) -> GearResult<Vec<(SeatRow, Action)>> {
    let mut decisions = Vec::new();
    for seat in tx.seats_of_project(project)? {
        if seat.role != SeatRole::Lane {
            // The captain has no task queue (design/09).
            continue;
        }
        // The seat's entry from this tick's observation, if it was observed.
        let obs = observed.iter().find(|o| o.seat.id == seat.id).cloned();
        let view = lane_view(tx, &seat, obs, now)?;
        let action = decide(&view, cfg, now);
        let action = if matches!(action, Action::Assign { .. }) && tx.captain_quota_paused(project)? {
            none("captain quota: current tasks may finish; new tasks are paused")
        } else { action };
        decisions.push((seat, action));
    }
    Ok(decisions)
}

/// Turn decisions into Deliver ops in the caller's transaction:
/// - Assign → `gear_app::tasks::start_attempt(tx, project, task, seat.id, seat.generation, now)`
///   then a delivery with `assign_text(task, contract_version, task_branch(seat.name, task), packet_ref(task))`;
/// - Continue → a delivery for the seat's open attempt with `continue_text(task, adopted_version, packet_ref(task))`;
/// - None / Escalate → nothing.
/// Returns the Intended ops (via `ops::intend_deliveries`).
pub fn intend_dispatch(tx: &Tx<'_>, project: &str, actions: &[(SeatRow, Action)], now: Ts) -> GearResult<Vec<OpRow>> {
    let mut deliveries: Vec<DeliverAction> = Vec::new();
    // T-84: the project standard travels with the assignment and the continue.
    let standard = gear_app::standard::line_for(tx, project)?;
    for (seat, action) in actions {
        match action {
            Action::Assign { task } => {
                if tx.captain_quota_paused(project)? { continue; }
                // The attempt exists before the delivery that announces it.
                let attempt =
                    gear_app::tasks::start_attempt(tx, project, task, &seat.id, seat.generation, now)?;
                deliveries.push(DeliverAction {
                    project: project.to_string(),
                    seat: seat.id.clone(),
                    generation: seat.generation,
                    task: task.clone(),
                    attempt: attempt.n,
                    text: with_standard(
                        assign_text(
                            task,
                            attempt.contract_version,
                            &task_branch(&seat.name, task),
                            &packet_ref(task),
                        ),
                        &standard,
                    ),
                });
            }
            Action::Continue { task } => {
                // Continue the attempt that is actually open, never a guess.
                let Some(attempt) = tx
                    .open_attempts_of_seat(&seat.id)?
                    .into_iter()
                    .find(|attempt| attempt.task == *task)
                else {
                    continue;
                };
                deliveries.push(DeliverAction {
                    project: project.to_string(),
                    seat: seat.id.clone(),
                    generation: seat.generation,
                    task: task.clone(),
                    attempt: attempt.n,
                    text: with_standard(
                        continue_text(task, attempt.adopted_version, &packet_ref(task)),
                        &standard,
                    ),
                });
            }
            Action::Adopt { task, version } => {
                // One line for the contract update; the lane's next view adopts
                // it (T-8). The attempt is the open one, never a guess.
                let Some(attempt) = tx
                    .open_attempts_of_seat(&seat.id)?
                    .into_iter()
                    .find(|attempt| attempt.task == *task)
                else {
                    continue;
                };
                if attempt.state == AttemptState::Started {
                    // Design §10.3: an attempt that already started got its
                    // packet — the engine never delivers it a second one. The
                    // contract change goes as an instruction message, which the
                    // doorbell rings; the lane adopts the new contract at its
                    // next view (T-8).
                    contract_notice(tx, seat, task, attempt.n, *version, now)?;
                } else {
                    deliveries.push(DeliverAction {
                        project: project.to_string(),
                        seat: seat.id.clone(),
                        generation: seat.generation,
                        task: task.clone(),
                        attempt: attempt.n,
                        text: contract_changed_text(task, *version),
                    });
                }
            }
            // A seat that must wait, or one handed to the captain: no delivery.
            Action::None { .. } | Action::Escalate { .. } => {}
        }
    }
    let ops = crate::ops::intend_deliveries(tx, &deliveries, now)?;
    // The attempt is on its way again: the record of the failed delivery has
    // done its job (T-97). A second failure writes a fresh one.
    for delivery in &deliveries {
        crate::delivery::clear(tx, &delivery.seat)?;
    }
    Ok(ops)
}

/// The notice that the contract of an open attempt changed, for an attempt
/// that already started (T-8, design §10.3).
///
/// It is an instruction message, not a delivery: the doorbell rings it (at
/// once, so a working lane reads it), and the lane adopts the new contract at
/// its next view. One notice per task and version: the lane's answer closes
/// it, so the engine does not write a second one every tick.
fn contract_notice(
    tx: &Tx<'_>,
    seat: &SeatRow,
    task: &str,
    attempt: i64,
    version: i64,
    now: Ts,
) -> GearResult<bool> {
    let thread = format!("contract:{task}:{version}");
    let open = tx
        .thread_messages(&seat.project, &thread)?
        .into_iter()
        .any(|message| matches!(message.state, MsgState::Open | MsgState::Waiting));
    if open {
        return Ok(false);
    }
    tx.insert_message(&NewMessage {
        project: seat.project.clone(),
        thread,
        task: Some(task.to_string()),
        attempt: Some(attempt),
        sender: "engine".to_string(),
        sender_gen: 0,
        recipient: seat.id.clone(),
        kind: MsgKind::Instruction,
        urgent: true,
        body: contract_changed_text(task, version),
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created: now,
        meta: None,
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::Presence;
    use gear_herdr::AgentStatus;
    use gear_model::{SeatRole as Role, SeatState as State};
    use gear_store::{SeatRow, TaskRow};
    use std::collections::BTreeSet;

    const NOW: i64 = 1_800_000_000;

    fn obs(idle: bool) -> SeatObs {
        SeatObs {
            seat: SeatRow {
                id: "p:heat".into(),
                project: "p".into(),
                role: Role::Lane,
                name: "heat".into(),
                profile: "opencode".into(),
                generation: 1,
                state: State::Active,
                state_since: NOW - 1000,
                state_reason: None,
                task_id: None,
            },
            presence: Presence::Present,
            status: Some(if idle { AgentStatus::Idle } else { AgentStatus::Working }),
            flags: BTreeSet::new(),
            idle,
            screen: None,
            session: None,
        }
    }

    fn attempt(state: AttemptState) -> AttemptRow {
        AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(),
            task: "T-1".into(),
            n: 1,
            seat: "p:heat".into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state,
            candidate: None,
            report: None,
            safe_point: None,
            started: NOW - 500,
            updated: NOW - 500,
        }
    }

    fn view(attempt: Option<AttemptRow>) -> LaneView {
        LaneView {
            switch_pending: false,
            obs: Some(obs(true)),
            attempt,
            pending_op: None,
            last_deliver_at: Some(NOW - 1000),
            nudges_since_progress: 0,
            ready_tasks: vec![TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-2".into(),
                rank: 1,
                state: gear_model::TaskState::Ready,
                priority: "must".into(),
                lane: Some("heat".into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: None,
                 issue: None,updated: NOW,
            }],
            safe_for_next: false,
            failed_delivery: None,
        }
    }

    #[test]
    fn delivered_attempt_is_continued() {
        // Delivery confirmed but the agent never started (design/09 gap, R-31):
        // nudge it like a started attempt instead of leaving it silent.
        let cfg = DispatchCfg::default();
        let action = decide(&view(Some(attempt(AttemptState::Delivered))), &cfg, NOW);
        assert_eq!(action, Action::Continue { task: "T-1".into() });

        let mut v = view(Some(attempt(AttemptState::Delivered)));
        v.nudges_since_progress = cfg.nudge_budget;
        match decide(&v, &cfg, NOW) {
            Action::Escalate { task, reason } => {
                assert_eq!(task, "T-1");
                assert_eq!(
                    reason,
                    format!("no verifiable progress after {} continues", cfg.nudge_budget)
                );
            }
            other => panic!("expected an escalation, got {other:?}"),
        }
    }

    #[test]
    fn unknown_attempt_is_not_nudged() {
        // An unknown outcome is never retried (design/08).
        let action = decide(&view(Some(attempt(AttemptState::Unknown))), &DispatchCfg::default(), NOW);
        assert!(matches!(action, Action::None { .. }));
    }
}
