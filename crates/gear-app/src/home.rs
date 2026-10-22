//! `gear` home page (design/13). Read-only for business state; records one
//! ViewRecord of exactly what was displayed (evidence for op confirmation).

use crate::identity::Fingerprint;
use crate::Env;
use gear_model::{GearResult, Ts};
use gear_store::{Store, ViewRecord};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    /// Text printed to the agent (≈120 lines max).
    pub text: String,
    /// What was shown; written by [`record_view`] only after the text was printed.
    pub view: ViewRecord,
}

/// Build the home page for the caller's seat.
///
/// Slice 1 content:
/// - identity line: seat, role, generation;
/// - captain: the inbox batch from `mail::select_batch` (thread id, message ids,
///   original bodies, "+N more"), then unshown Reply/Instruction messages;
/// - lane/temp: unshown Reply/Instruction messages addressed to it (originals);
/// - footer: suggested next command.
/// "Unshown" = created after the seat's latest view that listed it.
pub fn build(store: &mut Store, env: &Env, fp: Option<&Fingerprint>, now: Ts) -> GearResult<Home> {
    todo!("slice1")
}

/// Persist the view. The CLI calls this **after** printing succeeded; if
/// printing fails the view is not recorded (design/13).
pub fn record_view(store: &mut Store, view: &ViewRecord) -> GearResult<()> {
    todo!("slice1")
}
