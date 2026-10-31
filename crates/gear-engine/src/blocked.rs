//! A seat blocked by a dialog reaches the captain (lesson 7b).
//!
//! On 2026-10-25 a Codex reviewer sat on its start-up "Update available"
//! dialog: Herdr reported the agent `blocked`, nothing in the engine noticed
//! and nobody was told. Now a lane or reviewer seat that stays `blocked` for
//! more than two minutes gets one item in its captain's inbox, with the last
//! screen lines — once per blocked spell. When the seat works again the item
//! is resolved and the timer is reset, so a later spell rings again.

use crate::observe::{Presence, SeatObs};
use gear_herdr::AgentStatus;
use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{NewMessage, Tx};

/// How long a seat may stay blocked before the captain is rung.
pub const BLOCKED_S: i64 = 120;

/// The thread a seat's blocked item lives in.
fn thread(seat: &str) -> String {
    format!("blocked:{seat}")
}

fn key(seat: &str) -> String {
    format!("seat-blocked:{seat}")
}

/// Whether the seat is in a blocked spell right now (its timer was started by
/// the last tick and not cleared by a recovery). A delivery that is intended
/// for it waits instead of being typed into the dialog (T-68).
pub fn holding(tx: &Tx<'_>, seat: &str) -> GearResult<bool> {
    Ok(tx
        .get_meta(&format!("{}:since", key(seat)))?
        .is_some_and(|since| !since.is_empty()))
}

/// Ring the captain for every lane or reviewer seat that has been blocked for
/// more than [`BLOCKED_S`] seconds. Returns the seat ids that were rung.
///
/// The timer and the alert live in the store's meta, so they survive a
/// restart and a new spell rings again.
pub fn check(tx: &Tx<'_>, project: &str, observed: &[SeatObs], now: Ts) -> GearResult<Vec<String>> {
    let mut rung = Vec::new();
    for obs in observed
        .iter()
        .filter(|o| matches!(o.seat.role, SeatRole::Lane | SeatRole::Temp))
    {
        let Some(seat) = tx.get_seat(&obs.seat.id)? else {
            continue;
        };
        let key = key(&seat.id);
        let blocked = obs.presence == Presence::Present
            && obs.status == Some(AgentStatus::Blocked)
            && matches!(
                seat.state,
                SeatState::Active | SeatState::Standby | SeatState::Retiring
            );
        if !blocked {
            recover(tx, project, &key, now)?;
            continue;
        }
        let since_key = format!("{key}:since");
        let since = match tx.get_meta(&since_key)?.and_then(|s| s.parse::<Ts>().ok()) {
            Some(since) if since <= now => since,
            _ => {
                tx.set_meta(&since_key, &now.to_string())?;
                now
            }
        };
        if now - since <= BLOCKED_S {
            continue; // not blocked for long enough yet
        }
        let Some(captain) = tx.captain_of(project)? else {
            continue; // no captain: nobody to tell
        };
        let screen = obs.screen.as_deref().unwrap_or_default().trim();
        if tx
            .get_meta(&format!("{key}:alert"))?
            .is_some_and(|s| !s.is_empty())
        {
            if !crate::attention::due(tx, project, &thread(&seat.id), &captain.id, now)? {
                if let Some(id) = tx.get_meta(&format!("{key}:alert"))? {
                    if let Some(item) = tx.get_message(project, &id)? {
                        let screen = obs.screen.as_deref().unwrap_or_default().trim();
                        let body = format!("seat {} has been blocked for more than {BLOCKED_S} seconds: its agent waits on a dialog and cannot work. Look at its pane, answer the dialog (or restart the agent), then resolve this item. Last screen lines:\n{}", seat.name, screen);
                        if matches!(item.state, MsgState::Open | MsgState::Waiting)
                            && item.body != body
                        {
                            tx.update_message_body(project, &item.id, &body)?;
                        }
                    }
                }
                continue;
            }
        }

        let message = tx.insert_message(&NewMessage {
            meta: None,
            project: project.into(),
            thread: thread(&seat.id),
            task: None,
            attempt: None,
            sender: "engine".into(),
            sender_gen: 0,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent: true,
            body: format!(
                "seat {} has been blocked for more than {BLOCKED_S} seconds: its agent waits on a dialog and cannot work. Look at its pane, answer the dialog (or restart the agent), then resolve this item. Last screen lines:\n{}",
                seat.name, screen
            ),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        tx.set_meta(&format!("{key}:alert"), &message.id)?;
        rung.push(seat.id);
    }
    Ok(rung)
}

/// The seat is not blocked any more: its item is resolved and the timer is
/// reset, so a later blocked spell rings the captain again.
fn recover(tx: &Tx<'_>, project: &str, key: &str, now: Ts) -> GearResult<()> {
    let alert = tx.get_meta(&format!("{key}:alert"))?.unwrap_or_default();
    let since = tx.get_meta(&format!("{key}:since"))?.unwrap_or_default();
    if alert.is_empty() && since.is_empty() {
        return Ok(()); // nothing to recover: no write
    }
    if !alert.is_empty() {
        if let Some(message) = tx.get_message(project, &alert)? {
            if matches!(message.state, MsgState::Open | MsgState::Waiting) {
                tx.update_message_state(
                    project,
                    &alert,
                    MsgState::Resolved,
                    Some("the seat works again"),
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
