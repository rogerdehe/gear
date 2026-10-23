//! A lane the engine flagged reaches the captain (lesson L-52).
//!
//! The engine flags a seat when a delivery to it is never confirmed or when
//! continuing it made no progress. Until 2026-10-23 a flag went only to
//! engine.log: the lane stood still and nobody knew. Now every flagged seat
//! gets one action item in its captain's inbox (so the doorbell rings), kept
//! until the captain resolves it; a second flag of the same seat while the
//! item is open adds nothing.

use gear_model::{GearResult, MsgKind, MsgState, Ts};
use gear_store::{NewMessage, Tx};

/// The thread a seat's stall item lives in.
fn thread(seat: &str) -> String {
    format!("stall:{seat}")
}

/// Open the captain's item for a flagged seat unless one is open already.
/// Returns true when an item was opened.
pub fn alert(tx: &Tx<'_>, seat_id: &str, now: Ts) -> GearResult<bool> {
    let Some(seat) = tx.get_seat(seat_id)? else {
        return Ok(false);
    };
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
