//! Continue / assign decisions (design/09). Pure: no store, no Herdr. The
//! engine builds a [`LaneView`] per lane seat each tick and turns the
//! [`Action`] into ops (08).

use crate::observe::SeatObs;
use gear_model::Ts;
use gear_store::{AttemptRow, OpRow, TaskRow};

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
/// 7. attempt Started/Stopped → Continue, or Escalate when
///    `nudges_since_progress >= nudge_budget`;
///    attempt Submitted → Assign the first ready task if `safe_for_next`, else None;
///    no attempt → Assign the first ready task, else None;
///    seat Retiring never Assigns (Continue allowed);
///    temp seats (role Temp) never Assign.
/// `reason` texts are free-form but non-empty.
pub fn decide(v: &LaneView, cfg: &DispatchCfg, now: Ts) -> Action {
    let _ = (v, cfg, now);
    todo!("T-35")
}
