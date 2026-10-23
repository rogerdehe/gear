//! Keep the lanes fed (the soul of GEAR: lanes never stop).
//!
//! Forecast the remaining ready/current work using measured lane development
//! time; warn within about one average task. Without measurements, retain the
//! ready-count warning. One active item per lane, with persistent severity:
//! repeats stay quiet even after acknowledgment; worsening gets a fresh id.
//! Recovery, parked/held lanes and the captain's tail declaration close it.

use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{MessageRow, NewMessage, Store, Tx};

use crate::dispatch;

/// The ready tasks a lane needs before it is out of danger.
const ENOUGH_READY: usize = 2;

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
        let state_key = format!("feed-severity:{}", seat.id);
        if !matches!(seat.state, SeatState::Active | SeatState::Standby)
            || gear_app::plan::lane_in_tail(tx, project, &seat.name)?
        {
            tx.set_meta(&state_key, "0")?;
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
        let current = tx.in_progress_attempts_of_seat(&seat.id)?;
        let working = !current.is_empty();
        let average = average_task_seconds(tx, project, &seat.id)?;
        let horizon = match average {
            Some(average) => {
                let mut seconds = ready as i64 * average;
                for attempt in &current {
                    let times = tx.attempt_times(project, &attempt.task, attempt.n)?;
                    let elapsed = times.started.map(|start| (now - start).max(0)).unwrap_or(0);
                    seconds += (average - elapsed).max(0);
                }
                Some(seconds)
            }
            None => None,
        };
        let recovered = ready >= ENOUGH_READY
            || horizon.zip(average).is_some_and(|(left, average)| left > average);
        if recovered {
            tx.set_meta(&state_key, "0")?;
            if close(tx, project, &seat.name, &captain.id,
                &format!("the queue is refilled ({ready} ready)"), now)? {
                changes.push(FeedChange { lane: seat.name.clone(), action: "closed" });
            }
            continue;
        }
        let severity = if ready == 0 && !working { 3 } else if ready == 0 { 2 } else { 1 };
        let previous = tx.get_meta(&state_key)?.and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
        // The captain may resolve an alert without refilling immediately. The
        // same condition stays acknowledged until it worsens or recovers.
        let body = if ready == 0 && !working {
            format!("lane {} is IDLE", seat.name)
        } else if let Some(seconds) = horizon {
            format!("lane {} runs dry in ~{} min", seat.name, (seconds + 59) / 60)
        } else {
            format!(
                "lane {} has {} ready {} left",
                seat.name,
                ready,
                if ready == 1 { "task" } else { "tasks" }
            )
        };
        if severity <= previous {
            if severity < previous {
                // Improve the existing description silently; only worsening
                // is a new fact that needs another doorbell.
                if let Some(item) = open_item(tx, project, &seat.name, &captain.id)? {
                    tx.update_message_body(project, &item.id, &body)?;
                }
                tx.set_meta(&state_key, &severity.to_string())?;
            }
            continue;
        }
        let mut action = "opened";
        if let Some(item) = open_item(tx, project, &seat.name, &captain.id)? {
            if previous == 0 {
                // An alert from an older engine is the same incident.
                if item.body != body { tx.update_message_body(project, &item.id, &body)?; }
                tx.set_meta(&state_key, &severity.to_string())?;
                continue;
            }
            // A worsening must have a fresh id: a view of the earlier alert
            // must not hide the new fact from the captain's doorbell.
            tx.update_message_state(project, &item.id, MsgState::Resolved,
                Some("the shortage worsened"), None, None, now)?;
            action = "updated";
        }
        tx.insert_message(&NewMessage {
            project: project.to_string(), thread: thread(&seat.name),
            task: None, attempt: None, sender: "engine".to_string(), sender_gen: 0,
            recipient: captain.id.clone(), kind: MsgKind::System, urgent: false,
            body, reply_to: None, needs_ack: false, state: MsgState::Open, created: now,
        })?;
        tx.set_meta(&state_key, &severity.to_string())?;
        changes.push(FeedChange { lane: seat.name.clone(), action });
    }
    Ok(changes)
}

/// Average measured lane work per handed-in attempt. Review/captain waiting
/// does not consume a lane's development capacity; rework is another sample.
fn average_task_seconds(tx: &Tx<'_>, project: &str, seat: &str) -> GearResult<Option<i64>> {
    let mut total = 0;
    let mut samples = 0;
    for task in tx.tasks_of_project(project)? {
        for attempt in tx.attempts_of_task(project, &task.id)? {
            if attempt.seat != seat { continue; }
            let times = tx.attempt_times(project, &task.id, attempt.n)?;
            if let (Some(start), Some(end)) = (times.started, times.handed_in) {
                if end > start {
                    total += end - start;
                    samples += 1;
                }
            }
        }
    }
    Ok((samples > 0).then(|| (total + samples - 1) / samples))
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
            // A captain who defers the item (Waiting) still owns the same
            // lane item: deferral is not a new incident (R2 of the review).
            matches!(message.state, MsgState::Open | MsgState::Waiting)
                && message.kind == MsgKind::System
                && message.recipient == captain
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
