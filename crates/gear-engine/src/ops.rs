//! Side-effect operations: intend → execute → confirm (design/08).

use gear_herdr::HerdrAct;
use gear_model::state::require_transition;
use gear_model::{ErrorCode, GearError, GearResult, OpKind, OpState, Ts};
use gear_store::{NewOp, OpRow, Store, Tx};

/// Key of the store's `meta` table where the engine remembers the projects it
/// has written ops for.
///
/// The store reaches its ops per project (`ops_in_state`) or per seat
/// (`slot_ops`); it cannot list its projects. `recover_on_start` must recover
/// the Executing ops of *every* project, so the engine keeps the list itself.
/// Every path that writes an op registers its project here: an op of an
/// unregistered project would not be recovered after a restart, and an
/// unrecovered op may be sent again.
const PROJECTS_META_KEY: &str = "projects";

/// Reason stored with an op that the engine found Executing after a restart.
const RECOVERED_REASON: &str = "engine restarted before the call finished";

/// Write Intended ops for the doorbell actions (one transaction). Each op
/// fixes seat, generation and the covered message ids in its payload.
pub fn intend_doorbells(tx: &Tx<'_>, actions: &[crate::doorbell::DoorbellAction], now: Ts) -> GearResult<Vec<OpRow>> {
    let mut ops = Vec::with_capacity(actions.len());
    for action in actions {
        remember_project(tx, &action.project)?;
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
    store.write(|tx| {
        let mut recovered = Vec::new();
        for project in known_projects(tx)? {
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

/// Projects the engine has written ops for (see [`PROJECTS_META_KEY`]).
fn known_projects(tx: &Tx<'_>) -> GearResult<Vec<String>> {
    let Some(raw) = tx.get_meta(PROJECTS_META_KEY)? else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&raw).map_err(|e| {
        GearError::new(
            ErrorCode::BadFormat,
            format!("meta.{PROJECTS_META_KEY} is not a list of project ids: {e}"),
        )
    })
}

/// Remember `project`, so that a later `recover_on_start` finds its ops.
fn remember_project(tx: &Tx<'_>, project: &str) -> GearResult<()> {
    let mut projects = known_projects(tx)?;
    if projects.iter().any(|p| p == project) {
        return Ok(());
    }
    projects.push(project.to_string());
    let text = serde_json::to_string(&projects).map_err(|e| {
        GearError::new(ErrorCode::BadFormat, format!("cannot store the project list: {e}"))
    })?;
    tx.set_meta(PROJECTS_META_KEY, &text)
}
