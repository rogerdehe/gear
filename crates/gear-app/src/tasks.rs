//! Task and attempt state changes (design/06). Transaction-level: every
//! function takes the caller's `Tx` (plan apply, `gear send --kind result`, the
//! engine) and re-reads what it checks. Transitions go through
//! `TaskState::can_transition` / `AttemptState::can_transition` →
//! `ErrorCode::BadTransition`; a missing task/attempt → `NoTarget`.

use gear_model::{GearResult, Ts};
use gear_store::{AttemptRow, Tx};

/// Create attempt `n = last + 1` for `task` on `seat_id` (state Delivered,
/// `contract_version = adopted_version =` the task's current contract version).
/// Preconditions (else `Forbidden`, detail says which): task Ready; every dep
/// Done; seat state Active or Standby with `generation` = the seat's current
/// generation; the seat has no in-progress attempt. The task stays Ready.
pub fn start_attempt(tx: &Tx<'_>, project: &str, task: &str, seat_id: &str, generation: i64, now: Ts) -> GearResult<AttemptRow> {
    let _ = (tx, project, task, seat_id, generation, now);
    todo!("T-42")
}

/// Delivery confirmed: attempt Delivered → Started, task Ready → Running.
pub fn on_delivery_confirmed(tx: &Tx<'_>, project: &str, task: &str, n: i64, now: Ts) -> GearResult<()> {
    let _ = (tx, project, task, n, now);
    todo!("T-42")
}

/// `gear send --kind result`: the exact attempt, under its adopted contract
/// version (else `BadFormat`). Attempt Started/Stopped/Unknown → Submitted with
/// candidate and report; task Running → Verifying.
pub fn submit_result(tx: &Tx<'_>, project: &str, task: &str, n: i64, contract_version: i64, candidate: &str, report: &str, now: Ts) -> GearResult<()> {
    let _ = (tx, project, task, n, contract_version, candidate, report, now);
    todo!("T-42")
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
