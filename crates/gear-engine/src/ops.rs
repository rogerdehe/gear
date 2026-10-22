//! Side-effect operations: intend → execute → confirm (design/08).

use gear_herdr::HerdrAct;
use gear_model::{GearResult, OpKind, Ts};
use gear_store::{NewOp, OpRow, Store, Tx};

/// Write Intended ops for the doorbell actions (one transaction). Each op
/// fixes seat, generation and the covered message ids in its payload.
pub fn intend_doorbells(tx: &Tx<'_>, actions: &[crate::doorbell::DoorbellAction], now: Ts) -> GearResult<Vec<OpRow>> {
    let mut ops = Vec::with_capacity(actions.len());
    for action in actions {
        let payload = serde_json::json!({ "text": action.text, "covers": action.covers });
        ops.push(tx.insert_op(&NewOp {
            project: action.project.clone(),
            seat: action.seat.clone(),
            generation: action.generation,
            kind: OpKind::Doorbell,
            task: None,
            attempt: None,
            ref_msg: None,
            plan_version: None,
            payload,
            created: now,
        })?);
    }
    Ok(ops)
}

/// Execute every Intended op of `project`:
/// - in a tx: re-check the seat's current generation/state still match the
///   op (else → Void) and move it to Executing;
/// - outside any tx: call Herdr;
/// - in a tx: Ok → Submitted, Refused → Failed, Uncertain → Unknown.
/// Returns (op id, final state) pairs.
pub fn execute_intended(store: &mut Store, herdr: &mut dyn HerdrAct, project: &str, now: Ts) -> GearResult<Vec<(String, String)>> {
    todo!("slice1")
}

/// Settle Submitted/Unknown doorbell ops using ViewRecords: an op is Confirmed
/// only when a view of the same seat+generation, at or after the op time,
/// lists every message id in the op's `covers`. Otherwise bump its check
/// budget; when the budget is exhausted, flag the seat (no resend).
/// Returns (confirmed op ids, flagged seat ids).
pub fn confirm_pending(store: &mut Store, project: &str, budget: i64, now: Ts) -> GearResult<(Vec<String>, Vec<String>)> {
    todo!("slice1")
}

/// On engine start: every Executing op becomes Unknown (the previous process
/// died mid-call). Nothing is re-sent.
pub fn recover_on_start(store: &mut Store, now: Ts) -> GearResult<Vec<String>> {
    todo!("slice1")
}
