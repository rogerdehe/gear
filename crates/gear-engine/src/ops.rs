//! Side-effect operations: intend → execute → confirm (design/08).

use gear_herdr::{HerdrAct, Outcome};
use gear_model::state::require_transition;
use gear_model::{ErrorCode, GearError, GearResult, OpKind, OpState, Ts};
use gear_store::{BindingStatus, NewOp, OpRow, Store, Tx};

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
pub fn execute_intended(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    now: Ts,
) -> GearResult<Vec<(String, String)>> {
    // 1. In one transaction: re-check every Intended op of the project and
    //    move the ones that may run to Executing; the others become Void.
    let prepared = store.write(|tx| {
        let ops = tx.ops_in_state(project, OpState::Intended)?;
        let mut prepared = Vec::new();
        for op in ops {
            if op.kind != OpKind::Doorbell {
                // Slice 1 sends doorbells; the other kinds have no text yet.
                continue;
            }
            let item = check_op(tx, &op)?;
            match &item {
                Prepared::Run { id, .. } => {
                    require_transition(
                        OpState::Intended.can_transition(OpState::Executing),
                        "operation",
                        "intended",
                        "executing",
                    )?;
                    tx.set_op_state(project, id, OpState::Executing, None, now)?;
                }
                Prepared::Void { id, reason } => {
                    require_transition(
                        OpState::Intended.can_transition(OpState::Void),
                        "operation",
                        "intended",
                        "void",
                    )?;
                    tx.set_op_state(project, id, OpState::Void, Some(reason), now)?;
                }
            }
            prepared.push(item);
        }
        Ok(prepared)
    })?;

    // 2. Outside any transaction: call Herdr.
    let mut outcomes = Vec::with_capacity(prepared.len());
    for item in &prepared {
        outcomes.push(match item {
            Prepared::Run { pane_id, text, .. } => Some(herdr.prompt(pane_id, text)),
            Prepared::Void { .. } => None,
        });
    }

    // 3. In one transaction: record what Herdr answered.
    store.write(|tx| {
        let mut results = Vec::with_capacity(prepared.len());
        for (item, outcome) in prepared.iter().zip(outcomes) {
            let (state, result) = match (item, outcome) {
                // A voided op was written in step 1 and never ran.
                (Prepared::Void { .. }, _) => {
                    results.push((item.id().to_string(), OpState::Void.as_str().to_string()));
                    continue;
                }
                (Prepared::Run { .. }, Some(Outcome::Ok(()))) => {
                    (OpState::Submitted, None)
                }
                (Prepared::Run { .. }, Some(Outcome::Refused { code, message })) => {
                    (OpState::Failed, Some(format!("{code}: {message}")))
                }
                (Prepared::Run { .. }, Some(Outcome::Uncertain { reason })) => {
                    (OpState::Unknown, Some(reason))
                }
                (Prepared::Run { .. }, None) => {
                    return Err(GearError::new(
                        ErrorCode::BadFormat,
                        format!("operation {} has no outcome", item.id()),
                    ))
                }
            };
            require_transition(
                OpState::Executing.can_transition(state),
                "operation",
                "executing",
                state.as_str(),
            )?;
            tx.set_op_state(project, item.id(), state, result.as_deref(), now)?;
            results.push((item.id().to_string(), state.as_str().to_string()));
        }
        Ok(results)
    })
}

/// What to do with one Intended op after the guard checks.
enum Prepared {
    /// The op may run: send `text` to `pane_id`.
    Run { id: String, pane_id: String, text: String },
    /// The op must not run (stale seat, unreachable seat, no binding).
    Void { id: String, reason: String },
}

impl Prepared {
    fn id(&self) -> &str {
        match self {
            Prepared::Run { id, .. } => id,
            Prepared::Void { id, .. } => id,
        }
    }
}

/// Re-check an op against the seat's current state: the generation fixed at
/// intent time must still be the seat's, the seat must be reachable, and it
/// must still have an active binding (that is the pane the text goes to).
fn check_op(tx: &Tx<'_>, op: &OpRow) -> GearResult<Prepared> {
    let void = |reason: String| Ok(Prepared::Void { id: op.id.clone(), reason });

    let Some(seat) = tx.get_seat(&op.seat)? else {
        return void("the seat no longer exists".to_string());
    };
    if seat.generation != op.generation {
        return void(format!(
            "the seat's generation is {} now, the op fixed {}",
            seat.generation, op.generation
        ));
    }
    if !crate::observe::reachable(seat.state) {
        return void(format!("the seat is not reachable ({})", seat.state.as_str()));
    }
    let Some(binding) = tx.get_binding(&op.seat, op.generation)? else {
        return void("the seat has no binding for this generation".to_string());
    };
    if binding.status != BindingStatus::Active {
        return void("the seat's binding is not active".to_string());
    }
    let Some(text) = op.payload.get("text").and_then(serde_json::Value::as_str) else {
        return void("the op carries no text".to_string());
    };
    Ok(Prepared::Run {
        id: op.id.clone(),
        pane_id: binding.pane_id.clone(),
        text: text.to_string(),
    })
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
