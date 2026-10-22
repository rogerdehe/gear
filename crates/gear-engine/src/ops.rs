//! Side-effect operations: intend → execute → confirm (design/08).

use gear_herdr::{HerdrAct, Outcome};
use gear_model::state::require_transition;
use gear_model::{AttemptState, ErrorCode, GearError, GearResult, OpKind, OpState, Ts};
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

/// One Deliver op per action (state Intended): `task`/`attempt` set, payload
/// `{ "text": <text> }`. Same transaction rules as [`intend_doorbells`].
pub fn intend_deliveries(tx: &Tx<'_>, actions: &[crate::dispatch::DeliverAction], now: Ts) -> GearResult<Vec<OpRow>> {
    let mut ops = Vec::with_capacity(actions.len());
    for action in actions {
        let payload = serde_json::json!({ "text": action.text });
        ops.push(tx.insert_op(&NewOp {
            project: action.project.clone(),
            seat: action.seat.clone(),
            generation: action.generation,
            kind: OpKind::Deliver,
            task: Some(action.task.clone()),
            attempt: Some(action.attempt),
            ref_msg: None,
            plan_version: None,
            payload,
            created: now,
        })?);
    }
    Ok(ops)
}

/// Execute every Intended **doorbell or deliver** op of `project`:
/// - in a tx: re-check the seat's current generation/state still match the
///   op (else → Void) and move it to Executing; a delivery is also voided
///   when its task/attempt is no longer the seat's open attempt;
/// - outside any tx: call Herdr;
/// - in a tx: Ok → Submitted, Refused → Failed, Uncertain → Unknown.
/// Returns (op id, final state) pairs.
///
/// Doorbells and deliveries both carry their text in `payload.text` and are
/// drained here; onboard/release/title/notify have no text yet and stay
/// Intended until their own packets.
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
            if !matches!(op.kind, OpKind::Doorbell | OpKind::Deliver) {
                // Onboard/release/title/notify have no text yet.
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
/// A delivery must additionally still belong to the seat's open attempt: an
/// attempt that was settled, abandoned or replaced must not be nudged (R-35).
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
    if op.kind == OpKind::Deliver {
        // The attempt this delivery was written for must still be the seat's
        // open attempt. Stale: a newer/other attempt of the same task is open,
        // or this attempt already settled (accepted/rejected/abandoned).
        let (Some(task), Some(attempt)) = (op.task.as_deref(), op.attempt) else {
            return void("the deliver op names no task or attempt".to_string());
        };
        let open = tx.open_attempts_of_seat(&op.seat)?;
        if !open.iter().any(|a| a.task == task && a.n == attempt) {
            let superseded = open.iter().any(|a| a.task == task && a.n != attempt);
            let settled = tx
                .get_attempt(&op.project, task, attempt)?
                .map(|a| a.state.is_terminal())
                .unwrap_or(false);
            if superseded || settled {
                return void(format!(
                    "task {task} attempt {attempt} is no longer the seat's open attempt"
                ));
            }
            // No attempt row at all: the op was intended before the attempt
            // was recorded, so there is nothing to compare it with.
        }
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
/// only when a view of the same seat+generation, at or after the op's
/// `created` (intent) time, lists every message id in the op's `covers`.
/// Evidence counts from intent, not from the moment the text reached the pane:
/// a view recorded after intent means the seat looked at its home page, which
/// is what the doorbell is for. The view's time is stored on the op (`result`)
/// as the evidence that settled it. Without evidence the op's check budget is
/// bumped; when it is exhausted the seat is flagged (no resend).
/// Deliver ops (T-48): Confirmed when a view of the same seat+generation at or
/// after `created` shows `shown_task == op.task` and `shown_attempt ==
/// op.attempt`; on confirmation an attempt still in Delivered goes through
/// `gear_app::tasks::on_delivery_confirmed` in the same transaction. Budget and
/// flagging as for doorbells.
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
                let evidence = match op.kind {
                    // A doorbell is proven by the message ids it carried.
                    OpKind::Doorbell => shown_in_a_view(tx, &op)?,
                    // A delivery is proven by the task/attempt it announced.
                    OpKind::Deliver => shown_delivery(tx, &op)?,
                    // Onboard/release/title/notify have their own evidence.
                    _ => None,
                };
                if let Some(view_ts) = evidence {
                    require_transition(
                        state.can_transition(OpState::Confirmed),
                        "operation",
                        state.as_str(),
                        "confirmed",
                    )?;
                    // design/08: confirm(op, evidence) — keep the pointer.
                    let evidence = format!("view {}/{} @ {view_ts}", op.seat, op.generation);
                    tx.set_op_state(project, &op.id, OpState::Confirmed, Some(&evidence), now)?;
                    if op.kind == OpKind::Deliver {
                        // Design/06: the confirmed delivery starts the attempt.
                        start_confirmed_attempt(tx, project, &op, now)?;
                    }
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

/// The time of the view that shows every message id the op covers, if any.
/// Only a view of the seat's current generation, at or after the op's `created`
/// time, counts: a view from before the doorbell was intended proves nothing.
fn shown_in_a_view(tx: &Tx<'_>, op: &OpRow) -> GearResult<Option<Ts>> {
    let covers: Vec<&str> = op
        .payload
        .get("covers")
        .and_then(|c| c.as_array())
        .map(|ids| ids.iter().filter_map(|id| id.as_str()).collect())
        .unwrap_or_default();
    if covers.is_empty() {
        // Nothing to show: no view can be linked to this op.
        return Ok(None);
    }
    let views = tx.views_since(&op.seat, op.generation, op.created)?;
    Ok(views
        .iter()
        .find(|view| {
            covers
                .iter()
                .all(|id| view.shown_msgs.iter().any(|shown| shown == id))
        })
        .map(|view| view.ts))
}

/// The time of the view that shows the delivery's task and attempt, if any.
/// Same window as a doorbell: same seat and generation, at or after `created`.
fn shown_delivery(tx: &Tx<'_>, op: &OpRow) -> GearResult<Option<Ts>> {
    let (Some(task), Some(attempt)) = (op.task.as_deref(), op.attempt) else {
        return Ok(None);
    };
    let views = tx.views_since(&op.seat, op.generation, op.created)?;
    Ok(views
        .iter()
        .find(|view| {
            view.shown_task.as_deref() == Some(task) && view.shown_attempt == Some(attempt)
        })
        .map(|view| view.ts))
}

/// A confirmed delivery starts its attempt: `Delivered` → `Started` and the
/// task `Ready` → `Running`, in the same transaction (design/06).
fn start_confirmed_attempt(tx: &Tx<'_>, project: &str, op: &OpRow, now: Ts) -> GearResult<()> {
    let (Some(task), Some(attempt)) = (op.task.as_deref(), op.attempt) else {
        return Ok(());
    };
    let still_delivered = tx
        .get_attempt(project, task, attempt)?
        .map(|row| row.state == AttemptState::Delivered)
        .unwrap_or(false);
    if still_delivered {
        gear_app::tasks::on_delivery_confirmed(tx, project, task, attempt, now)?;
    }
    Ok(())
}

/// On engine start: every Executing op becomes Unknown (the previous process
/// died mid-call). Nothing is re-sent.
///
/// Returns `(project, op id)` pairs: op ids are only unique within a project.
pub fn recover_on_start(store: &mut Store, now: Ts) -> GearResult<Vec<(String, String)>> {
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
                recovered.push((op.project.clone(), op.id));
            }
        }
        Ok(recovered)
    })
}
