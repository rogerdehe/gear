//! A lane the engine flagged reaches the captain (lesson L-52).
//!
//! The engine flags a seat when a delivery to it is never confirmed or when
//! continuing it made no progress. Until 2026-10-23 a flag went only to
//! engine.log: the lane stood still and nobody knew. Now every flagged seat
//! gets one action item in its captain's inbox (so the doorbell rings), kept
//! until the captain resolves it; a second flag of the same seat while the
//! item is open adds nothing.
//!
//! T-75: a seat whose provider is in a back-off is not stuck. It waits for
//! the provider — minutes without any output — and no stall item is raised
//! for it. The screen is the only evidence, and the profile of the seat's
//! kind says what a back-off looks like.

use crate::observe::SeatObs;
use gear_herdr::Profile;
use gear_model::{GearResult, MsgKind, MsgState, Ts};
use gear_store::{NewMessage, Tx};
use std::collections::BTreeMap;

/// The thread a seat's stall item lives in.
fn thread(seat: &str) -> String {
    format!("stall:{seat}")
}

/// The seats that wait for their provider (T-75): their screen shows a
/// provider back-off. The profile of the seat's kind says what a back-off
/// looks like on that screen; a kind without back-off markers never waits.
pub fn waiting_for_provider(
    profiles: &BTreeMap<String, Profile>,
    observed: &[SeatObs],
) -> Vec<String> {
    observed
        .iter()
        .filter(|obs| {
            profiles
                .get(&obs.seat.profile)
                .map(|profile| profile.backoff(obs.screen.as_deref().unwrap_or_default()))
                .unwrap_or(false)
        })
        .map(|obs| obs.seat.id.clone())
        .collect()
}

/// Close the captain's item for a seat that is not stuck any more (design
/// §10.3: a late check-in confirms the delivery and closes the stall item).
///
/// Returns true when an open item was closed.
///
/// `before` bounds the item: one that is **newer** than it belongs to a later
/// failure and is not closed here (an old delivery op must not close the
/// stall of a failure that happened after it).
pub fn resolve(
    tx: &Tx<'_>,
    project: &str,
    seat_id: &str,
    now: Ts,
    before: Option<Ts>,
) -> GearResult<bool> {
    let open = tx
        .thread_messages(project, &thread(seat_id))?
        .into_iter()
        .find(|m| matches!(m.state, MsgState::Open | MsgState::Waiting));
    let Some(item) = open else {
        return Ok(false);
    };
    if let Some(before) = before {
        if item.created > before {
            return Ok(false);
        }
    }
    tx.update_message_state(
        project,
        &item.id,
        MsgState::Resolved,
        Some("the lane checked in: the delivery was confirmed after all"),
        None,
        None,
        now,
    )?;
    Ok(true)
}

/// Open the captain's item for a flagged seat unless one is open already.
/// Returns true when an item was opened.
///
/// A seat that waits for its provider gets no item (T-75): the engine
/// recorded the wait, and the captain is not rung for a seat that is not
/// stuck.
pub fn alert(tx: &Tx<'_>, seat_id: &str, now: Ts) -> GearResult<bool> {
    let Some(seat) = tx.get_seat(seat_id)? else {
        return Ok(false);
    };
    if tx.seats_in_backoff(&seat.project)?.iter().any(|waiting| waiting == seat_id) {
        return Ok(false); // waiting for the provider, not stuck
    }
    let Some(captain) = tx.captain_of(&seat.project)? else {
        return Ok(false); // no captain: nobody to tell
    };
    if captain.id == seat.id {
        return Ok(false); // the captain itself: its doorbell is the signal
    }
    let open = tx
        .thread_messages(&seat.project, &thread(&seat.id))?
        .into_iter()
        .any(|m| matches!(m.state, MsgState::Open | MsgState::Waiting));
    if open {
        return Ok(false);
    }
    tx.insert_message(&NewMessage {
        meta: None,
        project: seat.project.clone(),
        thread: thread(&seat.id),
        task: None,
        attempt: None,
        sender: "engine".to_string(),
        sender_gen: 0,
        recipient: captain.id.clone(),
        kind: MsgKind::System,
        urgent: true,
        body: format!(
            "lane {} is stuck: its agent is gone from its pane, a delivery was not confirmed, or continuing it made no progress. Look at its pane (agent exited? context full? a dialog? quota?), get it going (restart or resume it, or give it a fresh session), then resolve this item.",
            seat.name
        ),
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created: now,
    })?;
    Ok(true)
}
