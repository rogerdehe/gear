//! Declared task waits and their round clocks (design §5.3–5.4).
use crate::EngineCfg;
use gear_herdr::HerdrAct;
use gear_model::{AttemptState, GearResult, MsgKind, MsgState, OpKind, OpState, Ts};
use gear_store::{NewMessage, Store, Tx};

/// A wait belongs only to the lane's current, unfinished attempt.
pub(crate) fn waiting_seat(tx: &Tx<'_>, seat: &str) -> GearResult<bool> {
    for attempt in tx.open_attempts_of_seat(seat)? {
        if matches!(attempt.state, AttemptState::Started | AttemptState::Stopped)
            && tx
                .get_task(&attempt.project, &attempt.task)?
                .is_some_and(|task| task.wait_on.is_some())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Reconcile declared waits and their seat pause rows.
fn reconcile(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<()> {
    for task in tx.tasks_of_project(project)? {
        let Some(on) = task.wait_on.as_deref() else {
            continue;
        };
        let Some(attempt) = tx
            .attempts_of_task(project, &task.id)?
            .into_iter()
            .rev()
            .find(|a| matches!(a.state, AttemptState::Started | AttemptState::Stopped))
        else {
            continue;
        };
        tx.open_pause(
            project,
            &attempt.seat,
            "wait",
            &format!("waiting for {on}"),
            now,
        )?;
        crate::stall::resolve(tx, project, &attempt.seat, now, None)?;
        // A packet or overrun doorbell intended before the wait must not be
        // typed into the lane after it declares the wait.
        for op in tx.slot_ops(&attempt.seat)? {
            if op.state != OpState::Intended {
                continue;
            }
            let overrun_doorbell = if op.kind == OpKind::Doorbell {
                let mut found = false;
                if let Some(ids) = op.payload.get("covers").and_then(|v| v.as_array()) {
                    for id in ids.iter().filter_map(|v| v.as_str()) {
                        if tx
                            .get_message(project, id)?
                            .is_some_and(|m| m.thread.starts_with("overrun-question:"))
                        {
                            found = true;
                        }
                    }
                }
                found
            } else {
                false
            };
            if (op.kind == OpKind::Deliver && op.task.as_deref() == Some(task.id.as_str()))
                || overrun_doorbell
            {
                tx.set_op_state(
                    project,
                    &op.id,
                    OpState::Void,
                    Some("task is in a declared wait"),
                    now,
                )?;
            }
        }
        // A wait on something outside GEAR (T-440) ends when the lane sends
        // any later message on the task but another blocked one, or when the
        // time the wait was given runs out. Nothing merges to wake it.
        if task.wait_cond.as_deref() == Some("external") {
            let key = gear_app::mail::external_wait_key(project, &task.id);
            let until = tx.get_meta(&key)?.and_then(|s| s.parse::<Ts>().ok());
            let declared = until
                .map(|at| at - gear_app::mail::EXTERNAL_WAIT_MIN * 60)
                .unwrap_or(0);
            let answered = tx
                .messages_from_since(project, &attempt.seat, declared)?
                .iter()
                .any(|m| m.task.as_deref() == Some(task.id.as_str()) && m.kind != MsgKind::Blocked);
            let expired = until.is_some_and(|at| now >= at);
            if answered || expired {
                tx.clear_task_wait(project, &task.id)?;
                tx.close_pause(project, &attempt.seat, "wait", now)?;
                tx.set_meta(&key, "")?;
                if expired && !answered {
                    tx.insert_message(&NewMessage {
                        project: project.into(), thread: format!("wait-external-end:{}:{}", task.id, attempt.n),
                        task: Some(task.id.clone()), attempt: Some(attempt.n), sender: "engine".into(), sender_gen: 0,
                        recipient: attempt.seat.clone(), kind: MsgKind::Instruction, urgent: true,
                        body: format!("GEAR: your external wait on {} ended after {} min. Push what you have, or declare the wait again with its reason.", task.id, gear_app::mail::EXTERNAL_WAIT_MIN),
                        reply_to: None, needs_ack: false, state: MsgState::Open, created: now, meta: None,
                    })?;
                }
            }
            continue;
        }
        // A verdict can mark a task Done before the merge actually lands.
        // Only a merged attempt is evidence that the awaited work arrived.
        let mut landed = None;
        for candidate in tx.attempts_of_task(project, on)?.into_iter().rev() {
            if let (Some(at), Some(sha)) = (
                tx.attempt_times(project, on, candidate.n)?.merged,
                candidate.candidate.as_deref(),
            ) {
                if at <= now && !sha.is_empty() {
                    landed = Some((at, sha.to_string()));
                    break;
                }
            }
        }
        if let Some((at, sha)) = landed {
            tx.clear_task_wait(project, &task.id)?;
            tx.close_pause(project, &attempt.seat, "wait", at.min(now))?;
            tx.insert_message(&NewMessage {
                project: project.into(),
                thread: format!("wait:{}:{}:{on}", task.id, attempt.n),
                task: Some(task.id.clone()),
                attempt: Some(attempt.n),
                sender: "engine".into(),
                sender_gen: 0,
                recipient: attempt.seat.clone(),
                kind: MsgKind::Instruction,
                urgent: true,
                body: format!("GEAR: {on} is merged ({sha}). Continue {}.", task.id),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: now,
                meta: None,
            })?;
        }
    }
    for pause in tx.pauses_of_project(project)? {
        if pause.kind == "wait" && pause.end_at.is_none() && !waiting_seat(tx, &pause.seat)? {
            tx.close_pause(project, &pause.seat, "wait", now)?;
        }
    }
    Ok(())
}

/// Process the declared waits of every project.
pub fn tick(
    store: &mut Store,
    _herdr: &mut dyn HerdrAct,
    _cfg: &EngineCfg,
    now: Ts,
) -> GearResult<()> {
    let projects = store.read(|tx| tx.project_ids())?;
    for project in projects {
        store.write(|tx| reconcile(tx, &project, now))?;
    }
    Ok(())
}
