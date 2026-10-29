//! Side-effect operations: intend → execute → confirm (design/08).

use gear_herdr::{AgentStatus, HerdrAct, Outcome};
use gear_model::state::require_transition;
use gear_model::{AttemptState, ErrorCode, GearError, GearResult, MsgKind, OpKind, OpState, SeatRole, SeatState, Ts};
use gear_store::{BindingStatus, NewOp, OpRow, Store, Tx};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Reason stored with an op that the engine found Executing after a restart.
const RECOVERED_REASON: &str = "engine restarted before the call finished";

/// The probe lines this process wrote and no one has drained yet (T-407).
///
/// The engine has no log writer of its own: [`take_probe_log`] hands the lines
/// to `gear-cli`, which writes them into its tick log beside the release lines
/// (T-398), so they carry the same stamp. A test reads them with
/// [`probe_log`] instead, which does not drain: two tests running side by side
/// would take each other's lines, and each test reads its own by a seat or
/// task id no other test uses. A caller that never drains does not grow for
/// ever.
static PROBE_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The probe lines written since the last call, oldest first: the engine
/// writes them into its log (T-407).
pub fn take_probe_log() -> Vec<String> {
    let mut log = PROBE_LOG.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *log)
}

/// The probe lines this process wrote, oldest first, without draining them
/// (T-407): what a test reads.
pub fn probe_log() -> Vec<String> {
    PROBE_LOG.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// One line of the engine log for a finished multi-step operation and for
/// every step of one (T-407): `<op> <seat or task> <step> <ms> total=<ms>`.
///
/// `total` is the operation's time so far, so the last line of an operation
/// carries its whole duration. The times come from the durable state the
/// module keeps (`step_at`, the op's times), so an engine restart in the
/// middle still logs the right durations — except in a merge, which is one
/// worker's run and starts its steps again when it is retried.
fn probe_line(op: &str, subject: &str, step: &str, ms: i64, total: i64) -> String {
    format!("probe {op} {subject} {step} {ms} total={total}")
}

/// Write one probe line into the engine log and keep it for the tests
/// (T-407): the engine drains it into its tick log every tick.
pub(crate) fn probe(op: &str, subject: &str, step: &str, ms: i64, total: i64) {
    let line = probe_line(op, subject, step, ms, total);
    let mut log = PROBE_LOG.lock().unwrap_or_else(|e| e.into_inner());
    if log.len() >= 1024 {
        log.remove(0);
    }
    log.push(line);
}

/// The milliseconds of `seconds`, never negative (T-407).
fn ms(seconds: Ts) -> i64 {
    seconds.saturating_mul(1000)
}

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
    let (no_task, goal) = store.read(|tx| {
        let Some(row) = tx.get_seat(seat)? else { return Ok((false, None)) };
        let goal = if row.role == gear_model::SeatRole::Captain { crate::doorbell::goal_line(tx, &row.project)? } else { None };
        Ok((crate::no_task_of(tx, &row)?, goal))
    })?;
    let text = crate::doorbell::with_goal(crate::doorbell::text(&items, no_task), goal.as_deref());
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
        Ok(Some((op.id, binding.pane_id, agent_name, kind, row.role)))
    })?;
    let Some((op_id, pane_id, agent_name, kind, role)) = claimed else { return Ok(None) };
    // T-241: an urgent text for a lane or a reviewer must not sit behind a
    // long tool call — the working turn is cut first, then the text is aimed
    // as today. The captain's turn is never cut.
    if urgent && matches!(role, SeatRole::Lane | SeatRole::Temp) {
        interrupt_working_turn(herdr, &pane_id);
    }
    // Outside any transaction: call Herdr. Design §10.1: the text goes to the
    // live agent **name**, never to the pane id — a pane that lost its agent
    // refuses the text instead of typing it into a shell.
    let names = crate::names::read(herdr).unwrap_or_default();
    let outcome = aim(herdr, &names, &pane_id, &agent_name, &kind, &text, PromptPolicy::Doorbell { working: true, unknown: urgent });
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
                    if op.kind == OpKind::Deliver && (crate::blocked::holding(tx, &op.seat)?
                        || tx.get_meta(&crate::quiet::compact_key(&op.seat))?.is_some_and(|s| !s.is_empty())) =>
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
            Prepared::Run { pane_id, text, agent_name, kind, policy, interrupt, .. } => {
                // T-241: an urgent doorbell for a lane or a reviewer cuts the
                // working turn first, so the text is not queued as steering
                // behind a long tool call.
                if *interrupt {
                    interrupt_working_turn(herdr, pane_id);
                }
                // Design §10.1: the text goes to the live agent **name**. The
                // agent of the pane is read first: only an agent of the
                // seat's kind is the seat's agent, and a pane without one
                // never receives the text.
                Some(aim(herdr, &names, pane_id, agent_name, kind, text, *policy))
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
            // T-407: a delivery that was typed is a finished step in the
            // engine log, and so is the Enter that follows it. `herdr prompt`
            // types the text and sends the Enter in one call, so the typed
            // step runs from the op's intent to now and the Enter's own step
            // is the same instant (0 ms) — its boundary is logged all the same.
            if let (Prepared::Run { probe: Some((task, created)), .. }, OpState::Submitted) = (item, state) {
                let typed = ms(now.saturating_sub(*created));
                probe("deliver", task, "typed", typed, typed);
                probe("deliver", task, "enter", 0, typed);
            }
            results.push((item.id().to_string(), state.as_str().to_string()));
        }
        Ok(results)
    })
}

/// What to do with one Intended op after the guard checks.
enum Prepared {
    /// The op may run: send `text` to the agent of `pane_id`, which must
    /// carry `agent_name` (T-134) and be of the seat's kind (`kind`).
    Run {
        id: String,
        pane_id: String,
        text: String,
        agent_name: String,
        kind: String,
        policy: PromptPolicy,
        /// T-241: cut the agent's working turn (esc) before the text is
        /// aimed, so an urgent doorbell is not queued as steering.
        interrupt: bool,
        /// T-407: what the probe line of a delivery names — its task and the
        /// time it was intended, which its steps are measured from. `None`
        /// for a doorbell, whose one step is not a delivery.
        probe: Option<(String, Ts)>,
    },
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
    let mut urgent = false;
    let policy = if op.kind == OpKind::Deliver {
        // T-290: a rework packet (attempt n ≥ 2) that is on its way to a lane
        // whose agent queues input while it works (pi, claude) may go while
        // the agent is Working — it waits in the input queue and is read
        // after the current step.
        let working = match (op.task.as_deref(), op.attempt) {
            (Some(task), Some(n)) if n >= 2 && gear_herdr::queues_input(&binding.kind) => tx
                .get_attempt(&op.project, task, n)?
                .is_some_and(|attempt| {
                    attempt.state == AttemptState::Delivered && attempt.sent_at.is_none()
                }),
            _ => false,
        };
        PromptPolicy::Delivery { working }
    } else {
        let ids = op.payload.get("covers").and_then(|v| v.as_array());
        let mut working = ids.is_some_and(|v| !v.is_empty());
        let mut unknown = working;
        let captain = tx.captain_of(&op.project)?.map(|c| c.id);
        if let Some(ids) = ids {
            for id in ids {
                let message = match id.as_str() { Some(id) => tx.get_message(&op.project, id)?, None => None };
                if let Some(message) = message {
                    working &= crate::doorbell::reaches_while_working(&message, &seat, &binding.kind, captain.as_deref());
                    unknown &= message.urgent;
                    urgent |= message.urgent;
                } else { working = false; unknown = false; }
            }
        }
        PromptPolicy::Doorbell { working, unknown }
    };
    Ok(Prepared::Run {
        probe: (op.kind == OpKind::Deliver).then(|| {
            (op.task.clone().unwrap_or_else(|| "-".to_string()), op.created)
        }),
        // T-241: an urgent doorbell for a lane or a reviewer cuts the working
        // turn first (never the captain: its turn is not cut). Deliveries and
        // every other op never interrupt.
        interrupt: op.kind == OpKind::Doorbell
            && matches!(seat.role, SeatRole::Lane | SeatRole::Temp)
            && urgent,
        policy,
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
#[derive(Clone, Copy)]
enum PromptPolicy {
    /// A task packet. `working`: a rework packet for an agent that queues
    /// input while it works may go mid-turn (T-290).
    Delivery { working: bool },
    Doorbell { working: bool, unknown: bool },
}

fn aim(
    herdr: &mut dyn HerdrAct,
    names: &BTreeMap<String, Option<String>>,
    pane_id: &str,
    agent_name: &str,
    kind: &str,
    text: &str,
    policy: PromptPolicy,
) -> Aimed {
    match crate::names::resolve_target(herdr, names, pane_id, agent_name, kind) {
        crate::names::Target::Prompt { name } => {
            // Recheck immediately before prompting: the observation used to
            // plan an op can be stale by the time it is executed.
            match herdr.agent_get(pane_id) {
                Ok(Some(info)) if info.agent.as_deref() == Some(kind) => {
                    let ready = match info.agent_status {
                        gear_herdr::AgentStatus::Idle | gear_herdr::AgentStatus::Done => true,
                        gear_herdr::AgentStatus::Working => {
                            matches!(policy, PromptPolicy::Doorbell { working: true, .. })
                                || matches!(policy, PromptPolicy::Delivery { working: true })
                        }
                        gear_herdr::AgentStatus::Unknown => matches!(policy, PromptPolicy::Doorbell { unknown: true, .. }),
                        gear_herdr::AgentStatus::Blocked => false,
                    };
                    if !ready {
                        let reason = format!("agent in {pane_id} is not ready ({:?})", info.agent_status);
                        return match policy {
                            PromptPolicy::Delivery { .. } => Aimed::Retry(reason),
                            PromptPolicy::Doorbell { .. } => Aimed::Refused(reason),
                        };
                    }
                }
                Ok(None) => return Aimed::Gone(format!("no agent in the pane {pane_id}")),
                Ok(Some(_)) => return Aimed::Gone(format!("the agent in {pane_id} changed kind")),
                Err(e) => return Aimed::Retry(format!("the pane {pane_id} could not be read: {}", e.0)),
            }
            match herdr.prompt(&name, text) {
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
        }},
        // The agent had lost its name: it was renamed now, the text goes next
        // tick. Nothing is typed and nothing is restarted — the sender too
        // (design §10.1 has no exception for it).
        crate::names::Target::Renamed => Aimed::Retry(format!(
            "the agent of {pane_id} had lost its name; it was renamed and the text goes again next tick"
        )),
        crate::names::Target::Gone(reason) => Aimed::Gone(reason),
        crate::names::Target::Blocked(reason) => Aimed::Retry(reason),
        crate::names::Target::Unreadable(reason) => Aimed::Retry(reason),
    }
}

/// T-241: cut a working turn so a text is not queued as steering. Only the
/// pane's own report of a **working** agent matters; an idle, done, blocked or
/// unknown agent, or a pane that cannot be read, is left alone (as quiet.rs
/// does before its own escape).
fn interrupt_working_turn(herdr: &mut dyn HerdrAct, pane_id: &str) {
    if let Ok(Some(info)) = herdr.agent_get(pane_id) {
        if info.agent_status == gear_herdr::AgentStatus::Working {
            let _ = herdr.send_keys(pane_id, &["esc".into()]);
        }
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
    confirm_pending_with(store, project, budget, DEFAULT_DELIVERY_GRACE_S, &[], now)
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
/// `working` names the seats whose agent Herdr reports **working** this tick
/// (T-213): a doorbell that reached a working agent is queued behind its turn,
/// which is not a stall — such a doorbell spends no check budget, so the
/// budget counts only the checks made while the agent is idle or done. A
/// delivery keeps its wall-clock window: a working seat is not spared the
/// confirmation window.
/// Returns (confirmed op ids, flagged seat ids).
pub fn confirm_pending_with(
    store: &mut Store,
    project: &str,
    budget: i64,
    grace_s: i64,
    working: &[String],
    now: Ts,
) -> GearResult<(Vec<String>, Vec<String>)> {
    // The seats whose screen shows a provider limit right now (T-15): their
    // deliveries cannot be answered until the quota comes back, so they spend
    // no check budget below.
    let on_quota = crate::quota::seats_on_quota(store, project)?;
    store.write(|tx| {
        let mut confirmed = Vec::new();
        let mut flagged: Vec<String> = Vec::new();
        // Liveness (T-240): only recover_on_start moved Executing ops on, and
        // an op whose sender never recorded its outcome held the seat's
        // delivery slot for ever — the `gear send` path sets its doorbell to
        // Executing, calls Herdr, then records the outcome in a second write;
        // when that write fails ("database is locked") the op stayed
        // Executing and no doorbell rang again. A doorbell or a delivery stuck
        // in Executing past `DOORBELL_EXPIRY_S` is therefore treated as an
        // Unknown op: the loop below expires the doorbell and never re-sends
        // the delivery. A merge legitimately runs longer, and every other kind
        // is untouched.
        for op in tx.ops_in_state(project, OpState::Executing)? {
            if !matches!(op.kind, OpKind::Doorbell | OpKind::Deliver) || now - op.created < DOORBELL_EXPIRY_S {
                continue;
            }
            require_transition(
                OpState::Executing.can_transition(OpState::Unknown),
                "operation",
                "executing",
                "unknown",
            )?;
            tx.set_op_state(
                project,
                &op.id,
                OpState::Unknown,
                Some("stale executing: its sender never recorded the outcome"),
                now,
            )?;
        }
        for state in [OpState::Submitted, OpState::Unknown] {
            for op in tx.ops_in_state(project, state)? {
                // What proves the op, and whether that proof is a view of the
                // reviewer's own seat (T-267).
                let evidence = match op.kind {
                    // A doorbell is proven by a view of the ids it carried
                    // that are still open; ids closed another way need no
                    // showing (else the doorbell never settles, holds the
                    // slot, and no doorbell rings again — L-66).
                    OpKind::Doorbell => shown_in_a_view(tx, project, &op, now)?.map(|ts| (ts, false)),
                    // A review delivery is proven by the reviewer's own first
                    // `gear` — a view of its seat and generation after the
                    // delivery — or by the verdict it hands in, whichever came
                    // first (T-267). A newer delivery of the same seat, task
                    // and attempt supersedes this one: only it is the packet
                    // the reviewer got, and its confirmation voids this op.
                    // A review takes longer than the check budget of a
                    // doorbell, so it spends none (like a kind without an
                    // evidence rule, R-46).
                    OpKind::Deliver if is_reviewer(tx, &op.seat)? => {
                        if superseded(tx, project, &op)? { continue }
                        match review_delivery_evidence(tx, &op)? {
                            Some(found) => Some(found),
                            None => continue,
                        }
                    }
                    // A delivery is proven by the task/attempt it announced.
                    OpKind::Deliver => shown_delivery(tx, &op)?.map(|ts| (ts, false)),
                    // Onboard/release/title/notify have no evidence rule yet:
                    // leave them completely alone — no budget, no flag (R-46).
                    _ => continue,
                };
                if let Some((view_ts, review_by_view)) = evidence {
                    require_transition(
                        state.can_transition(OpState::Confirmed),
                        "operation",
                        state.as_str(),
                        "confirmed",
                    )?;
                    // design/08: confirm(op, evidence) — keep the pointer. A
                    // review delivery settled by a verdict row says so, as one
                    // settled by the reviewer's own view says `view` like any
                    // other: the stored explanation can be traced (P3 of the
                    // T-5 review).
                    let review_delivery = op.kind == OpKind::Deliver && is_reviewer(tx, &op.seat)?;
                    let hand_in = op.kind == OpKind::Deliver && !review_delivery && !tx.views_since(&op.seat, op.generation, op.created)?.iter().any(|view|
                        view.ts == view_ts && view.shown_task.as_deref() == op.task.as_deref() && view.shown_attempt == op.attempt);
                    let evidence = if review_delivery && !review_by_view {
                        format!("verdict {} attempt {} @ {view_ts}", op.task.as_deref().unwrap_or("-"), op.attempt.unwrap_or(0))
                    } else if hand_in {
                        format!("hand-in {} attempt {} @ {view_ts}", op.task.as_deref().unwrap_or("-"), op.attempt.unwrap_or(0))
                    } else {
                        format!("view {}/{} @ {view_ts}", op.seat, op.generation)
                    };
                    tx.set_op_state(project, &op.id, OpState::Confirmed, Some(&evidence), now)?;
                    // T-407: a delivery's last step, from its typing (`op`
                    // is still the row read before this update, so `updated`
                    // is when the text was typed) and from its intent.
                    if op.kind == OpKind::Deliver {
                        let total = ms(now.saturating_sub(op.created));
                        probe(
                            "deliver",
                            op.task.as_deref().unwrap_or("-"),
                            "confirmed",
                            ms(now.saturating_sub(op.updated)),
                            total,
                        );
                        probe("deliver", op.task.as_deref().unwrap_or("-"), "done", total, total);
                    }
                    if review_delivery {
                        supersede_older_review_deliveries(tx, project, &op, now)?;
                    }
                    if op.kind == OpKind::Deliver {
                        // The delivery got through: nothing is left to send
                        // again for this attempt (T-97).
                        crate::delivery::clear(tx, &op.seat)?;
                        tx.set_meta(&crate::delivery::sent_key(&op.id), "")?;
                        // Design/06: the confirmed delivery starts the attempt.
                        if start_confirmed_attempt(tx, project, &op, now)? {
                            // §3.2 step 5: at the check-in the attempt records
                            // the session it ran in.
                            record_attempt_session(tx, project, &op)?;
                            crate::stall::resolve(tx, project, &op.seat, now, None)?;
                        } else if hand_in {
                            // A result already started and submitted the
                            // attempt. A newer stall belongs to later work.
                            crate::stall::resolve(tx, project, &op.seat, now, Some(view_ts))?;
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
                // A doorbell that reached a working agent (T-213) is not a
                // stall: the agent is in the middle of a build or a suite and
                // the text waits in the pane behind its turn. Its check
                // budget is not spent, so the budget counts only the checks
                // made while the agent is idle or done.
                if op.kind == OpKind::Doorbell && working.contains(&op.seat) {
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
                    // T-407: the delivery's last step, as for a confirmed
                    // one: from its typing (`op.updated`) and its intent, and
                    // the operation's own completion line after it.
                    let total = ms(now.saturating_sub(op.created));
                    probe(
                        "deliver",
                        op.task.as_deref().unwrap_or("-"),
                        "failed",
                        ms(now.saturating_sub(op.updated)),
                        total,
                    );
                    probe("deliver", op.task.as_deref().unwrap_or("-"), "done", total, total);
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
                    // T-206 (F3), T-218: the first failure of a delivery may
                    // still be retried by the lane's own path: the marker
                    // says so, and the tick holds the captain's ring back
                    // until the retry has had its chance. Only a lane whose
                    // attempt waits for its check-in gets that retry; every
                    // other seat (a reviewer, the captain) has none, so its
                    // unconfirmed delivery reaches the captain at once.
                    let task = op.task.clone().unwrap_or_default();
                    let attempt = op.attempt.unwrap_or_default();
                    let marker = crate::lane_nudge::retry_marker(&op.seat, &task, attempt);
                    let eligible = crate::lane_nudge::retry_eligible(
                        tx,
                        project,
                        &op.seat,
                        &task,
                        attempt,
                    )?;
                    match tx.get_meta(&marker)?.as_deref() {
                        None => {
                            // The first failure: the lane's own path may
                            // still deliver the packet again (T-206, F3).
                            if eligible {
                                tx.set_meta(&marker, "allowed")?;
                            }
                        }
                        Some("pending") => {
                            // The retry failed too: the fact stands, the
                            // captain hears about it.
                            tx.set_meta(&marker, "")?;
                        }
                        _ => {}
                    }
                    // The one automatic retry is still ahead: the captain is
                    // not rung yet. The stall item opens when the retried
                    // delivery fails or its own window ends unconfirmed
                    // (T-218).
                    let retry_pending =
                        crate::lane_nudge::retry_pending(tx, &op.seat, &task, attempt)?;
                    if !retry_pending && !flagged.contains(&op.seat) {
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

/// How long a submitted delivery waits before the engine looks at its pane for
/// text that was typed but never sent (T-371).
pub const UNSENT_RESUBMIT_AFTER_S: i64 = 30;

/// How many lines of the pane the typed text is looked for in (T-371).
const UNSENT_READ_LINES: u32 = 12;

/// How many of the tail's last non-empty lines must show the text (T-371).
const UNSENT_TAIL_LINES: usize = 6;

/// How much of the text's first line identifies it in the pane (T-371).
const UNSENT_NEEDLE_CHARS: usize = 40;

/// The meta key that marks an op as already given its Enter (T-371).
fn resubmit_key(op_id: &str) -> String {
    format!("engine.resubmit.{op_id}")
}

/// The meta key that marks an op as already typed again (T-404).
fn retype_key(op_id: &str) -> String {
    format!("engine.retype.{op_id}")
}

/// The needle a pane must show for `text`: its first line, cut to its first
/// [`UNSENT_NEEDLE_CHARS`] characters. `None` = nothing to look for.
fn unsent_needle(text: &str) -> Option<String> {
    let needle: String = text
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(UNSENT_NEEDLE_CHARS)
        .collect();
    if needle.trim().is_empty() {
        return None;
    }
    Some(needle)
}

/// Whether one of the tail's last [`UNSENT_TAIL_LINES`] non-empty lines shows
/// `needle` (T-371).
fn tail_shows(tail: &str, needle: &str) -> bool {
    let lines: Vec<&str> = tail.lines().filter(|line| !line.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(UNSENT_TAIL_LINES);
    lines[start..].iter().any(|line| line.contains(needle))
}

/// Give a delivery whose text sits unsent in its lane's input box its Enter
/// (T-371, M-4569, L-76).
///
/// A delivery op stays Submitted until a view or a hand-in confirms it, and
/// nothing looked at the pane in between: the text was typed, the Enter was
/// lost, and the lane sat idle with its packet waiting in the input box. For
/// every lane Deliver op of the project in `Submitted` at least
/// [`UNSENT_RESUBMIT_AFTER_S`] old, whose seat is Active at the op's
/// generation, whose agent is Idle or Done and that never had its Enter
/// (`engine.resubmit.<op id>` not set): the pane's tail is read, and when the
/// text's first line (its first [`UNSENT_NEEDLE_CHARS`] characters) shows in
/// one of its last [`UNSENT_TAIL_LINES`] non-empty lines, one `enter` is sent
/// and the meta key records it — so the op gets that Enter once, never twice.
/// A failed read or a refused key changes nothing (no meta, no second try at
/// once). A confirmed op is never touched: the lane read its packet.
///
/// When the tail does **not** show the text, the delivery never reached the
/// input box at all — a codex lane showed its empty input placeholder while
/// the op stayed Submitted (T-404). The delivery is then typed again once
/// with `herdr prompt` (text + Enter), unless it was typed again before
/// (`engine.retype.<op id>` not set), and the meta key records it; a refused
/// prompt changes nothing. A retyped op is listed in the same `resubmitted`
/// result.
pub fn resubmit_unsent(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    now: Ts,
) -> GearResult<Vec<String>> {
    // The candidates: one read transaction, the pane calls come after it.
    let candidates = store.read(|tx| {
        let mut out = Vec::new();
        for op in tx.ops_in_state(project, OpState::Submitted)? {
            if op.kind != OpKind::Deliver || now - op.created < UNSENT_RESUBMIT_AFTER_S {
                continue;
            }
            let Some(seat) = tx.get_seat(&op.seat)? else { continue };
            if seat.role != SeatRole::Lane
                || seat.state != SeatState::Active
                || seat.generation != op.generation
            {
                continue;
            }
            let Some(binding) = tx.get_binding(&op.seat, op.generation)? else { continue };
            if binding.status != BindingStatus::Active {
                continue;
            }
            if tx.get_meta(&resubmit_key(&op.id))?.is_some()
                || tx.get_meta(&retype_key(&op.id))?.is_some()
            {
                continue;
            }
            let Some(text) = op.payload.get("text").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(needle) = unsent_needle(text) else { continue };
            out.push((
                op.id.clone(),
                binding.pane_id.clone(),
                needle,
                text.to_string(),
                // T-407: what the step's probe line names, and the times it
                // is measured from — the op's task and its own times.
                op.task.clone().unwrap_or_else(|| "-".to_string()),
                op.created,
                op.updated,
            ));
        }
        Ok(out)
    })?;
    let mut sent = Vec::new();
    for (op_id, pane, needle, text, task, created, typed_at) in candidates {
        // Only an agent that has nothing better to do left the text unsent.
        let Ok(Some(agent)) = herdr.agent_get(&pane) else { continue };
        if !matches!(agent.agent_status, AgentStatus::Idle | AgentStatus::Done) {
            continue;
        }
        let Ok(tail) = herdr.read_tail(&pane, UNSENT_READ_LINES) else { continue };
        if !tail_shows(&tail, &needle) {
            // The text never reached the input box (T-404): it is typed again
            // once, and a refused prompt changes nothing, not even the meta.
            if !matches!(herdr.prompt(&pane, &text), Outcome::Ok(())) {
                continue;
            }
            store.write(|tx| tx.set_meta(&retype_key(&op_id), &now.to_string()))?;
            // T-407: the text typed again is the delivery's `typed` step.
            probe("deliver", &task, "typed", ms(now.saturating_sub(created)), ms(now.saturating_sub(created)));
            sent.push(op_id);
            continue;
        }
        // The Enter itself: a refused key changes nothing, not even the meta.
        if !matches!(herdr.send_keys(&pane, &["enter".to_string()]), Outcome::Ok(())) {
            continue;
        }
        store.write(|tx| tx.set_meta(&resubmit_key(&op_id), &now.to_string()))?;
        // T-407: the Enter the delivery lost is its own step, measured from
        // the typing it waited behind.
        probe("deliver", &task, "enter", ms(now.saturating_sub(typed_at)), ms(now.saturating_sub(created)));
        sent.push(op_id);
    }
    Ok(sent)
}

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
            if started_here {
                // §3.2 step 5: at the check-in the attempt records the session
                // it ran in (T-192, the late check-in of a delivery whose
                // answer was lost).
                record_attempt_session(tx, project, &op)?;
            }
            let mut late_hand_in_key = None;
            // The scan reports an attempt only when it changes it: an attempt
            // that is already Started is nothing new (T-192).
            let mut changed = started_here;
            if !started_here {
                let state = tx.get_attempt(project, task, attempt)?.map(|row| row.state);
                if matches!(state, Some(AttemptState::Submitted | AttemptState::Accepted | AttemptState::Rejected)) {
                    let key = format!("engine.delivery_late_handin.{}.{}", project, op.id);
                    if tx.get_meta(&key)?.is_some_and(|value| !value.is_empty()) { continue }
                    late_hand_in_key = Some(key);
                    changed = true;
                } else if state != Some(AttemptState::Started) {
                    continue;
                }
            }
            // The lane is not stuck: its stall item closes. The item must
            // predate the check-in, so a later failure of the same lane keeps
            // its own item.
            crate::stall::resolve(tx, project, &op.seat, now, Some(view_ts))?;
            if let Some(key) = late_hand_in_key { tx.set_meta(&key, "1")?; }
            if changed {
                confirmed.push(format!("{task}/{attempt} @ {view_ts}"));
            }
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

/// What proves a reviewer's delivery (T-267): the first view of the reviewer's
/// own seat and generation at or after the delivery, or the verdict the
/// reviewer hands in — whichever came first. The flag is true when the view
/// came first, so the stored evidence is the view's.
fn review_delivery_evidence(tx: &Tx<'_>, op: &OpRow) -> GearResult<Option<(Ts, bool)>> {
    let verdict = review_verdict(tx, op)?;
    let view = tx
        .views_since(&op.seat, op.generation, op.created)?
        .first()
        .map(|view| view.ts);
    Ok(match (view, verdict) {
        (Some(view), Some(verdict)) if view > verdict => Some((verdict, false)),
        (Some(view), _) => Some((view, true)),
        (None, Some(verdict)) => Some((verdict, false)),
        (None, None) => None,
    })
}

/// A reviewer whose packet was delivered twice has two delivery ops for one
/// attempt, and only the newer one is the packet the reviewer got: while it
/// still waits for its evidence, the older op is not confirmed (T-267) —
/// the newer one's confirmation voids it.
fn superseded(tx: &Tx<'_>, project: &str, op: &OpRow) -> GearResult<bool> {
    Ok(tx.ops_in_state(project, OpState::Submitted)?.into_iter().any(|other| {
        other.id != op.id
            && other.kind == OpKind::Deliver
            && other.seat == op.seat
            && other.task == op.task
            && other.attempt == op.attempt
            && (other.created, op_seq(&other.id)) > (op.created, op_seq(&op.id))
    }))
}

/// The numeric part of an op id (`O-12` → 12).
fn op_seq(id: &str) -> i64 {
    id.rsplit('-').next().and_then(|n| n.parse::<i64>().ok()).unwrap_or(0)
}

/// A reviewer's delivery op that the reviewer's own first `gear` proves
/// received is confirmed; every older Submitted delivery of the same seat for
/// the same task and attempt is then Void (T-267) — otherwise it holds the
/// seat's delivery slot and is sent again.
fn supersede_older_review_deliveries(
    tx: &Tx<'_>,
    project: &str,
    op: &OpRow,
    now: Ts,
) -> GearResult<()> {
    let older: Vec<String> = tx
        .ops_in_state(project, OpState::Submitted)?
        .into_iter()
        .filter(|other| {
            other.id != op.id
                && other.kind == OpKind::Deliver
                && other.seat == op.seat
                && other.task == op.task
                && other.attempt == op.attempt
                && (other.created, op_seq(&other.id)) < (op.created, op_seq(&op.id))
        })
        .map(|other| other.id)
        .collect();
    for id in older {
        // A Submitted op that reached the pane is voided the raw way, as a
        // review's start op is (release.rs): the state machine has no
        // Submitted → Void transition, and this is the one op the newer
        // delivery replaces.
        tx.set_op_state(
            project,
            &id,
            OpState::Void,
            Some(&format!("superseded by {}", op.id)),
            now,
        )?;
    }
    Ok(())
}

/// The time the reviewer's verdict on the reviewed attempt was stored, if it
/// is there.
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
    if let Some(view) = views.iter().find(|view| view.shown_task.as_deref() == Some(task) && view.shown_attempt == Some(attempt)) {
        return Ok(Some(view.ts));
    }
    // A hand-in from this seat proves it received the attempt even when no
    // check-in view was recorded. Do not flag its delivery as unconfirmed.
    let row = tx.get_attempt(&op.project, task, attempt)?;
    if row.as_ref().is_some_and(|a| a.seat == op.seat && matches!(a.state, gear_model::AttemptState::Submitted | gear_model::AttemptState::Accepted | gear_model::AttemptState::Rejected)) {
        return Ok(tx.attempt_times(&op.project, task, attempt)?.handed_in);
    }
    // T-277: the seat's own word about this very attempt is proof as well — a
    // `gear send` (progress, a question, a result) names the task and attempt
    // and carries the sender's generation, so a lane that works on the task
    // without ever running `gear` first has plainly received it. The first
    // such message's time is the evidence; a message on another task or from
    // an older generation proves nothing about this delivery.
    let messages = tx.messages_from_since(&op.project, &op.seat, op.created)?;
    if let Some(message) = messages.into_iter().find(|message| {
        message.sender_gen == op.generation
            && message.task.as_deref() == Some(task)
            && message.attempt == Some(attempt)
            && matches!(message.kind, MsgKind::Progress | MsgKind::Question | MsgKind::Result)
    }) {
        return Ok(Some(message.created));
    }
    Ok(None)
}

/// The session the attempt ran in (design §3.2 step 5): at the check-in the
/// seat's binding knows the session the agent is in.
fn record_attempt_session(tx: &Tx<'_>, project: &str, op: &OpRow) -> GearResult<()> {
    // A review delivery names the **writer's** attempt, while its binding
    // belongs to the reviewer: the writer's session is not the reviewer's, and
    // a review says nothing about the session the writer worked in.
    if is_reviewer(tx, &op.seat)? {
        return Ok(());
    }
    let (Some(task), Some(attempt)) = (op.task.as_deref(), op.attempt) else {
        return Ok(());
    };
    // Only the seat's own attempt, in its own generation: a delivery of
    // another seat's round leaves the attempt alone.
    let Some(row) = tx.get_attempt(project, task, attempt)? else {
        return Ok(());
    };
    if row.seat != op.seat || row.generation != op.generation {
        return Ok(());
    }
    let Some(binding) = tx.get_binding(&op.seat, op.generation)? else {
        return Ok(());
    };
    let Some(session) = binding.session_ref.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    tx.set_attempt_session(project, task, attempt, session, &binding.kind)
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

    /// A reviewer's delivery of `task` attempt 1, Submitted, created `created`.
    fn review_delivery(tx: &Tx<'_>, task: &str, created: Ts) -> String {
        let op = tx
            .insert_op(&NewOp {
                project: "p".into(),
                seat: "p:r-1".into(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some(task.into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({ "text": "Review packet" }),
                created,
            })
            .unwrap();
        tx.set_op_state("p", &op.id, OpState::Executing, None, created).unwrap();
        tx.set_op_state("p", &op.id, OpState::Submitted, None, created).unwrap();
        op.id
    }

    /// A view of the reviewer's own seat and generation at `ts`.
    fn reviewer_view(tx: &Tx<'_>, ts: Ts) {
        tx.insert_view(&gear_store::ViewRecord {
            project: "p".into(),
            seat: "p:r-1".into(),
            generation: 1,
            ts,
            shown_msgs: vec![],
            shown_ops: vec![],
            shown_task: None,
            shown_attempt: None,
            shown_contract: None,
        })
        .unwrap();
    }

    fn reviewer_world() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_temp_seat(tx, "p", "r-1", "T-1", "w1:p3", "codex", T0)
            })
            .unwrap();
        store
    }

    #[test]
    fn a_review_delivery_is_confirmed_by_the_reviewers_first_view() {
        // T-267: the reviewer's own `gear` proves the packet arrived, before
        // any verdict — a lost packet shows at once instead of waiting for a
        // review that never started.
        let mut store = reviewer_world();
        store
            .write(|tx| {
                review_delivery(tx, "T-1", T0);
                reviewer_view(tx, T0 + 10);
                Ok(())
            })
            .unwrap();

        let (confirmed, flagged) = confirm_pending(&mut store, "p", 1, T0 + 20).unwrap();

        assert_eq!(confirmed, vec!["O-1".to_string()]);
        assert!(flagged.is_empty());
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Confirmed);
        assert_eq!(op.result, Some(format!("view p:r-1/1 @ {}", T0 + 10)));
    }

    #[test]
    fn the_first_evidence_of_a_review_delivery_is_the_one_stored() {
        // A verdict that came before the view is the earlier proof: the op
        // keeps saying so (T-267, "whichever came first").
        let mut store = reviewer_world();
        store
            .write(|tx| {
                review_delivery(tx, "T-1", T0);
                tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    passed: true,
                    findings: "ops/reviews/T-1.md".into(),
                    by_seat: "p:r-1".into(),
                    created: T0 + 5,
                })?;
                reviewer_view(tx, T0 + 10);
                Ok(())
            })
            .unwrap();

        let (confirmed, _) = confirm_pending(&mut store, "p", 1, T0 + 20).unwrap();

        assert_eq!(confirmed, vec!["O-1".to_string()]);
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.result, Some(format!("verdict T-1 attempt 1 @ {}", T0 + 5)));
    }

    #[test]
    fn a_review_delivery_with_no_view_and_no_verdict_stays_submitted() {
        // Neither proof is in: the delivery waits, spends no budget and flags
        // nothing — the reviewer may still be starting (T-267).
        let mut store = reviewer_world();
        store.write(|tx| { review_delivery(tx, "T-1", T0); Ok(()) }).unwrap();

        let (confirmed, flagged) = confirm_pending(&mut store, "p", 1, T0 + 600).unwrap();

        assert!(confirmed.is_empty());
        assert!(flagged.is_empty());
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Submitted);
        assert_eq!(op.check_budget, 0);
    }

    #[test]
    fn a_newer_review_delivery_supersedes_the_older_one() {
        // Two deliveries of one attempt (T-267): the newer is the packet the
        // reviewer got, so confirming it voids the older, which would hold the
        // seat's delivery slot and be sent again.
        let mut store = reviewer_world();
        store
            .write(|tx| {
                review_delivery(tx, "T-1", T0);
                review_delivery(tx, "T-1", T0 + 5);
                reviewer_view(tx, T0 + 10);
                Ok(())
            })
            .unwrap();

        let (confirmed, _) = confirm_pending(&mut store, "p", 1, T0 + 20).unwrap();

        assert_eq!(confirmed, vec!["O-2".to_string()]);
        let older = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(older.state, OpState::Void);
        assert_eq!(older.result.as_deref(), Some("superseded by O-2"));
        let newer = store.read(|tx| tx.get_op("p", "O-2")).unwrap().unwrap();
        assert_eq!(newer.state, OpState::Confirmed);
    }

    #[test]
    fn a_lane_delivery_still_needs_a_view_of_its_task_and_attempt() {
        // Unchanged (T-48): any view of a lane's seat is not proof; the view
        // must show the task and attempt the delivery announced.
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
                let op = tx
                    .insert_op(&NewOp {
                        project: "p".into(),
                        seat: "p:heat".into(),
                        generation: 1,
                        kind: OpKind::Deliver,
                        task: Some("T-1".into()),
                        attempt: Some(1),
                        ref_msg: None,
                        plan_version: None,
                        payload: serde_json::json!({ "text": "New task T-1" }),
                        created: T0,
                    })
                    .unwrap();
                tx.set_op_state("p", &op.id, OpState::Executing, None, T0)?;
                tx.set_op_state("p", &op.id, OpState::Submitted, None, T0)?;
                tx.insert_view(&gear_store::ViewRecord {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    ts: T0 + 10,
                    shown_msgs: vec![],
                    shown_ops: vec![],
                    shown_task: None,
                    shown_attempt: None,
                    shown_contract: None,
                })?;
                Ok(())
            })
            .unwrap();

        let (confirmed, _) = confirm_pending_with(&mut store, "p", 2, 60, &[], T0 + 20).unwrap();

        assert!(confirmed.is_empty(), "a view of the seat alone proves nothing");
        assert_eq!(store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap().state, OpState::Submitted);
    }

    /// A lane Deliver op for `task` attempt 1, Submitted, created `created`.
    fn lane_delivery(tx: &Tx<'_>, task: &str, created: Ts) -> GearResult<()> {
        let op = tx.insert_op(&NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: OpKind::Deliver,
            task: Some(task.into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({ "text": format!("New task {task}") }),
            created,
        })?;
        tx.set_op_state("p", &op.id, OpState::Executing, None, created)?;
        tx.set_op_state("p", &op.id, OpState::Submitted, None, created)?;
        Ok(())
    }

    /// A message `sender` sent in `sender_gen` naming `task`/`attempt`.
    fn seat_message(
        tx: &Tx<'_>,
        sender: &str,
        sender_gen: i64,
        task: &str,
        attempt: i64,
        kind: MsgKind,
        created: Ts,
    ) -> GearResult<()> {
        tx.insert_message(&gear_store::NewMessage {
            project: "p".into(),
            thread: format!("{task}:{attempt}"),
            task: Some(task.into()),
            attempt: Some(attempt),
            sender: sender.into(),
            sender_gen,
            recipient: "p:captain".into(),
            kind,
            urgent: false,
            body: "work update".into(),
            reply_to: None,
            needs_ack: false,
            state: gear_model::MsgState::Info,
            created,
            meta: None,
        })?;
        Ok(())
    }

    fn lane_world() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)
            })
            .unwrap();
        store
    }

    #[test]
    fn a_lane_delivery_is_confirmed_by_its_progress_message() {
        // T-277: a lane that works on the task without ever running `gear`
        // still received the delivery — its own progress message names the
        // task and attempt and carries its generation. That message's time is
        // the evidence, so the engine stops re-delivering the task.
        let mut store = lane_world();
        store
            .write(|tx| {
                lane_delivery(tx, "T-1", T0)?;
                seat_message(tx, "p:heat", 1, "T-1", 1, MsgKind::Progress, T0 + 10)
            })
            .unwrap();

        let (confirmed, _) = confirm_pending_with(&mut store, "p", 2, 60, &[], T0 + 20).unwrap();

        assert_eq!(confirmed, vec!["O-1".to_string()]);
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Confirmed);
        assert!(
            op.result.as_deref().is_some_and(|result| result.contains(&format!("T-1 attempt 1 @ {}", T0 + 10))),
            "the message's time is the evidence: {:?}",
            op.result
        );
    }

    #[test]
    fn a_message_for_another_task_or_from_an_older_generation_does_not_confirm() {
        // T-277: only the seat's word about this very task and attempt, in the
        // generation the delivery named, proves the delivery arrived.
        let mut store = lane_world();
        store
            .write(|tx| {
                lane_delivery(tx, "T-1", T0)?;
                seat_message(tx, "p:heat", 1, "T-2", 1, MsgKind::Progress, T0 + 5)?;
                seat_message(tx, "p:heat", 1, "T-1", 2, MsgKind::Result, T0 + 6)?;
                seat_message(tx, "p:heat", 0, "T-1", 1, MsgKind::Question, T0 + 7)?;
                seat_message(tx, "p:other", 1, "T-1", 1, MsgKind::Progress, T0 + 8)
            })
            .unwrap();

        let (confirmed, _) = confirm_pending_with(&mut store, "p", 2, 60, &[], T0 + 20).unwrap();

        assert!(confirmed.is_empty(), "none of these messages is the lane's word on T-1 attempt 1");
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Submitted);
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

    /// An op its sender left in `Executing` (T-240): the second write that
    /// recorded the outcome never landed.
    fn executing_op(tx: &Tx<'_>, seat: &str, kind: OpKind, created: Ts) -> String {
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
                created,
            })
            .unwrap();
        tx.set_op_state("p", &op.id, OpState::Executing, None, created).unwrap();
        op.id
    }

    #[test]
    fn a_doorbell_stuck_in_executing_does_not_hold_the_slot() {
        // T-240: `gear send` sets its doorbell to Executing, calls Herdr and
        // records the outcome in a second write; under load that write failed
        // ("database is locked"), so the op stayed Executing and held the
        // seat's delivery slot for ever — no doorbell rang again (M4.2: hull
        // sat idle 20 min with work ready). Past DOORBELL_EXPIRY_S such an op
        // is an Unknown op, which the doorbell expiry below then settles.
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
                executing_op(tx, "p:heat", OpKind::Doorbell, T0 - DOORBELL_EXPIRY_S);
                Ok(())
            })
            .unwrap();

        let (confirmed, _) = confirm_pending(&mut store, "p", 1, T0).unwrap();

        assert_eq!(confirmed, vec!["O-1".to_string()]);
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Confirmed, "it left Executing and expired like any doorbell");
        assert_eq!(
            op.result.as_deref(),
            Some("expired: rung again if items still wait")
        );
    }

    #[test]
    fn a_doorbell_executing_for_ten_seconds_is_left_alone() {
        // A doorbell whose Herdr call is still running is not stale yet.
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
                executing_op(tx, "p:heat", OpKind::Doorbell, T0 - 10);
                Ok(())
            })
            .unwrap();

        let (confirmed, _) = confirm_pending(&mut store, "p", 1, T0).unwrap();

        assert!(confirmed.is_empty());
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Executing);
    }

    #[test]
    fn a_merge_executing_for_an_hour_is_untouched() {
        // A merge legitimately runs longer than a doorbell: it is not made
        // Unknown behind its sender's back.
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
                executing_op(tx, "p:heat", OpKind::Merge, T0 - 3600);
                Ok(())
            })
            .unwrap();

        let (confirmed, _) = confirm_pending(&mut store, "p", 1, T0).unwrap();

        assert!(confirmed.is_empty());
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Executing);
    }

    #[test]
    fn a_delivery_executing_for_five_minutes_becomes_unknown() {
        // A stale delivery is not re-sent: it becomes Unknown, exactly as a
        // delivery whose sender died with the engine (recover_on_start).
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
                executing_op(tx, "p:heat", OpKind::Deliver, T0 - 300);
                Ok(())
            })
            .unwrap();

        let (confirmed, _) =
            confirm_pending_with(&mut store, "p", 1, 1000, &[], T0).unwrap();

        assert!(confirmed.is_empty());
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Unknown);
    }

    // ---- T-241: an urgent message cuts a working turn ---------------------

    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_herdr::AgentStatus;
    use gear_model::{MsgKind, MsgState};
    use gear_store::NewMessage;

    const CAP_PANE: &str = "w1:p1";
    const LANE_PANE: &str = "w1:p2";
    const REVIEW_PANE: &str = "w1:p3";
    const REVIEW_NAME: &str = "p-review-t1";

    /// Captain, one lane (`pi`, queues input while it works) and one temp
    /// reviewer, all with an active binding.
    fn interrupt_world() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "pi", T0)?;
                testing::seed_temp_seat(tx, "p", "r-1", "T-1", REVIEW_PANE, "pi", T0)?;
                Ok(())
            })
            .unwrap();
        store
    }

    /// An open message for `recipient`, urgent or not. Returns its id.
    fn message_for(tx: &Tx<'_>, recipient: &str, urgent: bool) -> GearResult<String> {
        Ok(tx
            .insert_message(&NewMessage {
                project: "p".into(),
                thread: "t".into(),
                task: None,
                attempt: None,
                sender: "gear:captain".into(),
                sender_gen: 0,
                recipient: recipient.into(),
                kind: MsgKind::Instruction,
                urgent,
                body: "please look".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T0,
                meta: None,
            })
            .unwrap()
            .id)
    }

    /// An Intended doorbell for `seat` covering `message`.
    fn doorbell_for(tx: &Tx<'_>, seat: &str, message: &str) -> GearResult<String> {
        Ok(tx
            .insert_op(&NewOp {
                project: "p".into(),
                seat: seat.into(),
                generation: 1,
                kind: OpKind::Doorbell,
                task: None,
                attempt: None,
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({
                    "text": "GEAR: 1 item(s) waiting for you. Run the command `gear` (no arguments) in your own shell; it shows only your items.",
                    "covers": [message]
                }),
                created: T0 + 1,
            })
            .unwrap()
            .id)
    }

    fn prompt_or_esc(h: &FakeHerdr, pane: &str, no_esc_message: &str) {
        let esc = h.calls.iter().position(|c| {
            matches!(c, Call::SendKeys { pane: p, keys } if p == pane && keys == &["esc".to_string()])
        });
        let prompt = h.calls.iter().position(|c| matches!(c, Call::Prompt { pane: p, .. } if p == pane));
        match (esc, prompt) {
            (None, Some(_)) => assert_eq!(no_esc_message, "", "{no_esc_message}"),
            (Some(e), Some(p)) => assert!(e < p, "the escape goes before the prompt"),
            (esc, prompt) => panic!("unexpected calls: esc={esc:?} prompt={prompt:?}"),
        }
    }

    fn review_herdr(status: AgentStatus) -> FakeHerdr {
        let mut h = FakeHerdr::new();
        h.add_agent_named(REVIEW_PANE, "pi", status, REVIEW_NAME);
        h
    }

    #[test]
    fn an_urgent_doorbell_to_a_working_lane_cuts_the_turn_first() {
        let mut store = interrupt_world();
        store
            .write(|tx| {
                let id = message_for(tx, "p:heat", true)?;
                doorbell_for(tx, "p:heat", &id)?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(LANE_PANE, "pi", AgentStatus::Working, "p-heat");

        let done = execute_intended(&mut store, &mut herdr, "p", T0 + 2).unwrap();

        assert_eq!(done, vec![("O-1".to_string(), "submitted".to_string())]);
        prompt_or_esc(&herdr, LANE_PANE, "a working lane is cut before its urgent doorbell");
    }

    #[test]
    fn a_non_urgent_doorbell_to_a_working_lane_sends_no_escape() {
        let mut store = interrupt_world();
        store
            .write(|tx| {
                let id = message_for(tx, "p:heat", false)?;
                doorbell_for(tx, "p:heat", &id)?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(LANE_PANE, "pi", AgentStatus::Working, "p-heat");

        execute_intended(&mut store, &mut herdr, "p", T0 + 2).unwrap();

        assert!(!herdr.calls.iter().any(|c| matches!(c, Call::SendKeys { .. })), "only an urgent doorbell cuts a turn");
    }

    #[test]
    fn an_urgent_doorbell_to_an_idle_lane_sends_no_escape() {
        let mut store = interrupt_world();
        store
            .write(|tx| {
                let id = message_for(tx, "p:heat", true)?;
                doorbell_for(tx, "p:heat", &id)?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(LANE_PANE, "pi", AgentStatus::Idle, "p-heat");

        let done = execute_intended(&mut store, &mut herdr, "p", T0 + 2).unwrap();

        assert_eq!(done, vec![("O-1".to_string(), "submitted".to_string())]);
        assert!(!herdr.calls.iter().any(|c| matches!(c, Call::SendKeys { .. })), "an idle lane has no turn to cut");
    }

    #[test]
    fn an_urgent_doorbell_to_the_captain_sends_no_escape() {
        let mut store = interrupt_world();
        store
            .write(|tx| {
                let id = message_for(tx, "p:captain", true)?;
                doorbell_for(tx, "p:captain", &id)?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(CAP_PANE, "codex", AgentStatus::Working, "p-captain");

        let done = execute_intended(&mut store, &mut herdr, "p", T0 + 2).unwrap();

        assert_eq!(done, vec![("O-1".to_string(), "submitted".to_string())]);
        assert!(!herdr.calls.iter().any(|c| matches!(c, Call::SendKeys { .. })), "the captain's turn is not cut");
    }

    #[test]
    fn an_urgent_send_to_a_working_reviewer_cuts_the_turn_first() {
        let mut store = interrupt_world();
        let message = store
            .write(|tx| message_for(tx, "p:r-1", true))
            .unwrap();
        let mut herdr = review_herdr(AgentStatus::Working);

        let pane = deliver_doorbell_now(&mut store, &mut herdr, "p", "p:r-1", 1, &message, true, T0 + 2)
            .unwrap();

        assert_eq!(pane.as_deref(), Some(REVIEW_PANE));
        prompt_or_esc(&herdr, REVIEW_PANE, "a working reviewer is cut before its urgent send");
    }

    // ---- T-371: a delivery that sits unsent in the input box ---------------

    const UNSENT_TEXT: &str = "Continue T-1: run `gear` first";

    /// A lane Deliver op carrying `text`, Submitted, created `created`.
    fn unsent_delivery(tx: &Tx<'_>, text: &str, created: Ts) -> GearResult<String> {
        let op = tx.insert_op(&NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: OpKind::Deliver,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({ "text": text }),
            created,
        })?;
        tx.set_op_state("p", &op.id, OpState::Executing, None, created)?;
        tx.set_op_state("p", &op.id, OpState::Submitted, None, created)?;
        Ok(op.id)
    }

    /// A `pi` agent at [`LANE_PANE`] in `status` whose screen is `tail`.
    fn unsent_herdr(status: AgentStatus, tail: &str) -> FakeHerdr {
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(LANE_PANE, "pi", status);
        herdr.set_screen(LANE_PANE, tail);
        herdr
    }

    fn enters(herdr: &FakeHerdr, pane: &str) -> usize {
        herdr
            .calls
            .iter()
            .filter(|call| {
                matches!(call, Call::SendKeys { pane: p, keys } if p == pane && keys == &["enter".to_string()])
            })
            .count()
    }

    #[test]
    fn an_unsent_delivery_gets_one_enter_and_never_a_second() {
        // M-4569: the packet was typed and the Enter was lost; the lane sat
        // idle with the text in its input box. After 30 s the engine presses
        // the key once and records it.
        let mut store = lane_world();
        store.write(|tx| unsent_delivery(tx, UNSENT_TEXT, T0)).unwrap();
        let mut herdr = unsent_herdr(AgentStatus::Idle, "> earlier output\n> Continue T-1: run `gear` first");

        let sent = resubmit_unsent(&mut store, &mut herdr, "p", T0 + 40).unwrap();

        assert_eq!(sent, vec!["O-1".to_string()]);
        assert_eq!(enters(&herdr, LANE_PANE), 1, "the Enter is pressed once");
        assert!(
            herdr.prompts_to(LANE_PANE).is_empty(),
            "the text is in the input box: it is not typed again"
        );
        assert_eq!(
            store.read(|tx| tx.get_meta("engine.resubmit.O-1")).unwrap(),
            Some((T0 + 40).to_string()),
            "the press is recorded with its time"
        );

        let again = resubmit_unsent(&mut store, &mut herdr, "p", T0 + 50).unwrap();

        assert!(again.is_empty(), "the op got its Enter already");
        assert_eq!(enters(&herdr, LANE_PANE), 1, "a second pass sends no key");
    }

    #[test]
    fn a_delivery_only_20s_old_gets_no_enter() {
        // The Enter may still be on its way, or the lane may be about to press
        // it itself: nothing is pressed in the first 30 s.
        let mut store = lane_world();
        store.write(|tx| unsent_delivery(tx, UNSENT_TEXT, T0)).unwrap();
        let mut herdr = unsent_herdr(AgentStatus::Idle, "> Continue T-1: run `gear` first");

        let sent = resubmit_unsent(&mut store, &mut herdr, "p", T0 + 20).unwrap();

        assert!(sent.is_empty());
        assert_eq!(enters(&herdr, LANE_PANE), 0);
        assert_eq!(store.read(|tx| tx.get_meta("engine.resubmit.O-1")).unwrap(), None);
    }

    /// A codex lane whose delivery never reached the input: a named agent
    /// (a prompt to an unnamed agent is refused) and a screen with the empty
    /// input placeholder.
    fn unsent_lost_herdr(status: AgentStatus) -> FakeHerdr {
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(LANE_PANE, "opencode", status, "p-heat");
        herdr.set_screen(LANE_PANE, "> \n  ? for shortcuts, 0 tokens");
        herdr
    }

    #[test]
    fn a_delivery_that_never_reached_the_input_is_typed_again_once() {
        // T-404: the op stayed Submitted while the pane showed an empty input
        // and the agent sat idle — nobody had retyped the lost delivery.
        let mut store = lane_world();
        store.write(|tx| unsent_delivery(tx, UNSENT_TEXT, T0)).unwrap();
        let mut herdr = unsent_lost_herdr(AgentStatus::Idle);

        let sent = resubmit_unsent(&mut store, &mut herdr, "p", T0 + 90).unwrap();

        assert_eq!(sent, vec!["O-1".to_string()]);
        assert_eq!(
            herdr.prompts_to(LANE_PANE),
            vec![UNSENT_TEXT],
            "the delivery is typed again"
        );
        assert_eq!(enters(&herdr, LANE_PANE), 0, "the prompt carries its own Enter");
        assert_eq!(
            store.read(|tx| tx.get_meta("engine.retype.O-1")).unwrap(),
            Some((T0 + 90).to_string()),
            "the retype is recorded with its time"
        );
        assert_eq!(
            store.read(|tx| tx.get_meta("engine.resubmit.O-1")).unwrap(),
            None,
            "the Enter path was not the one that ran"
        );

        let again = resubmit_unsent(&mut store, &mut herdr, "p", T0 + 120).unwrap();

        assert!(again.is_empty(), "the op was typed again already");
        assert_eq!(herdr.prompts_to(LANE_PANE).len(), 1, "a second pass types nothing");
    }

    #[test]
    fn a_delivery_younger_than_the_window_is_not_typed_again() {
        // The text may still be on its way: nothing is typed in the window.
        let mut store = lane_world();
        store.write(|tx| unsent_delivery(tx, UNSENT_TEXT, T0)).unwrap();
        let mut herdr = unsent_lost_herdr(AgentStatus::Idle);

        let sent = resubmit_unsent(&mut store, &mut herdr, "p", T0 + 20).unwrap();

        assert!(sent.is_empty());
        assert!(herdr.prompts_to(LANE_PANE).is_empty());
        assert_eq!(store.read(|tx| tx.get_meta("engine.retype.O-1")).unwrap(), None);
    }

    #[test]
    fn a_refused_retype_changes_nothing() {
        let mut store = lane_world();
        store.write(|tx| unsent_delivery(tx, UNSENT_TEXT, T0)).unwrap();
        let mut herdr = unsent_lost_herdr(AgentStatus::Idle);

        herdr.inject_next = Some(gear_herdr::fake::Inject::Refuse { code: "injected".into() });
        let sent = resubmit_unsent(&mut store, &mut herdr, "p", T0 + 90).unwrap();

        assert!(sent.is_empty(), "a refused prompt is no retype");
        assert_eq!(
            store.read(|tx| tx.get_meta("engine.retype.O-1")).unwrap(),
            None,
            "and it is not recorded"
        );
    }

    #[test]
    fn a_working_agent_gets_no_enter() {
        let mut store = lane_world();
        store.write(|tx| unsent_delivery(tx, UNSENT_TEXT, T0)).unwrap();
        let mut herdr = unsent_herdr(AgentStatus::Working, "> Continue T-1: run `gear` first");

        let sent = resubmit_unsent(&mut store, &mut herdr, "p", T0 + 40).unwrap();

        assert!(sent.is_empty(), "a working agent is not interrupted");
        assert_eq!(enters(&herdr, LANE_PANE), 0);
        assert!(herdr.prompts_to(LANE_PANE).is_empty());
    }

    #[test]
    fn a_working_agent_with_an_empty_input_is_not_typed_again() {
        let mut store = lane_world();
        store.write(|tx| unsent_delivery(tx, UNSENT_TEXT, T0)).unwrap();
        let mut herdr = unsent_lost_herdr(AgentStatus::Working);

        let sent = resubmit_unsent(&mut store, &mut herdr, "p", T0 + 90).unwrap();

        assert!(sent.is_empty(), "a working agent is not interrupted");
        assert!(herdr.prompts_to(LANE_PANE).is_empty(), "nor typed into");
        assert_eq!(enters(&herdr, LANE_PANE), 0);
    }

    #[test]
    fn a_confirmed_delivery_is_never_touched() {
        let mut store = lane_world();
        store
            .write(|tx| {
                let id = unsent_delivery(tx, UNSENT_TEXT, T0)?;
                tx.set_op_state("p", &id, OpState::Confirmed, Some("view p:heat/1"), T0 + 5)
            })
            .unwrap();
        let mut herdr = unsent_herdr(AgentStatus::Idle, "> Continue T-1: run `gear` first");

        let sent = resubmit_unsent(&mut store, &mut herdr, "p", T0 + 40).unwrap();

        assert!(sent.is_empty(), "the lane read its packet");
        assert_eq!(enters(&herdr, LANE_PANE), 0);
        assert!(herdr.prompts_to(LANE_PANE).is_empty(), "a confirmed op is not typed again");
        assert_eq!(store.read(|tx| tx.get_meta("engine.resubmit.O-1")).unwrap(), None);
        assert_eq!(store.read(|tx| tx.get_meta("engine.retype.O-1")).unwrap(), None);
    }

    #[test]
    fn a_failed_read_or_a_refused_key_changes_nothing() {
        let mut store = lane_world();
        store.write(|tx| unsent_delivery(tx, UNSENT_TEXT, T0)).unwrap();
        let mut herdr = unsent_herdr(AgentStatus::Idle, "> Continue T-1: run `gear` first");

        herdr.observe_fails = true;
        assert!(resubmit_unsent(&mut store, &mut herdr, "p", T0 + 40).unwrap().is_empty());
        assert_eq!(enters(&herdr, LANE_PANE), 0);
        assert_eq!(store.read(|tx| tx.get_meta("engine.resubmit.O-1")).unwrap(), None);
        herdr.observe_fails = false;

        herdr.inject_next = Some(gear_herdr::fake::Inject::Refuse { code: "injected".into() });
        assert!(
            resubmit_unsent(&mut store, &mut herdr, "p", T0 + 40).unwrap().is_empty(),
            "a refused key changes nothing"
        );
        assert_eq!(store.read(|tx| tx.get_meta("engine.resubmit.O-1")).unwrap(), None);

        // The next pass still finds the text and presses the key (the fake
        // logged the refused attempt, so the successful one is the second).
        let before = enters(&herdr, LANE_PANE);
        let sent = resubmit_unsent(&mut store, &mut herdr, "p", T0 + 50).unwrap();
        assert_eq!(sent, vec!["O-1".to_string()]);
        assert_eq!(enters(&herdr, LANE_PANE), before + 1, "the retry presses it again");
    }
}
