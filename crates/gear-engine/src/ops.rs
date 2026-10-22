//! Side-effect operations: intend → execute → confirm (design/08).

use gear_herdr::HerdrAct;
use gear_model::state::require_transition;
use gear_model::{GearResult, OpKind, OpState, Ts};
use gear_store::{NewOp, OpRow, Store, Tx};

/// Reason stored with an op that the engine found Executing after a restart.
const RECOVERED_REASON: &str = "engine restarted before the call finished";

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
pub fn confirm_pending(
    store: &mut Store,
    project: &str,
    budget: i64,
    now: Ts,
) -> GearResult<(Vec<String>, Vec<String>)> {
    store.write(|tx| {
        let mut confirmed = Vec::new();
        let mut flagged: Vec<String> = Vec::new();
        for state in [OpState::Submitted, OpState::Unknown] {
            for op in tx.ops_in_state(project, state)? {
                if op.kind != OpKind::Doorbell {
                    // Slice 1 settles doorbells; the other kinds have their
                    // own evidence rules.
                    continue;
                }
                if shown_in_a_view(tx, &op)? {
                    require_transition(
                        state.can_transition(OpState::Confirmed),
                        "operation",
                        state.as_str(),
                        "confirmed",
                    )?;
                    tx.set_op_state(project, &op.id, OpState::Confirmed, None, now)?;
                    confirmed.push(op.id.clone());
                    continue;
                }
                // No evidence yet: the op waits for the next round. Its budget
                // counts the checks; when it reaches the limit the seat needs
                // a human check (the op is not resent).
                let used = tx.bump_op_check_budget(project, &op.id, now)?;
                if used == budget && !flagged.contains(&op.seat) {
                    flagged.push(op.seat.clone());
                }
            }
        }
        Ok((confirmed, flagged))
    })
}

/// Whether a view of the seat's current generation shows every message id the
/// op covers. The view must be at or after the op's time: a view from before
/// the doorbell went out proves nothing.
fn shown_in_a_view(tx: &Tx<'_>, op: &OpRow) -> GearResult<bool> {
    let covers: Vec<&str> = op
        .payload
        .get("covers")
        .and_then(|c| c.as_array())
        .map(|ids| ids.iter().filter_map(|id| id.as_str()).collect())
        .unwrap_or_default();
    if covers.is_empty() {
        // Nothing to show: no view can be linked to this op.
        return Ok(false);
    }
    let views = tx.views_since(&op.seat, op.generation, op.created)?;
    Ok(views.iter().any(|view| {
        covers
            .iter()
            .all(|id| view.shown_msgs.iter().any(|shown| shown == id))
    }))
}

/// On engine start: every Executing op becomes Unknown (the previous process
/// died mid-call). Nothing is re-sent.
pub fn recover_on_start(store: &mut Store, now: Ts) -> GearResult<Vec<String>> {
    store.write(|tx| {
        let mut recovered = Vec::new();
        for project in tx.project_ids()? {
            for op in tx.ops_in_state(&project, OpState::Executing)? {
                require_transition(
                    OpState::Executing.can_transition(OpState::Unknown),
                    "operation",
                    "executing",
                    "unknown",
                )?;
                tx.set_op_state(&op.project, &op.id, OpState::Unknown, Some(RECOVERED_REASON), now)?;
                recovered.push(op.id);
            }
        }
        Ok(recovered)
    })
}
