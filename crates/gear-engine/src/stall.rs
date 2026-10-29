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
use gear_store::{NewMessage, SeatRow, Tx};
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
    // The latest incident still owns the marker after the captain resolves
    // its message. A bounded old check-in may clear only an incident that
    // already existed when that view was recorded.
    let latest = tx.thread_messages(project, &thread(seat_id))?.into_iter()
        .filter(|m| m.sender == "engine" && m.kind == MsgKind::System)
        .max_by_key(|m| (m.created, m.id.strip_prefix("M-").and_then(|n| n.parse::<u64>().ok()).unwrap_or(0)));
    let Some(item) = latest else {
        if before.is_none() { tx.set_meta(&format!("stall-active:{seat_id}"), "")?; }
        return Ok(false);
    };
    if before.is_some_and(|limit| item.created > limit) { return Ok(false); }
    tx.set_meta(&format!("stall-active:{seat_id}"), "")?;
    if !matches!(item.state, MsgState::Open | MsgState::Waiting) { return Ok(false); }
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
    alert_fact(tx, seat_id, now, "its agent is gone from its pane, a delivery was not confirmed, or continuing it made no progress")
}

/// What the stuck lane is about (T-293): the work in flight, its last
/// check-in, and the delivery op it waits on. The captain should not have to
/// open the pane to know what the alert is about.
fn about(tx: &Tx<'_>, seat: &SeatRow, now: Ts) -> GearResult<String> {
    let mut parts = Vec::new();
    match tx.in_progress_attempts_of_seat(&seat.id)?.into_iter().next() {
        Some(attempt) => parts.push(format!("{} attempt {} ({})", attempt.task, attempt.n, attempt.state)),
        None => parts.push("no task".to_string()),
    }
    let minutes = |ts: Ts| now.saturating_sub(ts) / 60;
    match tx.views_since(&seat.id, seat.generation, 0)?.into_iter().max_by_key(|view| view.ts) {
        Some(view) => parts.push(format!("last check-in {} min ago", minutes(view.ts))),
        None => parts.push("no check-in this generation".to_string()),
    }
    if let Some(op) = tx.slot_ops(&seat.id)?.into_iter().min_by_key(|op| (op.created, op.id.clone())) {
        parts.push(format!("waits on {} ({}, {}, {} min)", op.id, op.kind, op.state, minutes(op.created)));
    }
    Ok(parts.join("; "))
}

pub fn alert_fact(tx: &Tx<'_>, seat_id: &str, now: Ts, fact: &str) -> GearResult<bool> {
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
    let body = format!("lane {} is stuck: {fact}. About: {}. Look at its pane (agent exited? context full? a dialog? quota?), get it going (restart or resume it, or give it a fresh session), then resolve this item.", seat.name, about(tx, &seat, now)?);
    let open = tx.thread_messages(&seat.project, &thread(&seat.id))?.into_iter()
        .find(|m| matches!(m.state, MsgState::Open | MsgState::Waiting));
    if let Some(item) = open {
        if item.body != body { tx.update_message_body(&seat.project, &item.id, &body)?; }
        return Ok(false);
    }
    let key = format!("stall-active:{seat_id}");
    if tx.get_meta(&key)?.is_some_and(|v| !v.is_empty())
        && !crate::attention::due(tx, &seat.project, &thread(&seat.id), &captain.id, now)? {
        return Ok(false);
    }
    tx.set_meta(&key, "1")?;
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
        body,
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created: now,
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_model::SeatRole;
    use gear_store::{Store, testing::{seed_active_seat, seed_project}};

    #[test]
    fn persistent_stall_is_one_item_realerts_and_closes_on_recovery() {
        let mut store = Store::open_in_memory().unwrap();
        store.write(|tx| {
            seed_project(tx, "p", "sess", 1000)?;
            seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p9", "claude", 1000)?;
            seed_active_seat(tx, "p", "lane", SeatRole::Lane, "w1:p1", "opencode", 1000)?;
            Ok(())
        }).unwrap();
        for now in 1001..1006 {
            store.write(|tx| alert(tx, "p:lane", now)).unwrap();
        }
        let messages = store.read(|tx| tx.thread_messages("p", "stall:p:lane")).unwrap();
        assert_eq!(messages.len(), 1);
        let id = messages[0].id.clone();
        store.write(|tx| alert_fact(tx, "p:lane", 1006, "a delivery was not confirmed")).unwrap();
        let updated = store.read(|tx| tx.thread_messages("p", "stall:p:lane")).unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].id, id);
        assert!(updated[0].body.contains("a delivery was not confirmed"));
        store.write(|tx| tx.update_message_state("p", &id, MsgState::Resolved, Some("seen"), None, None, 1010)).unwrap();
        assert!(!store.write(|tx| alert(tx, "p:lane", 1909)).unwrap());
        assert!(store.write(|tx| alert(tx, "p:lane", 1910)).unwrap());
        assert!(store.write(|tx| resolve(tx, "p", "p:lane", 1911, None)).unwrap());
        let messages = store.read(|tx| tx.thread_messages("p", "stall:p:lane")).unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|m| m.state == MsgState::Resolved));
    }
    #[test]
    fn old_checkin_does_not_clear_a_newer_resolved_incident() {
        let mut store = Store::open_in_memory().unwrap();
        store.write(|tx| {
            seed_project(tx, "p", "sess", 1000)?;
            seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p9", "claude", 1000)?;
            seed_active_seat(tx, "p", "lane", SeatRole::Lane, "w1:p1", "opencode", 1000)?;
            Ok(())
        }).unwrap();
        store.write(|tx| alert(tx, "p:lane", 1010)).unwrap();
        let first = store.read(|tx| tx.thread_messages("p", "stall:p:lane")).unwrap().remove(0);
        store.write(|tx| tx.update_message_state("p", &first.id, MsgState::Resolved, Some("seen"), None, None, 1020)).unwrap();
        assert!(store.write(|tx| alert(tx, "p:lane", 1920)).unwrap());
        let second = store.read(|tx| tx.thread_messages("p", "stall:p:lane")).unwrap().pop().unwrap();
        store.write(|tx| tx.update_message_state("p", &second.id, MsgState::Resolved, Some("seen"), None, None, 1921)).unwrap();
        assert!(!store.write(|tx| resolve(tx, "p", "p:lane", 1922, Some(1015))).unwrap());
        assert_eq!(store.read(|tx| tx.get_meta("stall-active:p:lane")).unwrap().as_deref(), Some("1"));
        assert!(!store.write(|tx| alert(tx, "p:lane", 1923)).unwrap());
        assert!(store.write(|tx| alert(tx, "p:lane", 2821)).unwrap());
    }

}
