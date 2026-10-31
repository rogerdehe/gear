//! Stale captain action items: promote, then remind once per period (T-40).
//! The reminder clock is persisted atomically with doorbell intent, so views,
//! restarts and ticks cannot multiply reminders for the same message.

use gear_model::{GearResult, MsgState, SeatRole, Ts};
use gear_store::{MessageRow, Store, Tx};

pub(crate) fn period(tx: &Tx<'_>, project: &str) -> GearResult<i64> {
    let config = tx
        .get_project(project)?
        .and_then(|row| gear_app::config::parse(&row.config_toml).ok());
    Ok(i64::from(
        config
            .map(|c| c.timing.captain_item_stale_min)
            .unwrap_or(15)
            .max(1),
    ) * 60)
}

/// Whether the last alert in an ongoing incident can be raised again.
pub(crate) fn due(
    tx: &Tx<'_>,
    project: &str,
    thread: &str,
    captain: &str,
    now: Ts,
) -> GearResult<bool> {
    let last = tx
        .thread_messages(project, thread)?
        .into_iter()
        .filter(|m| {
            m.sender == "engine" && m.recipient == captain && m.kind == gear_model::MsgKind::System
        })
        .max_by_key(|m| (m.created, m.resolved.unwrap_or(0)));
    match last {
        Some(m) if matches!(m.state, MsgState::Open | MsgState::Waiting) => Ok(false),
        Some(m) => {
            let interval = period(tx, project)?;
            Ok(m.resolved.is_some_and(|t| now - t >= interval))
        }
        None => Ok(true),
    }
}

fn key(project: &str, message: &str) -> String {
    format!("captain-reminder:{project}:{message}")
}

fn stale(message: &MessageRow, period: i64, now: Ts) -> bool {
    // T-51 overlap warnings ring once per shared-file set. A changed set
    // creates a fresh message; ordinary stale reminders would defeat that latch.
    // T-169's friction item waits in the inbox until its reports are closed:
    // the ordinary stale reminder would turn it urgent and ring the captain a
    // second time for the same reports (design §6.4).
    !(message.kind == gear_model::MsgKind::System
        && (message.thread.starts_with("parallel:") || message.thread == "friction"))
        && message.state == MsgState::Open
        && now - message.created >= period
}

pub(crate) fn promote(store: &mut Store, project: &str, now: Ts) -> GearResult<()> {
    store.write(|tx| {
        let Some(captain) = tx.captain_of(project)? else {
            return Ok(());
        };
        let period = period(tx, project)?;
        for message in tx.actionable_for(project, &captain.id, now)? {
            if !message.urgent && stale(&message, period, now) {
                tx.mark_message_urgent(project, &message.id)?;
            }
        }
        Ok(())
    })
}

/// For stale items, the period replaces the ordinary "not yet shown" rule.
/// Waiting items retain the normal due-time/view behavior; a deliberate
/// deferral is not an ignored open action item.
pub(crate) fn should_show(
    tx: &Tx<'_>,
    message: &MessageRow,
    shown: bool,
    period: i64,
    now: Ts,
) -> GearResult<bool> {
    if message.kind == gear_model::MsgKind::System {
        if let Some(lane) = message.thread.strip_prefix("lane:") {
            if gear_app::plan::lane_in_tail(tx, &message.project, lane)? {
                return Ok(false);
            }
        }
    }
    if !stale(message, period, now) {
        return Ok(!shown);
    }
    let last = tx
        .get_meta(&key(&message.project, &message.id))?
        .and_then(|value| value.parse::<Ts>().ok());
    Ok(last.is_none_or(|last| now - last >= period))
}

pub(crate) fn record(
    tx: &Tx<'_>,
    action: &crate::doorbell::DoorbellAction,
    now: Ts,
) -> GearResult<()> {
    if !tx
        .get_seat(&action.seat)?
        .is_some_and(|s| s.role == SeatRole::Captain)
    {
        return Ok(());
    }
    let period = period(tx, &action.project)?;
    for id in &action.covers {
        if tx
            .get_message(&action.project, id)?
            .is_some_and(|m| stale(&m, period, now))
        {
            tx.set_meta(&key(&action.project, id), &now.to_string())?;
        }
    }
    Ok(())
}
