//! Keep the lanes fed (the soul of GEAR: lanes never stop).
//!
//! Each tick, for every lane that may take work, count its ready tasks.
//! Fewer than two is the danger zone: the lane stops soon unless the captain
//! adds work. The captain gets one action item in the inbox — "lane <x> has
//! <n> ready tasks left", or "lane <x> is IDLE" when it has nothing left and
//! nothing running — so the doorbell rings before the lane stops.
//!
//! One item per lane, found by its thread, so a second tick or a restarted
//! engine creates no duplicate. The item is updated while the condition
//! lasts and closed when the queue is refilled. A lane that is parked, held
//! or retiring is not warned about: it is not supposed to take work.

use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{MessageRow, NewMessage, Store, Tx};

use crate::dispatch;

/// The ready tasks a lane needs before it is out of danger.
const ENOUGH_READY: usize = 2;

/// After the captain settles an item, the condition is not reported again for
/// this long — no endless nagging (design/11).
const REPEAT_AFTER_S: i64 = 10 * 60;

/// What happened to one lane this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedChange {
    pub lane: String,
    /// `opened`, `updated` or `closed`.
    pub action: &'static str,
}

/// Watch every lane and keep one item per lane in the captain's inbox.
pub fn keep_lanes_fed(
    store: &mut Store,
    project: &str,
    now: Ts,
) -> GearResult<Vec<FeedChange>> {
    store.write(|tx| feed(tx, project, now))
}

/// The work, over an open transaction.
fn feed(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Vec<FeedChange>> {
    let Some(captain) = tx.captain_of(project)? else {
        return Ok(Vec::new()); // no captain: nobody to warn
    };
    let mut changes = Vec::new();
    for seat in tx.seats_of_project(project)? {
        if seat.role != SeatRole::Lane {
            continue;
        }
        // A lane that is parked or held must not keep a warning: close it
        // (R3 of the T-7 review).
        if !matches!(seat.state, SeatState::Active | SeatState::Standby) {
            if close(
                tx,
                project,
                &seat.name,
                &captain.id,
                "the lane is parked or held",
                now,
            )? {
                changes.push(FeedChange { lane: seat.name.clone(), action: "closed" });
            }
            continue;
        }
        // Ready work, less what a delivery already took: a task with a
        // Delivered/started attempt is not available any more (R4).
        let mut ready = 0;
        for task in dispatch::ready_tasks_of(tx, &seat)? {
            let taken = tx
                .attempts_of_task(project, &task.id)?
                .iter()
                .any(|attempt| attempt.state.occupies_seat());
            if !taken {
                ready += 1;
            }
        }
        let working = !tx.in_progress_attempts_of_seat(&seat.id)?.is_empty();
        if ready >= ENOUGH_READY {
            if close(
                tx,
                project,
                &seat.name,
                &captain.id,
                &format!("the queue is refilled ({ready} ready)"),
                now,
            )? {
                changes.push(FeedChange { lane: seat.name.clone(), action: "closed" });
            }
            continue;
        }
        let body = if ready == 0 && !working {
            format!("lane {} is IDLE", seat.name)
        } else {
            format!(
                "lane {} has {} ready {} left",
                seat.name,
                ready,
                if ready == 1 { "task" } else { "tasks" }
            )
        };
        match open_item(tx, project, &seat.name, &captain.id)? {
            Some(item) => {
                if item.body != body {
                    tx.update_message_body(project, &item.id, &body)?;
                    changes.push(FeedChange { lane: seat.name.clone(), action: "updated" });
                }
            }
            None => {
                if cooling_down(tx, project, &seat.name, now)? {
                    continue;
                }
                tx.insert_message(&NewMessage {
                    project: project.to_string(),
                    thread: thread(&seat.name),
                    task: None,
                    attempt: None,
                    sender: "engine".to_string(),
                    sender_gen: 0,
                    recipient: captain.id.clone(),
                    kind: MsgKind::System,
                    urgent: false,
                    body,
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: now,
                })?;
                changes.push(FeedChange { lane: seat.name.clone(), action: "opened" });
            }
        }
    }
    Ok(changes)
}

/// The thread one lane's item lives in.
fn thread(lane: &str) -> String {
    format!("lane:{lane}")
}

/// The lane's open item, if there is one.
fn open_item(
    tx: &Tx<'_>,
    project: &str,
    lane: &str,
    captain: &str,
) -> GearResult<Option<MessageRow>> {
    Ok(tx
        .thread_messages(project, &thread(lane))?
        .into_iter()
        .find(|message| {
            message.state == MsgState::Open
                && message.kind == MsgKind::System
                && message.recipient == captain
        }))
}

/// Whether the lane's last item was settled so recently that reporting the
/// condition again would be nagging.
fn cooling_down(tx: &Tx<'_>, project: &str, lane: &str, now: Ts) -> GearResult<bool> {
    let last = tx
        .thread_messages(project, &thread(lane))?
        .into_iter()
        .last();
    Ok(last.is_some_and(|message| {
        message.resolved.is_some_and(|resolved| now - resolved < REPEAT_AFTER_S)
    }))
}

/// Close the lane's open item once its queue is refilled; true when one was
/// closed.
fn close(
    tx: &Tx<'_>,
    project: &str,
    lane: &str,
    captain: &str,
    reason: &str,
    now: Ts,
) -> GearResult<bool> {
    let Some(item) = open_item(tx, project, lane, captain)? else {
        return Ok(false);
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
    Ok(true)
}
