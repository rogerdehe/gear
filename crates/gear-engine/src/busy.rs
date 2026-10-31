//! T-46: a Working lane with no development attempt cannot take ready work.
//! Persist the continuous-condition timer and one alert per incident. Submitted
//! attempts belong to review, so they do not count as lane development work.

use crate::{
    dispatch,
    observe::{Presence, SeatObs},
};
use gear_herdr::AgentStatus;
use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{NewMessage, SeatRow, Tx};

fn key(seat: &str) -> String {
    format!("lane-busy-idle:{seat}")
}

/// Unknown observation interrupts the timer without claiming recovery or
/// forgetting an already-issued alert. The next known idle/task/queue state
/// decides whether that incident has actually ended.
pub fn observation_lost(tx: &Tx<'_>, project: &str) -> GearResult<()> {
    for seat in tx
        .seats_of_project(project)?
        .iter()
        .filter(|s| s.role == SeatRole::Lane)
    {
        tx.set_meta(&format!("{}:since", key(&seat.id)), "")?;
    }
    Ok(())
}

pub fn check(tx: &Tx<'_>, project: &str, observed: &[SeatObs], now: Ts) -> GearResult<Vec<String>> {
    let config = tx
        .get_project(project)?
        .and_then(|p| gear_app::config::parse(&p.config_toml).ok());
    let threshold = i64::from(
        config
            .map(|c| c.timing.lane_busy_idle_min)
            .unwrap_or(10)
            .max(1),
    ) * 60;
    let mut alerted = Vec::new();
    for obs in observed.iter().filter(|o| o.seat.role == SeatRole::Lane) {
        let Some(seat) = tx.get_seat(&obs.seat.id)? else {
            continue;
        };
        let key = key(&seat.id);
        let generation = seat.generation.to_string();
        if tx.get_meta(&format!("{key}:generation"))?.as_deref() != Some(&generation) {
            recover(tx, project, &key, now)?;
            tx.set_meta(&format!("{key}:generation"), &generation)?;
        }
        // A lane helping on an unanswered captain instruction (T-324, T-323)
        // is busy on purpose: it does not qualify, so it goes the recover way
        // as any lane that does not.
        if gear_board::idle::helping(tx, project, &seat, now)?.is_some()
            || !qualifies(tx, project, &seat, obs)?
        {
            recover(tx, project, &key, now)?;
            continue;
        }
        let since_key = format!("{key}:since");
        let since = tx.get_meta(&since_key)?.and_then(|s| s.parse::<Ts>().ok());
        let since = match since {
            Some(since) if since <= now => since,
            _ => {
                tx.set_meta(&since_key, &now.to_string())?;
                now
            }
        };
        if now - since <= threshold {
            continue;
        }
        let Some(captain) = tx.captain_of(project)? else {
            continue;
        };
        if tx
            .get_meta(&format!("{key}:alert"))?
            .is_some_and(|s| !s.is_empty())
        {
            if !crate::attention::due(
                tx,
                project,
                &format!("lane-busy:{}", seat.id),
                &captain.id,
                now,
            )? {
                continue;
            }
        }

        let message = tx.insert_message(&NewMessage {
            meta: None,
            project: project.into(),
            thread: format!("lane-busy:{}", seat.id),
            task: None,
            attempt: None,
            sender: "engine".into(),
            sender_gen: 0,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent: true,
            body: format!("lane {} is busy with no task; ready work waits", seat.name),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        tx.set_meta(&format!("{key}:alert"), &message.id)?;
        alerted.push(seat.id);
    }
    Ok(alerted)
}

/// The condition both monitors count (T-46, T-288): a present, Working lane
/// of the current generation that holds no development attempt, while ready
/// work waits for it.
fn qualifies(tx: &Tx<'_>, project: &str, seat: &SeatRow, obs: &SeatObs) -> GearResult<bool> {
    if obs.presence != Presence::Present
        || obs.status != Some(AgentStatus::Working)
        || !matches!(seat.state, SeatState::Active | SeatState::Standby)
        || obs.seat.generation != seat.generation
        || !tx.in_progress_attempts_of_seat(&seat.id)?.is_empty()
    {
        return Ok(false);
    }
    for task in dispatch::ready_tasks_of(tx, seat)? {
        if !tx
            .attempts_of_task(project, &task.id)?
            .iter()
            .any(|a| a.state.occupies_seat())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// T-288: the same condition, five minutes old, ends the lane's turn once, so
/// the next packet can go. A lane that is helping (an unanswered captain
/// instruction, T-323) is busy on purpose and is never interrupted.
///
/// The pane of every lane the caller has to interrupt is returned; the latch
/// `:interrupted` holds the `:since` value it was interrupted for, so later
/// ticks of the same condition stay quiet and a new condition interrupts
/// again.
pub fn due_interrupts(
    tx: &Tx<'_>,
    project: &str,
    observed: &[SeatObs],
    now: Ts,
) -> GearResult<Vec<String>> {
    const INTERRUPT_AFTER_S: i64 = 300;
    let mut panes = Vec::new();
    for obs in observed.iter().filter(|o| o.seat.role == SeatRole::Lane) {
        let Some(seat) = tx.get_seat(&obs.seat.id)? else {
            continue;
        };
        if !qualifies(tx, project, &seat, obs)? {
            continue;
        }
        let key = key(&seat.id);
        let Some(since) = tx
            .get_meta(&format!("{key}:since"))?
            .and_then(|s| s.parse::<Ts>().ok())
        else {
            continue;
        };
        if now - since < INTERRUPT_AFTER_S {
            continue;
        }
        let since = since.to_string();
        if tx.get_meta(&format!("{key}:interrupted"))?.as_deref() == Some(&since) {
            continue;
        }
        if gear_board::idle::helping(tx, project, &seat, now)?.is_some() {
            continue;
        }
        let Some(binding) = tx.get_binding(&seat.id, seat.generation)? else {
            continue;
        };
        tx.set_meta(&format!("{key}:interrupted"), &since)?;
        panes.push(binding.pane_id);
    }
    Ok(panes)
}

fn recover(tx: &Tx<'_>, project: &str, key: &str, now: Ts) -> GearResult<()> {
    if let Some(id) = tx
        .get_meta(&format!("{key}:alert"))?
        .filter(|s| !s.is_empty())
    {
        if let Some(message) = tx.get_message(project, &id)? {
            if matches!(message.state, MsgState::Open | MsgState::Waiting) {
                tx.update_message_state(
                    project,
                    &id,
                    MsgState::Resolved,
                    Some(
                        "lane no longer busy without a development attempt while ready work waits",
                    ),
                    None,
                    None,
                    now,
                )?;
            }
        }
    }
    tx.set_meta(&format!("{key}:since"), "")?;
    tx.set_meta(&format!("{key}:alert"), "")?;
    Ok(())
}
