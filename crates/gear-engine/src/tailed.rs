//! Every lane in the tail while required work remains (B02-08, T-117).
//!
//! `- tail:` in the plan silences a lane's dry/idle warnings until it is
//! cleared — it is for a lane with no valuable work left, never for a
//! dependency wait. A captain who tailed **every** lane during a wait and
//! forgot turned every warning off: the lanes stand idle and nothing says so
//! (the trial's B02-08).
//!
//! This step opens **one** item in the captain's inbox while that state
//! lasts — naming the tailed lanes and the required tasks that are ready or
//! waiting — adds no second one on a later tick, and closes it when the tail
//! is cleared or no required task remains.

use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, TaskState, Ts};
use gear_store::{MessageRow, NewMessage, Store, Tx};

/// The thread of the all-tail item (one per project).
fn thread() -> &'static str {
    "all-tail"
}

/// The project's all-tail **state** marker: "1" while the state has been
/// announced. It outlives the message: a captain who resolves the item has
/// acknowledged it, not ended the state (F1 of the T-117 review — resolving
/// used to bring a fresh item on the next tick).
fn active_key(project: &str) -> String {
    format!("all-tail-active:{project}")
}

/// What the step did in one tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The state began: the item was opened.
    Opened,
    /// The state ended: the item was closed.
    Closed,
}

impl Change {
    pub fn as_str(&self) -> &'static str {
        match self {
            Change::Opened => "opened",
            Change::Closed => "closed",
        }
    }
}

/// One step of the watch, in the engine's own transaction.
pub fn watch(store: &mut Store, project: &str, now: Ts) -> GearResult<Option<Change>> {
    store.write(|tx| watch_in(tx, project, now))
}

/// The work over an open transaction: open the item when every lane is tailed
/// and required work remains, close it when that stops being true.
fn watch_in(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Option<Change>> {
    let Some(captain) = tx.captain_of(project)? else {
        return Ok(None); // no captain: nobody to tell
    };
    let lanes = lane_names(tx, project)?;
    let mut tailed = Vec::new();
    for lane in &lanes {
        if gear_app::plan::lane_in_tail(tx, project, lane)? {
            tailed.push(lane.clone());
        }
    }
    let all_tailed = !lanes.is_empty() && tailed.len() == lanes.len();
    let required = required_work(tx, project)?;
    let open = open_item(tx, project, &captain.id)?;
    let announced = tx
        .get_meta(&active_key(project))?
        .is_some_and(|value| !value.is_empty());
    if all_tailed && !required.is_empty() {
        if let Some(item) = open {
            let current = body(&tailed, &required);
            if item.body != current {
                tx.update_message_body(project, &item.id, &current)?;
            }
            if !announced {
                tx.set_meta(&active_key(project), "1")?;
            }
            return Ok(None);
        }
        if announced && !crate::attention::due(tx, project, thread(), &captain.id, now)? {
            return Ok(None);
        }
        tx.set_meta(&active_key(project), "1")?;
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.to_string(),
            thread: thread().to_string(),
            task: None,
            attempt: None,
            sender: "engine".to_string(),
            sender_gen: 0,
            recipient: captain.id.clone(),
            kind: MsgKind::System,
            // An information item: it needs no ack and it does not ring like a
            // stuck lane — but it waits in the inbox until the state ends.
            urgent: false,
            body: body(&tailed, &required),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        return Ok(Some(Change::Opened));
    }
    // The state ended: forget it, so a later one is announced again — whether
    // or not the captain still has the item open.
    if announced {
        tx.set_meta(&active_key(project), "")?;
    }
    let Some(item) = open else { return Ok(None) };
    let reason = if all_tailed {
        "no required task remains"
    } else {
        "the tail is cleared"
    };
    tx.update_message_state(
        project,
        &item.id,
        MsgState::Resolved,
        Some(reason),
        None,
        None,
        now,
    )?;
    Ok(Some(Change::Closed))
}

/// The lanes of the project that can work: its lane seats, retired ones left
/// out (a retired lane takes no work, so its tail cannot silence anything).
fn lane_names(tx: &Tx<'_>, project: &str) -> GearResult<Vec<String>> {
    Ok(tx
        .seats_of_project(project)?
        .into_iter()
        .filter(|seat| seat.role == SeatRole::Lane && seat.state != SeatState::Retired)
        .map(|seat| seat.name)
        .collect())
}

/// The required work a lane would have to take: the tasks marked `required`
/// that are ready or waiting (a running or verifying one is in a lane's
/// hands, a finished one is gone).
fn required_work(tx: &Tx<'_>, project: &str) -> GearResult<Vec<String>> {
    Ok(tx
        .tasks_of_project(project)?
        .into_iter()
        .filter(|task| task.required)
        .filter(|task| matches!(task.state, TaskState::Ready | TaskState::Waiting))
        .map(|task| format!("{} ({})", task.id, task.state.as_str()))
        .collect())
}

/// The body names the tailed lanes and the required work they wait for.
fn body(lanes: &[String], required: &[String]) -> String {
    format!(
        "every lane is in the tail ({}) while required work remains: {}. A tailed lane gets no dry/idle warning, so nothing will say that work is standing. Clear the tail with `- tail: -` (or name the lanes that should work) when a lane should take it.",
        lanes.join(", "),
        required.join(", ")
    )
}

/// The project's open all-tail item, if there is one. A captain who defers it
/// (Waiting) still owns the same item: the state has not changed.
fn open_item(tx: &Tx<'_>, project: &str, captain: &str) -> GearResult<Option<MessageRow>> {
    Ok(tx
        .thread_messages(project, thread())?
        .into_iter()
        .find(|message| {
            matches!(message.state, MsgState::Open | MsgState::Waiting)
                && message.recipient == captain
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_model::{SeatRole, TaskState};
    use gear_store::testing::{seed_active_seat, seed_project};
    use gear_store::{Store, TaskRow};

    const T0: i64 = 1_800_000_000;

    /// A project with two lanes and the required task T-1 in `state`.
    fn world(state: TaskState) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                seed_project(tx, "p", "sess", T0)?;
                seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p9", "claude", T0)?;
                seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p1", "opencode", T0)?;
                seed_active_seat(tx, "p", "wire", SeatRole::Lane, "w1:p2", "opencode", T0)?;
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-1".into(),
                    rank: 1,
                    state,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: None,
                    required: true,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                    issue: None,
                    updated: T0,
                })?;
                Ok(())
            })
            .unwrap();
        store
    }

    /// The captain's all-tail items, open and closed.
    fn items(store: &mut Store) -> Vec<gear_store::MessageRow> {
        store.read(|tx| tx.thread_messages("p", thread())).unwrap()
    }

    fn open_items(store: &mut Store) -> Vec<gear_store::MessageRow> {
        items(store)
            .into_iter()
            .filter(|message| message.state == MsgState::Open)
            .collect()
    }

    fn tail(store: &mut Store, lanes: &[&str]) {
        store
            .write(|tx| {
                for lane in lanes {
                    tx.set_meta(&format!("lane-tail:p:{lane}"), "1")?;
                }
                Ok(())
            })
            .unwrap();
    }

    /// B02-08: every lane tailed with a required task left → one item; a
    /// second tick adds none; clearing the tail resolves it.
    #[test]
    fn all_lanes_tailed_with_required_work_opens_one_item_and_clearing_the_tail_closes_it() {
        let mut store = world(TaskState::Ready);
        tail(&mut store, &["heat", "wire"]);

        assert_eq!(
            watch(&mut store, "p", T0 + 1).unwrap(),
            Some(Change::Opened)
        );
        let open = open_items(&mut store);
        assert_eq!(open.len(), 1, "{open:?}");
        let item = &open[0];
        assert!(!item.urgent, "an information item, not a ringing alarm");
        assert!(
            item.body.contains("heat, wire"),
            "it names the tailed lanes: {}",
            item.body
        );
        assert!(
            item.body.contains("T-1 (ready)"),
            "it names the required task: {}",
            item.body
        );

        // A second tick adds nothing.
        assert_eq!(
            watch(&mut store, "p", T0 + 2).unwrap(),
            None,
            "one item per state"
        );
        assert_eq!(open_items(&mut store).len(), 1);

        // One lane out of the tail: the state ended, the item is resolved.
        store
            .write(|tx| tx.set_meta("lane-tail:p:wire", "0"))
            .unwrap();
        assert_eq!(
            watch(&mut store, "p", T0 + 3).unwrap(),
            Some(Change::Closed)
        );
        let closed = items(&mut store);
        assert_eq!(closed.len(), 1, "no new item: the same one closed");
        assert_eq!(closed[0].state, MsgState::Resolved);
        assert!(
            closed[0]
                .result
                .as_deref()
                .unwrap_or_default()
                .contains("tail is cleared"),
            "{:?}",
            closed[0].result
        );

        // Every lane tailed again with the work still there: a new item.
        tail(&mut store, &["heat", "wire"]);
        assert_eq!(
            watch(&mut store, "p", T0 + 4).unwrap(),
            Some(Change::Opened)
        );
        assert_eq!(open_items(&mut store).len(), 1);
        assert_eq!(items(&mut store).len(), 2, "the state began again");
    }

    /// The state also ends when no required task remains: the work the tail
    /// hid is gone.
    #[test]
    fn no_required_task_left_closes_the_item() {
        let mut store = world(TaskState::Ready);
        tail(&mut store, &["heat", "wire"]);
        assert_eq!(
            watch(&mut store, "p", T0 + 1).unwrap(),
            Some(Change::Opened)
        );

        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-1")?.unwrap();
                task.state = TaskState::Done;
                tx.update_task(&task)
            })
            .unwrap();
        assert_eq!(
            watch(&mut store, "p", T0 + 2).unwrap(),
            Some(Change::Closed)
        );
        let items = items(&mut store);
        assert_eq!(items[0].state, MsgState::Resolved);
        assert!(
            items[0]
                .result
                .as_deref()
                .unwrap_or_default()
                .contains("no required task"),
            "{:?}",
            items[0].result
        );
    }

    /// Only **every** lane in the tail counts: one lane free is no state.
    #[test]
    fn a_lane_out_of_the_tail_opens_nothing() {
        let mut store = world(TaskState::Ready);
        tail(&mut store, &["heat"]);
        assert_eq!(watch(&mut store, "p", T0 + 1).unwrap(), None);
        assert!(open_items(&mut store).is_empty());

        // A waiting required task counts too — a lane is needed for it when
        // its dependency clears.
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-1")?.unwrap();
                task.state = TaskState::Waiting;
                tx.update_task(&task)
            })
            .unwrap();
        tail(&mut store, &["wire"]);
        assert_eq!(
            watch(&mut store, "p", T0 + 2).unwrap(),
            Some(Change::Opened)
        );
        let open = open_items(&mut store);
        assert!(open[0].body.contains("T-1 (waiting)"), "{}", open[0].body);
    }

    /// A project without a lane has nothing to silence.
    #[test]
    fn a_project_without_lanes_opens_nothing() {
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                for lane in ["heat", "wire"] {
                    tx.set_meta(&format!("lane-tail:p:{lane}"), "1")?;
                }
                for seat in tx.seats_of_project("p")? {
                    if seat.role == SeatRole::Lane {
                        tx.set_seat_state(&seat.id, SeatState::Retired, Some("done"), T0)?;
                    }
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(watch(&mut store, "p", T0 + 1).unwrap(), None);
        assert!(open_items(&mut store).is_empty());
    }

    #[test]
    fn persistent_fact_rings_again_and_changed_fact_updates_same_item() {
        let mut store = world(TaskState::Ready);
        tail(&mut store, &["heat", "wire"]);
        assert_eq!(
            watch(&mut store, "p", T0 + 1).unwrap(),
            Some(Change::Opened)
        );
        let original = open_items(&mut store).remove(0);
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-1")?.unwrap();
                task.state = TaskState::Waiting;
                tx.update_task(&task)
            })
            .unwrap();
        watch(&mut store, "p", T0 + 2).unwrap();
        let updated = open_items(&mut store).remove(0);
        assert_eq!(updated.id, original.id);
        assert_ne!(updated.body, original.body);
        store
            .write(|tx| {
                tx.update_message_state(
                    "p",
                    &updated.id,
                    MsgState::Resolved,
                    Some("seen"),
                    None,
                    None,
                    T0 + 3,
                )
            })
            .unwrap();
        assert_eq!(watch(&mut store, "p", T0 + 902).unwrap(), None);
        assert_eq!(
            watch(&mut store, "p", T0 + 903).unwrap(),
            Some(Change::Opened)
        );
        assert_eq!(open_items(&mut store).len(), 1);
    }

    /// F1 of the review: resolving the item is an acknowledgement, not the end
    /// of the state — the next tick adds nothing while the lanes are still
    /// tailed and the same work still waits. Clearing the tail ends the state;
    /// a later one is announced again.
    #[test]
    fn a_resolved_item_is_not_reopened_while_the_state_lasts() {
        let mut store = world(TaskState::Waiting);
        tail(&mut store, &["heat", "wire"]);
        assert_eq!(
            watch(&mut store, "p", T0 + 1).unwrap(),
            Some(Change::Opened)
        );
        let item = open_items(&mut store).remove(0);

        // The captain resolves it — what `gear reply M-n --resolve` does.
        store
            .write(|tx| {
                tx.update_message_state(
                    "p",
                    &item.id,
                    MsgState::Resolved,
                    Some("seen"),
                    None,
                    None,
                    T0 + 2,
                )
            })
            .unwrap();

        // The same state: no new item, nothing reported.
        assert_eq!(
            watch(&mut store, "p", T0 + 3).unwrap(),
            None,
            "the same incident"
        );
        let all = items(&mut store);
        assert_eq!(all.len(), 1, "no second item: {all:?}");
        assert_eq!(all[0].state, MsgState::Resolved);

        // The tail is cleared: the state ends (nothing open to close), and a
        // tail set again is a new state — exactly one new item.
        store
            .write(|tx| tx.set_meta("lane-tail:p:wire", "0"))
            .unwrap();
        assert_eq!(watch(&mut store, "p", T0 + 4).unwrap(), None);
        tail(&mut store, &["wire"]);
        assert_eq!(
            watch(&mut store, "p", T0 + 5).unwrap(),
            Some(Change::Opened)
        );
        let all = items(&mut store);
        assert_eq!(all.len(), 2, "the new state is one new item: {all:?}");
        assert_eq!(all[1].state, MsgState::Open);
    }
}
