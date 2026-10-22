//! Continue / assign decisions (design/09). Pure: no store, no Herdr. The
//! engine builds a [`LaneView`] per lane seat each tick and turns the
//! [`Action`] into ops (08).

use crate::observe::SeatObs;
use gear_herdr::ScreenFlag;
use gear_model::{AttemptState, SeatRole, SeatState, Ts};
use gear_model::GearResult;
use gear_store::{AttemptRow, OpRow, SeatRow, TaskRow, Tx};

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
    // 4. Never type into a working agent.
    if !obs.idle {
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
/// `Continue <task> (contract v<version>). Task packet: <packet>. Report progress/blocked/done with gear send.`
pub fn continue_text(task: &str, version: i64, packet: &str) -> String {
    let _ = (task, version, packet);
    todo!("T-39")
}

/// design/09 "Delivery content", exactly:
/// `New task <task> (contract v<version>). Branch: <branch> (in your worktree). Task packet: <packet>. Read it before starting; report with gear send.`
pub fn assign_text(task: &str, version: i64, branch: &str, packet: &str) -> String {
    let _ = (task, version, branch, packet);
    todo!("T-39")
}

<<<<<<< HEAD
/// Build the [`LaneView`] of one lane seat from the store (read-only `tx`):
/// - `attempt`: the seat's open attempt (`open_attempts_of_seat`, first); if its
///   task is not Running/Verifying (e.g. Waiting) the view has **no attempt and
///   no ready tasks** — the continue preconditions of design/09 live here;
/// - `pending_op`: first of `slot_ops(seat)`;
/// - `last_deliver_at`: `created` of `last_op(seat, Deliver)`;
/// - `ready_tasks`: tasks of the project with `lane == seat.name`, state Ready,
///   priority ≠ `wont`, every dep Done — in rank order;
/// - `nudges_since_progress` 0 and `safe_for_next` false (sampling and the
///   design/06 safe conditions come later).
pub fn lane_view(tx: &Tx<'_>, seat: &SeatRow, obs: Option<SeatObs>, now: Ts) -> GearResult<LaneView> {
    let _ = (tx, seat, obs, now);
    todo!("T-41")
=======
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
            },
            presence: Presence::Present,
            status: Some(if idle { AgentStatus::Idle } else { AgentStatus::Working }),
            flags: BTreeSet::new(),
            idle,
        }
    }

    fn attempt(state: AttemptState) -> AttemptRow {
        AttemptRow {
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
            obs: Some(obs(true)),
            attempt,
            pending_op: None,
            last_deliver_at: Some(NOW - 1000),
            nudges_since_progress: 0,
            ready_tasks: vec![TaskRow {
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
                updated: NOW,
            }],
            safe_for_next: false,
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
>>>>>>> origin/runtime/t38-dispatch-delivered
}
