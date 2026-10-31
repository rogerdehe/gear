//! Keep the lanes fed (the soul of GEAR: lanes never stop).
//!
//! Forecast the remaining ready/current work using measured lane development
//! time; warn within about one average task. Without measurements, retain the
//! ready-count warning. One active item per lane, with persistent severity:
//! repeats stay quiet after acknowledgment until the reminder interval.
//! Recovery, parked/held lanes and the captain's tail declaration close it.
//!
//! Two more facts of the same kind are watched here, every tick:
//!
//! - a lane that is **one task from dry** (it works on its last task and has
//!   nothing startable behind it) is told to the captain, not only at plan
//!   apply (§10.7 row B3, T-66);
//! - the **imbalance**: a lane with nothing left next to a lane with a pile
//!   of work that has not started (§8.4) — one item for the project.
//!
//! When no lane of the project has work left to start, none of it is told:
//! the lane items close with that fact and the project stays quiet (T-232).

use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, TaskState, Ts};
use gear_store::{MessageRow, NewMessage, Store, Tx};

use crate::dispatch;

/// The ready tasks a lane needs before it is out of danger.
const ENOUGH_READY: usize = 2;

/// The thread the imbalance item lives in (§8.4).
const BALANCE_THREAD: &str = "balance";

/// Why a lane item closes when the whole project has nothing left to start.
const NO_WORK_LEFT: &str = "no lane has work left to start";

/// What happened to one lane this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedChange {
    pub lane: String,
    /// `opened`, `updated` or `closed`.
    pub action: &'static str,
}

/// Watch every lane and keep one item per lane in the captain's inbox.
pub fn keep_lanes_fed(store: &mut Store, project: &str, now: Ts) -> GearResult<Vec<FeedChange>> {
    store.write(|tx| feed(tx, project, now))
}

/// The work, over an open transaction.
fn feed(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Vec<FeedChange>> {
    let Some(captain) = tx.captain_of(project)? else {
        return Ok(Vec::new()); // no captain: nobody to warn
    };
    // Nothing startable anywhere in the project: no lane can run dry, and one
    // item per lane would ring for every lane at once (M4.1). Every lane item
    // closes and the project stays quiet (T-232).
    if startable_tasks(tx, project)? == 0 {
        return close_all_lanes(tx, project, &captain.id, now);
    }
    let mut changes = Vec::new();
    // The lanes that are one task from dry: the engine checks this every
    // tick, not only at plan apply (§10.7 row B3, T-66).
    let about_to_dry = gear_app::plan::idle_lanes(tx, project, now)?;
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
                changes.push(FeedChange {
                    lane: seat.name.clone(),
                    action: "closed",
                });
            }
            continue;
        }
        // A lane helping on an unanswered instruction of the captain (T-324,
        // T-323) is busy on purpose: it opens no IDLE item, and the open one
        // closes with the fact, as for a parked lane.
        if let Some(help) = gear_board::idle::helping(tx, project, &seat, now)? {
            tx.set_meta(&state_key, "0")?;
            if close(
                tx,
                project,
                &seat.name,
                &captain.id,
                &format!("the lane is {}", help.text()),
                now,
            )? {
                changes.push(FeedChange {
                    lane: seat.name.clone(),
                    action: "closed",
                });
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
        // One task from dry: the lane works on its last task and has nothing
        // startable behind it (§10.7 row B3). It is a warning about work in
        // flight: when the lane stops working, the idle state is the fact
        // (F2 of the T-162 review).
        let dry = about_to_dry
            .iter()
            .find(|idle| idle.lane == seat.name)
            .filter(|idle| idle.current.is_some())
            .filter(|_| working);
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
        let recovered = dry.is_none()
            && (ready >= ENOUGH_READY
                || horizon
                    .zip(average)
                    .is_some_and(|(left, average)| left > average));
        if recovered {
            tx.set_meta(&state_key, "0")?;
            if close(
                tx,
                project,
                &seat.name,
                &captain.id,
                &format!("the queue is refilled ({ready} ready)"),
                now,
            )? {
                changes.push(FeedChange {
                    lane: seat.name.clone(),
                    action: "closed",
                });
            }
            continue;
        }
        let severity = if ready == 0 && !working {
            // The lane stands idle: the last fact, and worse than the warning
            // about the work in flight (F2).
            3
        } else if dry.is_some() {
            2
        } else if let (Some(left), Some(average)) = (horizon, average) {
            // Taking the final queued task does not itself shorten the
            // forecast. Escalate when runway halves, then when actually idle.
            if left <= average / 2 {
                2
            } else {
                1
            }
        } else if ready == 0 {
            2
        } else {
            1
        };
        let previous = tx
            .get_meta(&state_key)?
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(0);
        // The captain may resolve an alert without refilling immediately. The
        // same condition stays acknowledged until it worsens, recovers, or is due again.
        let body = if ready == 0 && !working {
            // T-293: the IDLE item says what it means, not only the state.
            format!("lane {} is IDLE: no next task to dispatch (its agent may still be finishing a turn); plan a task for it or move one to it", seat.name)
        } else if let Some(dry) = dry {
            // The same words the plan apply uses: the lane has nothing to
            // start after its current task.
            dry.warning().trim_start_matches("warning: ").to_string()
        } else if let Some(seconds) = horizon {
            format!(
                "lane {} runs dry in ~{} min",
                seat.name,
                (seconds + 59) / 60
            )
        } else {
            format!(
                "lane {} has {} ready {} left",
                seat.name,
                ready,
                if ready == 1 { "task" } else { "tasks" }
            )
        };
        if let Some(item) = open_item(tx, project, &seat.name, &captain.id)? {
            if item.body != body {
                tx.update_message_body(project, &item.id, &body)?;
            }
            tx.set_meta(&state_key, &severity.to_string())?;
            continue;
        }
        if previous != 0
            && severity <= previous
            && !crate::attention::due(tx, project, &thread(&seat.name), &captain.id, now)?
        {
            tx.set_meta(&state_key, &severity.to_string())?;
            continue;
        }
        let action = "opened";
        tx.insert_message(&NewMessage {
            meta: None,
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
        tx.set_meta(&state_key, &severity.to_string())?;
        changes.push(FeedChange {
            lane: seat.name.clone(),
            action,
        });
    }
    // The imbalance of the lanes (§8.4): one item for the project.
    if let Some(change) = imbalance(tx, project, &captain.id, now)? {
        changes.push(change);
    }
    Ok(changes)
}

/// The tasks a lane could start now: the loop's own rule (`ready`), a task
/// with a seat-occupying attempt already taken is not available. Zero means
/// the project has no work left to start at all (T-232).
fn startable_tasks(tx: &Tx<'_>, project: &str) -> GearResult<usize> {
    let mut count = 0;
    for seat in tx.seats_of_project(project)? {
        if seat.role != SeatRole::Lane
            || !matches!(seat.state, SeatState::Active | SeatState::Standby)
        {
            continue;
        }
        for task in dispatch::ready_tasks_of(tx, &seat)? {
            let taken = tx
                .attempts_of_task(project, &task.id)?
                .iter()
                .any(|attempt| attempt.state.occupies_seat());
            if !taken {
                count += 1;
            }
        }
    }
    Ok(count)
}

/// No lane has work left to start: every lane item closes with that fact and
/// the severity is forgotten, so nothing rings while the project is empty
/// (T-232).
fn close_all_lanes(
    tx: &Tx<'_>,
    project: &str,
    captain: &str,
    now: Ts,
) -> GearResult<Vec<FeedChange>> {
    let mut changes = Vec::new();
    for seat in tx.seats_of_project(project)? {
        if seat.role != SeatRole::Lane {
            continue;
        }
        tx.set_meta(&format!("feed-severity:{}", seat.id), "0")?;
        if close(tx, project, &seat.name, captain, NO_WORK_LEFT, now)? {
            changes.push(FeedChange {
                lane: seat.name.clone(),
                action: "closed",
            });
        }
    }
    Ok(changes)
}

/// The imbalance item of §8.4: a lane with nothing left next to a lane with
/// a pile of work that has not started.
///
/// One item for the project, on the thread `balance`: it is updated while the
/// state lasts and closed when it ends — never one item per pair of lanes.
fn imbalance(tx: &Tx<'_>, project: &str, captain: &str, now: Ts) -> GearResult<Option<FeedChange>> {
    let mut idle: Option<String> = None;
    let mut busy: Option<(String, usize, i64)> = None;
    for seat in tx.seats_of_project(project)? {
        if seat.role != SeatRole::Lane || seat.state == SeatState::Retired {
            continue;
        }
        // A lane in the captain's tail is idle on purpose: it is no imbalance.
        if gear_app::plan::lane_in_tail(tx, project, &seat.name)? {
            continue;
        }
        // The lane's own reason (§8.2): nothing left in its queue.
        if matches!(
            gear_board::idle::idle_reason(tx, project, &seat.name, now)?,
            Some(gear_board::idle::IdleReason::NoWork)
        ) {
            if idle.is_none() {
                idle = Some(seat.name.clone());
            }
            continue;
        }
        // The work of the lane that has not started yet.
        let (count, minutes) = not_started(tx, project, &seat.name)?;
        if count < 2 || minutes <= 60 {
            continue;
        }
        // The lane with the most work left is the one to move work from.
        let better = match &busy {
            None => true,
            Some((_, tasks, left)) => (count, minutes) > (*tasks, *left),
        };
        if better {
            busy = Some((seat.name.clone(), count, minutes));
        }
    }
    let (Some(idle), Some((busy_lane, count, minutes))) = (idle, busy) else {
        // The imbalance is over: the item closes and the fact is forgotten, so
        // a later imbalance is told again (F1).
        tx.set_meta(&balance_key(project), "")?;
        let Some(item) = open_balance(tx, project, captain)? else {
            return Ok(None);
        };
        tx.update_message_state(
            project,
            &item.id,
            MsgState::Resolved,
            Some("the imbalance is over"),
            None,
            None,
            now,
        )?;
        return Ok(Some(FeedChange {
            lane: "balance".to_string(),
            action: "closed",
        }));
    };
    let body = format!(
        "{idle} has nothing left; {busy_lane} has {count} tasks ({minutes}m) left: move some (lane ownership allowing) or plan more for {idle}."
    );
    // The incident: the idle lane and the lane it would take work from. It
    // survives the item's resolution, so the same imbalance is not told again
    // (F1).
    let incident = format!("{idle}|{busy_lane}");
    let told = tx.get_meta(&balance_key(project))?.unwrap_or_default();
    match open_balance(tx, project, captain)? {
        Some(item) => {
            // The same item, updated: the fact is not repeated (§8.4).
            if item.body != body {
                tx.update_message_body(project, &item.id, &body)?;
            }
            tx.set_meta(&balance_key(project), &incident)?;
            Ok(Some(FeedChange {
                lane: idle,
                action: "updated",
            }))
        }
        None => {
            if !told.is_empty()
                && !crate::attention::due(tx, project, BALANCE_THREAD, captain, now)?
            {
                // The captain resolved the item and the imbalance is the same:
                // the fact is not told again.
                return Ok(None);
            }
            tx.set_meta(&balance_key(project), &incident)?;
            tx.insert_message(&NewMessage {
                meta: None,
                project: project.to_string(),
                thread: BALANCE_THREAD.to_string(),
                task: None,
                attempt: None,
                sender: "engine".to_string(),
                sender_gen: 0,
                recipient: captain.to_string(),
                kind: MsgKind::System,
                urgent: false,
                body,
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: now,
            })?;
            Ok(Some(FeedChange {
                lane: idle,
                action: "opened",
            }))
        }
    }
}

/// The key of the imbalance incident in the engine's meta: the fact that was
/// told, so the captain is not told the same imbalance twice (F1).
fn balance_key(project: &str) -> String {
    format!("feed-balance:{project}")
}

/// The work of a lane that has not started yet: the tasks that are ready or
/// wait for a dependency, with their estimates in minutes.
fn not_started(tx: &Tx<'_>, project: &str, lane: &str) -> GearResult<(usize, i64)> {
    let mut count = 0;
    let mut minutes = 0;
    for task in tx.tasks_of_project(project)? {
        if task.lane.as_deref() != Some(lane) {
            continue;
        }
        if !matches!(task.state, TaskState::Ready | TaskState::Waiting) {
            continue;
        }
        count += 1;
        minutes += gear_board::voyage::estimate_of(tx, project, &task)?.unwrap_or(0);
    }
    Ok((count, minutes))
}

/// The open imbalance item of the project, if there is one.
fn open_balance(tx: &Tx<'_>, project: &str, captain: &str) -> GearResult<Option<MessageRow>> {
    Ok(tx
        .thread_messages(project, BALANCE_THREAD)?
        .into_iter()
        .find(|message| {
            matches!(message.state, MsgState::Open | MsgState::Waiting)
                && message.kind == MsgKind::System
                && message.recipient == captain
        }))
}

/// Average measured lane work per handed-in attempt. Review/captain waiting
/// does not consume a lane's development capacity; rework is another sample.
fn average_task_seconds(tx: &Tx<'_>, project: &str, seat: &str) -> GearResult<Option<i64>> {
    let mut total = 0;
    let mut samples = 0;
    for task in tx.tasks_of_project(project)? {
        for attempt in tx.attempts_of_task(project, &task.id)? {
            if attempt.seat != seat {
                continue;
            }
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
