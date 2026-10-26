//! Side-effect operations: intend → execute → confirm (design/08).

use gear_herdr::{HerdrAct, Outcome};
use gear_model::state::require_transition;
use gear_model::{AttemptState, ErrorCode, GearError, GearResult, OpKind, OpState, SeatRole, Ts};
use gear_store::{BindingStatus, NewOp, OpRow, Store, Tx};
use std::collections::BTreeMap;

/// Reason stored with an op that the engine found Executing after a restart.
const RECOVERED_REASON: &str = "engine restarted before the call finished";

/// Result stored on a doorbell typed by the sender itself (T-130).
const SENT_BY_SENDER: &str = "typed by the sender";

/// Whether a doorbell op still covers the messages it named (T-130 review F2).
///
/// `Void` never reached the pane, and a definitely `Failed` one was refused by
/// Herdr before it typed anything — so both leave their messages free to be
/// rung again. Everything else (on its way, typed, or possibly typed) covers
/// them: a message is never rung twice, and an uncertain doorbell is never
/// resent.
pub fn doorbell_covers(op: &OpRow) -> bool {
    !matches!(op.state, OpState::Void | OpState::Failed)
}

/// The message ids a seat's doorbell ops still cover, one doorbell per message.
pub(crate) fn covered_messages(
    tx: &Tx<'_>,
    seat: &str,
) -> GearResult<std::collections::BTreeSet<String>> {
    let mut ids = std::collections::BTreeSet::new();
    for op in tx.ops_of_seat_kind(seat, OpKind::Doorbell)? {
        if !doorbell_covers(&op) {
            continue;
        }
        for id in op
            .payload
            .get("covers")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
        {
            ids.insert(id.to_string());
        }
    }
    Ok(ids)
}

/// Type one doorbell into its pane now, from a caller outside the tick
/// (`gear send`, T-130), and record it so the engine never rings the message
/// again.
///
/// The seat and its active binding are re-read here, as the tick does: a stale
/// generation, an unreachable seat or a closed pane is never typed into.
///
/// **The message is claimed before Herdr is called** (T-130 review F1): the
/// doorbell op is written — covering the message, holding the seat's slot — in
/// one write transaction that first checks whether any doorbell of this seat
/// that still covers its message (`covered_messages`) already names it. An
/// engine tick that has already intended its doorbell for the message wins:
/// the sender skips, and the engine types it. When the sender claims first, the
/// tick's own coverage read (and the slot this claim holds) keeps it from ringing
/// the same message again. Only a tick that read its coverage before this claim
/// was written and writes its intent after it can still double-ring, a window of
/// pure computation inside one tick — closing that is the claim protocol the
/// review deferred, and the report names it.
///
/// A prompt Herdr refuses records the op as **Failed**, which covers nothing
/// (F2), so the engine rings the message on its next tick; an uncertain prompt
/// records it as Unknown and is never resent (design/07). Returns the pane the
/// text went to; `None` = skipped (another doorbell covers the message, or the
/// seat cannot be reached) or refused.
pub fn deliver_doorbell_now(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    seat: &str,
    generation: i64,
    message: &str,
    urgent: bool,
    now: Ts,
) -> GearResult<Option<String>> {
    let items = [crate::doorbell::DoorbellItem {
        id: message.to_string(),
        urgent,
        at_once: true,
    }];
    // Design §10.4: a lane with no open attempt gets the text that tells it
    // to stop working on earlier things.
    let no_task = store.read(|tx| {
        let Some(row) = tx.get_seat(seat)? else { return Ok(false) };
        crate::no_task_of(tx, &row)
    })?;
    let text = crate::doorbell::text(&items, no_task);
    // One write transaction: check the seat, the binding and the coverage, and
    // claim the message with this path's own doorbell op before Herdr runs.
    let claimed = store.write(|tx| {
        let Some(row) = tx.get_seat(seat)? else { return Ok(None) };
        if row.generation != generation || !crate::observe::reachable(row.state) {
            return Ok(None);
        }
        let Some(binding) = tx.get_binding(seat, generation)? else { return Ok(None) };
        if binding.status != BindingStatus::Active {
            return Ok(None);
        }
        // F1: another path (the engine's tick, or an earlier send) already
        // covers this message — it types it, this call does not.
        if covered_messages(tx, seat)?.contains(message) {
            return Ok(None);
        }
        // T-134: the name this seat's agent must carry before the prompt —
        // a reviewer keeps the task-based name its review started with.
        let agent_name = crate::names::desired(&row);
        // The CLI that runs in the seat's pane: only an agent of this kind is
        // the seat's agent (design §10.1).
        let kind = tx
            .get_binding(seat, generation)?
            .map(|binding| binding.kind)
            .unwrap_or_default();
        let op = tx.insert_op(&NewOp {
            project: project.to_string(),
            seat: seat.to_string(),
            generation,
            kind: OpKind::Doorbell,
            task: None,
            attempt: None,
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({ "text": text, "covers": [message] }),
            created: now,
        })?;
        // Executing is the claim: the tick's `execute_intended` only drains
        // Intended ops, so nobody else types this one, and the seat's slot is
        // held from here on.
        tx.set_op_state(project, &op.id, OpState::Executing, None, now)?;
        Ok(Some((op.id, binding.pane_id, agent_name, kind)))
    })?;
    let Some((op_id, pane_id, agent_name, kind)) = claimed else { return Ok(None) };
    // Outside any transaction: call Herdr. Design §10.1: the text goes to the
    // live agent **name**, never to the pane id — a pane that lost its agent
    // refuses the text instead of typing it into a shell.
    let names = crate::names::read(herdr).unwrap_or_default();
    let outcome = aim(herdr, &names, &pane_id, &agent_name, &kind, &text);
    let state = match &outcome {
        Aimed::Sent => OpState::Submitted,
        // Nothing was typed: the doorbell waits for the next tick (the agent
        // had lost its name and was renamed).
        Aimed::Retry(..) => OpState::Intended,
        // A refusal proves nothing was typed: the engine rings it again (F2).
        Aimed::Refused(..) | Aimed::Gone(..) => OpState::Failed,
        // The text may be in the pane: never resent (design/07).
        Aimed::Uncertain(..) => OpState::Unknown,
    };
    let result = match &outcome {
        Aimed::Sent => SENT_BY_SENDER.to_string(),
        Aimed::Retry(reason) => reason.clone(),
        Aimed::Gone(reason) => format!("agent-gone: {reason}"),
        Aimed::Refused(reason) => reason.clone(),
        Aimed::Uncertain(reason) => reason.clone(),
    };
    store.write(|tx| {
        if state == OpState::Submitted {
            // The doorbell reached the pane: count it for the captain's
            // reminder clock, exactly as the tick's own path does.
            let action = crate::doorbell::DoorbellAction {
                project: project.to_string(),
                seat: seat.to_string(),
                generation,
                pane_id: pane_id.clone(),
                text: text.clone(),
                covers: vec![message.to_string()],
            };
            crate::attention::record(tx, &action, now)?;
        }
        tx.set_op_state(project, &op_id, state, Some(&result), now)
    })?;
    Ok((state == OpState::Submitted).then_some(pane_id))
}

/// Write Intended ops for the doorbell actions (one transaction). Each op
/// fixes seat, generation and the covered message ids in its payload.
pub fn intend_doorbells(tx: &Tx<'_>, actions: &[crate::doorbell::DoorbellAction], now: Ts) -> GearResult<Vec<OpRow>> {
    let mut ops = Vec::with_capacity(actions.len());
    for action in actions {
        crate::attention::record(tx, action, now)?;
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
            if tx.captain_quota_paused(project)? {
                let seat = tx.get_seat(&op.seat)?;
                if seat.as_ref().is_some_and(|seat| seat.role == gear_model::SeatRole::Captain)
                    && op.payload.get("captain_quota_wake").and_then(|v| v.as_bool()) != Some(true)
                { continue; }
                if op.kind == OpKind::Deliver && seat.as_ref().is_some_and(|seat| seat.role == gear_model::SeatRole::Lane) {
                    if let (Some(task), Some(n)) = (op.task.as_deref(), op.attempt) {
                        if tx.get_attempt(project, task, n)?.is_some_and(|a| a.state == gear_model::AttemptState::Delivered) {
                            continue;
                        }
                    }
                }
            }
            if !matches!(op.kind, OpKind::Doorbell | OpKind::Deliver) {
                // Onboard/release/title/notify have no text yet.
                continue;
            }
            let item = check_op(tx, &op)?;
            // A delivery for a seat the last tick saw blocked waits for the
            // dialog to clear: it stays Intended, so one pending delivery is
            // kept and no failed op is filed every tick (T-68). The
            // blocked-seat alert carries the reason to the captain (T-57).
            let item = match item {
                Prepared::Run { id, .. }
                    if op.kind == OpKind::Deliver && crate::blocked::holding(tx, &op.seat)? =>
                {
                    Prepared::Wait { id }
                }
                other => other,
            };
            match &item {
                // It waits: nothing is written, the op stays Intended.
                Prepared::Wait { .. } => continue,
                Prepared::Run { id, .. } => {
                    require_transition(
                        OpState::Intended.can_transition(OpState::Executing),
                        "operation",
                        "intended",
                        "executing",
                    )?;
                    tx.set_op_state(project, id, OpState::Executing, None, now)?;
                    // T-97: the session this prompt goes into is fixed now. A
                    // hand restart later refreshes the binding, and the failure
                    // must still name the session that received the prompt.
                    if op.kind == OpKind::Deliver {
                        let session = tx
                            .get_binding(&op.seat, op.generation)?
                            .and_then(|binding| binding.session_ref);
                        crate::delivery::record_sent(tx, id, session.as_deref())?;
                    }
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

    // 2. Outside any transaction: call Herdr. The names are read here, after
    //    the tick's own name step, so the name the agent carries now is the
    //    one the text is aimed at (design §10.1).
    let names = crate::names::read(herdr).unwrap_or_default();
    let mut outcomes = Vec::with_capacity(prepared.len());
    for item in &prepared {
        outcomes.push(match item {
            Prepared::Run { pane_id, text, agent_name, kind, .. } => {
                // Design §10.1: the text goes to the live agent **name**. The
                // agent of the pane is read first: only an agent of the
                // seat's kind is the seat's agent, and a pane without one
                // never receives the text.
                Some(aim(herdr, &names, pane_id, agent_name, kind, text))
            }
            // Waiting ops never reach here (they are not prepared).
            Prepared::Wait { .. } => None,
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
                (Prepared::Wait { .. }, _) => {
                    return Err(GearError::new(
                        ErrorCode::BadFormat,
                        format!("waiting operation {} was prepared", item.id()),
                    ))
                }
                (Prepared::Run { .. }, Some(Aimed::Sent)) => (OpState::Submitted, None),
                // Nothing was typed: the op waits for the next tick (the agent
                // had lost its name and was renamed, or the pane could not be
                // read).
                (Prepared::Run { .. }, Some(Aimed::Retry(reason))) => {
                    (OpState::Intended, Some(reason))
                }
                // The seat's agent is gone: the op fails at once, with that
                // reason (design §10.1).
                (Prepared::Run { .. }, Some(Aimed::Gone(reason))) => {
                    (OpState::Failed, Some(format!("agent-gone: {reason}")))
                }
                (Prepared::Run { .. }, Some(Aimed::Refused(reason))) => {
                    (OpState::Failed, Some(reason))
                }
                (Prepared::Run { .. }, Some(Aimed::Uncertain(reason))) => {
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
    /// The op may run: send `text` to the agent of `pane_id`, which must
    /// carry `agent_name` (T-134) and be of the seat's kind (`kind`).
    Run { id: String, pane_id: String, text: String, agent_name: String, kind: String },
    /// The op waits: it stays Intended and the next tick looks again (a
    /// delivery for a seat that is blocked, T-68).
    Wait { id: String },
    /// The op must not run (stale seat, unreachable seat, no binding).
    Void { id: String, reason: String },
}

impl Prepared {
    fn id(&self) -> &str {
        match self {
            Prepared::Run { id, .. } => id,
            Prepared::Wait { id, .. } => id,
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
        // open attempt. Deliveries are only intended for attempts
        // `start_attempt` created, so a missing row means the state moved
        // under the op just as much as a settled or replaced one (R-35,
        // R-41): void it either way.
        let (Some(task), Some(attempt)) = (op.task.as_deref(), op.attempt) else {
            return void("the deliver op names no task or attempt".to_string());
        };
        // A review delivery is the exception: it goes to a temp reviewer and
        // names the attempt it reviews, which is not the reviewer's own.
        if !is_reviewer(tx, &op.seat)? {
            let open = tx.open_attempts_of_seat(&op.seat)?;
            if !open.iter().any(|a| a.task == task && a.n == attempt) {
                return void(format!("task {task} attempt {attempt} is not the seat's open attempt"));
            }
        }
    }
    let Some(text) = op.payload.get("text").and_then(serde_json::Value::as_str) else {
        return void("the op carries no text".to_string());
    };
    Ok(Prepared::Run {
        id: op.id.clone(),
        pane_id: binding.pane_id.clone(),
        text: text.to_string(),
        // T-134: the name the agent in that pane must carry before the prompt.
        agent_name: crate::names::desired(&seat),
        // Design §10.1: the CLI that runs in the pane. Only an agent of this
        // kind is the seat's agent.
        kind: binding.kind.clone(),
    })
}

/// What happened to a text aimed at a seat's agent (design §10.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aimed {
    /// Herdr accepted the text.
    Sent,
    /// Nothing was typed: the op waits for the next tick. The agent had lost
    /// its name (it was renamed) or the pane could not be read.
    Retry(String),
    /// The seat's agent is gone: the op fails at once with this reason.
    Gone(String),
    /// Herdr refused the text: the op fails with this reason.
    Refused(String),
    /// The text may be in the pane: it is never sent again.
    Uncertain(String),
}

/// Aim `text` at the agent of `pane_id` (design §10.1: never type into the
/// wrong thing).
///
/// The text goes to the agent's **name**, never to the pane id: Herdr then
/// refuses it when no agent of that name runs, instead of a race typing it
/// into a shell. When Herdr refuses, `agent_get` decides: an agent of the
/// seat's kind is there → it lost its name, it is renamed and the text goes
/// next tick (nothing is restarted); no agent in the pane → the seat's agent
/// is gone and the op fails at once with that reason.
///
/// The sender has no exception: an unnamed agent is named again and the text
/// goes on the next tick, like everywhere else.
fn aim(
    herdr: &mut dyn HerdrAct,
    names: &BTreeMap<String, Option<String>>,
    pane_id: &str,
    agent_name: &str,
    kind: &str,
    text: &str,
) -> Aimed {
    match crate::names::resolve_target(herdr, names, pane_id, agent_name, kind) {
        crate::names::Target::Prompt { name } => match herdr.prompt(&name, text) {
            Outcome::Ok(()) => Aimed::Sent,
            Outcome::Uncertain { reason } => Aimed::Uncertain(reason),
            Outcome::Refused { code, message } => {
                // Herdr refused the text: `agent_get` decides (design §10.1).
                match herdr.agent_get(pane_id) {
                    Ok(Some(info)) if info.agent.as_deref() == Some(kind) => {
                        // The agent of the seat is there: it lost its name
                        // (M3.1). Nothing else is asked — `agent_get` alone
                        // decides (design §10.1). It is named again and the
                        // text goes next tick, and nothing is restarted.
                        // The naming gives the agent its name back (naming.rs).
                        let note = match crate::names::ensure(herdr, pane_id, agent_name) {
                            crate::names::Ensure::Renamed => "it was renamed".to_string(),
                            crate::names::Ensure::Named => {
                                "it already carried the name".to_string()
                            }
                            crate::names::Ensure::Taken { holder } => {
                                format!("the name is held by {holder:?}")
                            }
                            crate::names::Ensure::NoAgent => "the agent is gone".to_string(),
                            crate::names::Ensure::Failed(reason) => {
                                format!("the rename failed ({reason})")
                            }
                        };
                        Aimed::Retry(format!(
                            "the agent of {pane_id} lost its name ({code}: {message}); {note} and the text goes again next tick"
                        ))
                    }
                    Ok(None) => Aimed::Gone(format!(
                        "{code}: {message} (no agent in the pane {pane_id})"
                    )),
                    Ok(Some(info)) => Aimed::Gone(format!(
                        "{code}: {message} (the pane {pane_id} hosts a {} agent, not the seat's {kind})",
                        info.agent.as_deref().unwrap_or("unknown")
                    )),
                    // The pane could not be read: that is an observation
                    // failure, never "the agent is gone" (design/08). Nothing
                    // was typed, so the text may go next tick.
                    Err(error) => Aimed::Retry(format!(
                        "{code}: {message} (the pane {pane_id} could not be read: {})",
                        error.0
                    )),
                }
            }
        },
        // The agent had lost its name: it was renamed now, the text goes next
        // tick. Nothing is typed and nothing is restarted — the sender too
        // (design §10.1 has no exception for it).
        crate::names::Target::Renamed => Aimed::Retry(format!(
            "the agent of {pane_id} had lost its name; it was renamed and the text goes again next tick"
        )),
        crate::names::Target::Gone(reason) => Aimed::Gone(reason),
        crate::names::Target::Blocked(reason) => Aimed::Refused(reason),
        crate::names::Target::Unreadable(reason) => Aimed::Retry(reason),
    }
}

/// Whether the seat is a temp reviewer (design/10).
fn is_reviewer(tx: &Tx<'_>, seat: &str) -> GearResult<bool> {
    Ok(tx
        .get_seat(seat)?
        .map(|seat| seat.role == SeatRole::Temp)
        .unwrap_or(false))
}

/// The confirmation window of a delivery when nothing else says (design
/// §10.3: `timing.delivery_grace_s`, default 180 s).
pub const DEFAULT_DELIVERY_GRACE_S: i64 = 180;

/// [`confirm_pending_with`] with the default confirmation window.
pub fn confirm_pending(
    store: &mut Store,
    project: &str,
    budget: i64,
    now: Ts,
) -> GearResult<(Vec<String>, Vec<String>)> {
    confirm_pending_with(store, project, budget, DEFAULT_DELIVERY_GRACE_S, now)
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
pub fn confirm_pending_with(
    store: &mut Store,
    project: &str,
    budget: i64,
    grace_s: i64,
    now: Ts,
) -> GearResult<(Vec<String>, Vec<String>)> {
    // The seats whose screen shows a provider limit right now (T-15): their
    // deliveries cannot be answered until the quota comes back, so they spend
    // no check budget below.
    let on_quota = crate::quota::seats_on_quota(store, project)?;
    store.write(|tx| {
        let mut confirmed = Vec::new();
        let mut flagged: Vec<String> = Vec::new();
        for state in [OpState::Submitted, OpState::Unknown] {
            for op in tx.ops_in_state(project, state)? {
                let evidence = match op.kind {
                    // A doorbell is proven by a view of the ids it carried
                    // that are still open; ids closed another way need no
                    // showing (else the doorbell never settles, holds the
                    // slot, and no doorbell rings again — L-66).
                    OpKind::Doorbell => shown_in_a_view(tx, project, &op, now)?,
                    // A review delivery is proven by the verdict the reviewer
                    // hands in — the review packet is not on the reviewer's
                    // home page. A review takes longer than the check budget
                    // of a doorbell, so it spends none (like a kind without an
                    // evidence rule, R-46).
                    OpKind::Deliver if is_reviewer(tx, &op.seat)? => {
                        match review_verdict(tx, &op)? {
                            Some(created) => Some(created),
                            None => continue,
                        }
                    }
                    // A delivery is proven by the task/attempt it announced.
                    OpKind::Deliver => shown_delivery(tx, &op)?,
                    // Onboard/release/title/notify have no evidence rule yet:
                    // leave them completely alone — no budget, no flag (R-46).
                    _ => continue,
                };
                if let Some(view_ts) = evidence {
                    require_transition(
                        state.can_transition(OpState::Confirmed),
                        "operation",
                        state.as_str(),
                        "confirmed",
                    )?;
                    // design/08: confirm(op, evidence) — keep the pointer. A
                    // review delivery is settled by a verdict row, not by a
                    // view: say so, so the stored explanation can be traced
                    // (P3 of the T-5 review).
                    let evidence = if op.kind == OpKind::Deliver && is_reviewer(tx, &op.seat)? {
                        format!(
                            "verdict {} attempt {} @ {view_ts}",
                            op.task.as_deref().unwrap_or("-"),
                            op.attempt.unwrap_or(0)
                        )
                    } else {
                        format!("view {}/{} @ {view_ts}", op.seat, op.generation)
                    };
                    tx.set_op_state(project, &op.id, OpState::Confirmed, Some(&evidence), now)?;
                    if op.kind == OpKind::Deliver {
                        // The delivery got through: nothing is left to send
                        // again for this attempt (T-97).
                        crate::delivery::clear(tx, &op.seat)?;
                        tx.set_meta(&crate::delivery::sent_key(&op.id), "")?;
                        // Design/06: the confirmed delivery starts the attempt.
                        if start_confirmed_attempt(tx, project, &op, now)? {
                            // The lane is not stuck any more (design §10.3):
                            // its stall item closes with the confirmation.
                            crate::stall::resolve(tx, project, &op.seat, now, None)?;
                        }
                    }
                    confirmed.push(op.id.clone());
                    continue;
                }
                let captain_paused = tx.get_seat(&op.seat)?.is_some_and(|seat| seat.role == gear_model::SeatRole::Captain)
                    && tx.captain_quota_paused(project)?;
                if captain_paused || op.payload.get("captain_quota_wake").and_then(|v| v.as_bool()) == Some(true) {
                    continue;
                }
                // Liveness (L-69): a doorbell never holds the seat's slot for
                // more than DOORBELL_EXPIRY_S. Whatever kept it from being
                // confirmed, it expires; if items still wait, the next tick
                // rings again. No state may keep the captain deaf.
                if op.kind == OpKind::Doorbell && now - op.created >= DOORBELL_EXPIRY_S {
                    tx.set_op_state(project, &op.id, OpState::Confirmed, Some("expired: rung again if items still wait"), now)?;
                    confirmed.push(op.id.clone());
                    continue;
                }
                // A seat the screen shows on a provider limit is not failing:
                // its agent cannot answer until the quota comes back, so its
                // budget is not spent and it is not flagged (T-15: a quota
                // turn is not "no progress").
                if on_quota.contains(&op.seat) {
                    continue;
                }
                // No evidence yet: the op waits for the next round.
                if op.kind == OpKind::Deliver {
                    // Design §10.3: the confirmation window is wall clock
                    // (`timing.delivery_grace_s`), not a number of checks: a
                    // lane that reads its packet after a minute still
                    // confirms it.
                    if now - op.created < grace_s {
                        continue;
                    }
                    // The window is over: the delivery is not confirmed. It
                    // fails with the reason, the captain is rung once, and the
                    // open attempt is delivered again in a new agent session
                    // (dispatch).
                    let session = match crate::delivery::sent(tx, &op.id)? {
                        Some(session) => session,
                        None => tx
                            .get_binding(&op.seat, op.generation)?
                            .and_then(|binding| binding.session_ref),
                    };
                    tx.set_op_state(
                        project,
                        &op.id,
                        OpState::Failed,
                        Some(&format!(
                            "delivery not confirmed within {grace_s}s (the lane never showed task {} attempt {}): it is delivered again in a new agent session",
                            op.task.as_deref().unwrap_or("-"),
                            op.attempt.unwrap_or(0)
                        )),
                        now,
                    )?;
                    tx.set_meta(&crate::delivery::sent_key(&op.id), "")?;
                    crate::delivery::record(
                        tx,
                        &op.seat,
                        &crate::delivery::FailedDelivery {
                            task: op.task.clone().unwrap_or_default(),
                            attempt: op.attempt.unwrap_or(0),
                            generation: op.generation,
                            session,
                            retry_now: false,
                        },
                    )?;
                    if !flagged.contains(&op.seat) {
                        flagged.push(op.seat.clone());
                    }
                    continue;
                }
                // A doorbell's budget counts the checks: without evidence the
                // op waits for the next round, and the seat is flagged when
                // the budget is spent (no resend either way).
                let used = tx.bump_op_check_budget(project, &op.id, now)?;
                if used == budget && !flagged.contains(&op.seat) {
                    flagged.push(op.seat.clone());
                }
            }
        }
        Ok((confirmed, flagged))
    })
}

/// The longest a doorbell may wait for its answer before it expires and the
/// seat can be rung again (L-69).
pub const DOORBELL_EXPIRY_S: i64 = 300;

/// A late check-in confirms a delivery that failed (design §10.3).
///
/// The delivery op of an attempt fails when the lane never showed the
/// attempt within the confirmation window. A view that shows the attempt
/// **after** that is proof that the lane did read it: the attempt goes
/// Delivered → Started (design/06) and the stall item of the seat closes —
/// the lane is not stuck any more.
///
/// Returns the confirmed attempts ("T-n/attempt @ the view").
pub fn confirm_late_views(store: &mut Store, project: &str, now: Ts) -> GearResult<Vec<String>> {
    store.write(|tx| {
        let mut confirmed = Vec::new();
        for op in tx.ops_in_state(project, OpState::Failed)? {
            if op.kind != OpKind::Deliver {
                continue;
            }
            // A review delivery is settled by the reviewer's verdict, not by
            // a view of the reviewer's home page.
            if is_reviewer(tx, &op.seat)? {
                continue;
            }
            let Some(view_ts) = shown_delivery(tx, &op)? else {
                continue;
            };
            let (Some(task), Some(attempt)) = (op.task.as_deref(), op.attempt) else {
                continue;
            };
            // The attempt is confirmed while it waits for its first check-in
            // (Delivered). A later state means another op already confirmed
            // it — a retry of this very delivery (design §10.3): the lane
            // recovered either way, so its stall item closes as well.
            let started_here = start_confirmed_attempt(tx, project, &op, now)?;
            if !started_here {
                let already = tx
                    .get_attempt(project, task, attempt)?
                    .map(|row| row.state == AttemptState::Started)
                    .unwrap_or(false);
                if !already {
                    continue;
                }
            }
            // The lane is not stuck: its stall item closes. The item must
            // predate the check-in, so a later failure of the same lane keeps
            // its own item.
            crate::stall::resolve(tx, project, &op.seat, now, Some(view_ts))?;
            confirmed.push(format!("{task}/{attempt} @ {view_ts}"));
        }
        Ok(confirmed)
    })
}

/// The time of the first view of the seat (current generation) at or after the
/// doorbell, if any. A doorbell asks the seat to run `gear`; any view after it
/// is that answer. Whether the view listed every covered id does not matter:
/// the inbox batch caps what one view shows, and items still unseen ring the
/// next doorbell. Tying confirmation to the ids left the captain deaf twice
/// (L-66, L-69): an id no view would show held the seat's slot forever.
fn shown_in_a_view(tx: &Tx<'_>, project: &str, op: &OpRow, now: Ts) -> GearResult<Option<Ts>> {
    if op.payload.get("captain_quota_wake").and_then(|v| v.as_bool()) == Some(true) {
        let before = op.payload.get("views_before").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        return Ok(tx.views_since(&op.seat, op.generation, 0)?.iter().skip(before)
            .filter(|view| view.ts >= op.created).map(|view| view.ts).min());
    }
    let views = tx.views_since(&op.seat, op.generation, op.created)?;
    if let Some(ts) = views.iter().map(|view| view.ts).min() {
        return Ok(Some(ts));
    }
    // No view yet, but every item it rang for was closed another way: nothing
    // is left to answer.
    let ids: Vec<&str> = op
        .payload
        .get("covers")
        .and_then(|c| c.as_array())
        .map(|ids| ids.iter().filter_map(|id| id.as_str()).collect())
        .unwrap_or_default();
    if ids.is_empty() {
        return Ok(None);
    }
    for id in ids {
        match tx.get_message(project, id)? {
            Some(msg) if !matches!(msg.state, gear_model::MsgState::Open | gear_model::MsgState::Waiting) => {}
            _ => return Ok(None),
        }
    }
    Ok(Some(now))
}

/// The time the reviewer's verdict on the reviewed attempt was stored, if it
/// is there: a review delivery is proven by the verdict, not by a view.
fn review_verdict(tx: &Tx<'_>, op: &OpRow) -> GearResult<Option<Ts>> {
    let (Some(task), Some(attempt)) = (op.task.as_deref(), op.attempt) else {
        return Ok(None);
    };
    Ok(tx
        .get_review_verdict(&op.project, task, attempt)?
        .map(|verdict| verdict.created))
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
/// task `Ready` → `Running`, in the same transaction (design/06). Returns
/// false when the attempt is not waiting for its first check-in any more.
fn start_confirmed_attempt(
    tx: &Tx<'_>,
    project: &str,
    op: &OpRow,
    now: Ts,
) -> GearResult<bool> {
    let (Some(task), Some(attempt)) = (op.task.as_deref(), op.attempt) else {
        return Ok(false);
    };
    let still_delivered = tx
        .get_attempt(project, task, attempt)?
        .map(|row| row.state == AttemptState::Delivered)
        .unwrap_or(false);
    if !still_delivered {
        return Ok(false);
    }
    gear_app::tasks::on_delivery_confirmed(tx, project, task, attempt, now)?;
    Ok(true)
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

#[cfg(test)]
mod tests {
    use super::*;
    use gear_model::SeatRole;
    use gear_store::testing;

    const T0: i64 = 1_800_000_000;

    /// An op in `Submitted` for `seat`, with the kind and payload given.
    fn submitted_op(tx: &Tx<'_>, seat: &str, kind: OpKind) -> String {
        let op = tx
            .insert_op(&NewOp {
                project: "p".into(),
                seat: seat.into(),
                generation: 1,
                kind,
                task: None,
                attempt: None,
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({ "text": "x" }),
                created: T0,
            })
            .unwrap();
        tx.set_op_state("p", &op.id, OpState::Executing, None, T0).unwrap();
        tx.set_op_state("p", &op.id, OpState::Submitted, None, T0).unwrap();
        op.id
    }

    #[test]
    fn a_review_delivery_is_confirmed_by_the_verdict() {
        // The review packet is not on the reviewer's home page: the delivery
        // is proven by the verdict the reviewer hands in, and it spends no
        // check budget while it waits (a review takes longer than a tick).
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_temp_seat(tx, "p", "r-1", "T-1", "w1:p3", "codex", T0)?;
                let op = tx
                    .insert_op(&NewOp {
                        project: "p".into(),
                        seat: "p:r-1".into(),
                        generation: 1,
                        kind: OpKind::Deliver,
                        task: Some("T-1".into()),
                        attempt: Some(1),
                        ref_msg: None,
                        plan_version: None,
                        payload: serde_json::json!({ "text": "Review T-1 attempt 1." }),
                        created: T0,
                    })
                    .unwrap();
                tx.set_op_state("p", &op.id, OpState::Executing, None, T0)?;
                tx.set_op_state("p", &op.id, OpState::Submitted, None, T0)?;
                Ok(())
            })
            .unwrap();

        let (confirmed, flagged) = confirm_pending(&mut store, "p", 1, T0 + 10).unwrap();
        assert!(confirmed.is_empty());
        assert!(flagged.is_empty(), "a review waits for its verdict, it flags nothing");
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Submitted);
        assert_eq!(op.check_budget, 0, "no budget is spent while it waits");

        // The verdict arrives: the delivery is confirmed.
        store
            .write(|tx| {
                tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    passed: true,
                    findings: "ops/reviews/T-1.md".into(),
                    by_seat: "p:r-1".into(),
                    created: T0 + 20,
                })
            })
            .unwrap();
        let (confirmed, _) = confirm_pending(&mut store, "p", 1, T0 + 30).unwrap();
        assert_eq!(confirmed, vec!["O-1".to_string()]);
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Confirmed);
    }

    #[test]
    fn kinds_without_an_evidence_rule_are_left_untouched() {
        // R-46: an Onboard op has no evidence rule yet, so confirm_pending must
        // neither bump its budget nor flag its seat — it must skip it.
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
                submitted_op(tx, "p:heat", OpKind::Onboard);
                Ok(())
            })
            .unwrap();

        let (confirmed, flagged) = confirm_pending(&mut store, "p", 1, T0 + 30).unwrap();

        assert!(confirmed.is_empty());
        assert!(flagged.is_empty(), "an op without a rule never flags its seat");
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Submitted);
        assert_eq!(op.check_budget, 0, "no budget is spent on it");
    }
}
