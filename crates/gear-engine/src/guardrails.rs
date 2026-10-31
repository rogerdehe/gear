//! A lane refused by its own guardrails reaches the captain (T-113).
//!
//! The same flag is raised by a startup dialog that waits for an answer
//! (T-457): Codex 0.165's `/new` asks where the new conversation should run
//! while Herdr reports the agent done/idle.
//!
//! Pi runs `@aliou/pi-guardrails`: the `permission-gate` extension refuses a
//! dangerous command (`Command auto-denied: …`) and the `path-access`
//! extension refuses a write outside the workspace (`Access to <path> is
//! blocked (outside working directory).`). The agent goes on with the
//! refusal as a tool result, so nothing about the seat's state says it was
//! told to do something it may not do. The profile carries the two deny
//! lines as markers; the engine reads them off the screen and tells the
//! captain once per spell, with the screen lines that say it.

use crate::observe::{Presence, SeatObs};
use gear_herdr::ScreenFlag;
use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{NewMessage, Tx};

/// The thread a seat's guardrails item lives in.
fn thread(seat: &str) -> String {
    format!("guardrails:{seat}")
}

fn key(seat: &str) -> String {
    format!("seat-guardrails:{seat}")
}

/// Ring the captain for every lane or reviewer seat whose screen shows a
/// guardrails refusal. Returns the seat ids that were rung.
///
/// One item per spell: the item is raised when the refusal first shows and
/// is resolved when a later screen read shows no refusal any more. A screen
/// that was not read (the agent works) changes nothing — a fact that was not
/// looked at is not a fact that went away.
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
        let refused = obs.presence == Presence::Present
            && obs.flags.contains(&ScreenFlag::Blocked)
            && matches!(
                seat.state,
                SeatState::Active | SeatState::Standby | SeatState::Retiring
            );
        if !refused {
            // The screen was read and shows no refusal (or the seat is gone):
            // the spell is over.
            if obs.screen.is_some() {
                recover(tx, project, &key, now)?;
            }
            continue;
        }
        if tx
            .get_meta(&format!("{key}:alert"))?
            .is_some_and(|s| !s.is_empty())
        {
            continue; // this spell already has its item
        }
        let Some(captain) = tx.captain_of(project)? else {
            continue; // no captain: nobody to tell
        };
        let screen = obs.screen.as_deref().unwrap_or_default().trim();
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
            urgent: false,
            body: format!(
                "the screen of seat {} shows a stop: its own guardrails refused an action, or a startup dialog waits for an answer (T-457). Look at its pane and its last task, then resolve this item. Last screen lines:\n{}",
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

/// The refusal is gone: its item is resolved, so a later refusal rings again.
fn recover(tx: &Tx<'_>, project: &str, key: &str, now: Ts) -> GearResult<()> {
    let alert = tx.get_meta(&format!("{key}:alert"))?.unwrap_or_default();
    if alert.is_empty() {
        return Ok(()); // nothing to recover: no write
    }
    if let Some(message) = tx.get_message(project, &alert)? {
        if matches!(message.state, MsgState::Open | MsgState::Waiting) {
            tx.update_message_state(
                project,
                &alert,
                MsgState::Resolved,
                Some("the guardrails refused nothing any more"),
                None,
                None,
                now,
            )?;
        }
    }
    tx.set_meta(&format!("{key}:alert"), "")?;
    Ok(())
}
