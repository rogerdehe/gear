//! Dialogs never block a seat (M6 S2, T-468).
//!
//! A lane or a reviewer may not stop on a dialog at all: a CLI that asks a
//! question (a permission prompt, a startup screen) would wait for an answer
//! nobody gives, and the seat would sit blocked for minutes (lesson 7b, and
//! on 2026-10-29 the Codex 0.165 startup dialogs). The engine closes such a
//! screen in the same tick and tells the seat what to do instead: ask the
//! captain with `gear send --kind question`.
//!
//! The captain's dialog is the one GEAR allows: it confirms a change the user
//! asked for. Nobody may answer it (the user is away), so the engine cancels
//! it after [`CAPTAIN_CANCEL_S`] and the captain goes on as it would have
//! without it.

use crate::observe::{Presence, SeatObs};
use gear_herdr::{AgentStatus, HerdrAct, Outcome, Profile};
use gear_model::{GearResult, SeatRole, Ts};
use gear_store::{BindingStatus, Store};
use std::collections::BTreeMap;

/// The text a lane or reviewer gets when its dialog is closed (T-468).
pub const LANE_TEXT: &str =
    "Dialogs are not allowed for your role; ask the captain with gear send --kind question.";

/// How long a captain's dialog may wait for an answer before it is cancelled
/// (T-468): ten minutes, the voyage's scope decision of 2026-10-30.
pub const CAPTAIN_CANCEL_S: i64 = 600;

/// How long before a dialog that is still open is closed again (T-468): an
/// escape that was refused or lost must not leave the seat blocked for good.
const RETRY_S: i64 = 30;

fn closed_key(seat: &str) -> String {
    format!("engine.dialog.{seat}")
}

fn told_key(seat: &str) -> String {
    format!("engine.dialog.{seat}.told")
}

/// The keys that close the dialog a seat shows, if one is open.
///
/// A dialog the seat's profile knows (a CLI startup screen, T-457) is answered
/// with its own keys, so the screen it was asking about is the one it takes;
/// any other blocking screen — a permission prompt, a question — is cancelled
/// with `esc`. `None` = no dialog open (or the screen was not read).
fn dialog_keys(obs: &SeatObs, profiles: &BTreeMap<String, Profile>) -> Option<Vec<String>> {
    if obs.presence != Presence::Present {
        return None;
    }
    if let Some(profile) = profiles.get(&obs.seat.profile) {
        if let Some(screen) = obs.screen.as_deref() {
            if let Some(dialog) = profile.dialog(screen) {
                return Some(dialog.keys.clone());
            }
        }
    }
    if obs.status == Some(AgentStatus::Blocked) {
        return Some(vec!["esc".into()]);
    }
    None
}

/// The pane of the seat's active binding: where its agent runs.
fn pane_of(store: &mut Store, obs: &SeatObs) -> GearResult<Option<String>> {
    Ok(store.read(|tx| {
        Ok(tx
            .get_binding(&obs.seat.id, obs.seat.generation)?
            .filter(|binding| binding.status == BindingStatus::Active)
            .map(|binding| binding.pane_id))
    })?)
}

/// Close the dialog of every lane and reviewer seat in `observed`, and cancel
/// a captain dialog that nobody answered for [`CAPTAIN_CANCEL_S`]. Returns the
/// seats whose dialog was closed this tick.
///
/// A lane's or reviewer's dialog is closed at once and the seat is told
/// [`LANE_TEXT`] once per spell; a dialog that is still open [`RETRY_S`] later
/// is closed again, but the text is not repeated. A captain's dialog gets its
/// ten minutes; when they have passed it is cancelled and the timer starts
/// over, so a dialog that shows again waits its own ten minutes.
///
/// The timer lives in the store's meta, so it survives an engine restart.
pub fn check(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    observed: &[SeatObs],
    profiles: &BTreeMap<String, Profile>,
    now: Ts,
) -> GearResult<Vec<String>> {
    let mut closed = Vec::new();
    for obs in observed {
        let seat = &obs.seat;
        let Some(keys) = dialog_keys(obs, profiles) else {
            // No dialog: the spell is over, the next one starts fresh.
            store.write(|tx| {
                tx.set_meta(&closed_key(&seat.id), "")?;
                tx.set_meta(&told_key(&seat.id), "")?;
                Ok(())
            })?;
            continue;
        };
        let Some(pane) = pane_of(store, obs)? else {
            continue;
        };
        match seat.role {
            SeatRole::Lane | SeatRole::Temp => {
                let since = store
                    .read(|tx| tx.get_meta(&closed_key(&seat.id)))?
                    .and_then(|value| value.parse::<Ts>().ok());
                if since.is_some_and(|since| now - since < RETRY_S) {
                    continue; // closed a moment ago: let the agent take it
                }
                let took = herdr.send_keys(&pane, &keys);
                store.write(|tx| tx.set_meta(&closed_key(&seat.id), &now.to_string()))?;
                if !matches!(took, Outcome::Ok(())) {
                    continue; // try again after RETRY_S
                }
                let told = store
                    .read(|tx| tx.get_meta(&told_key(&seat.id)))?
                    .is_some_and(|value| !value.is_empty());
                if !told && tell(herdr, &pane, LANE_TEXT) {
                    store.write(|tx| tx.set_meta(&told_key(&seat.id), "1"))?;
                }
                closed.push(seat.id.clone());
            }
            SeatRole::Captain => {
                let since = store
                    .read(|tx| tx.get_meta(&closed_key(&seat.id)))?
                    .and_then(|value| value.parse::<Ts>().ok());
                let Some(since) = since else {
                    store.write(|tx| tx.set_meta(&closed_key(&seat.id), &now.to_string()))?;
                    continue; // the ten minutes start now
                };
                if now - since < CAPTAIN_CANCEL_S {
                    continue;
                }
                if matches!(herdr.send_keys(&pane, &keys), Outcome::Ok(())) {
                    store.write(|tx| tx.set_meta(&closed_key(&seat.id), ""))?;
                    closed.push(seat.id.clone());
                }
            }
        }
    }
    Ok(closed)
}

/// Type `text` into the pane and submit it. A blocked agent refuses an
/// `agent.prompt`, so the text goes in as keystrokes after the dialog was
/// closed; the terminal takes it and Enter sends it (T-468).
fn tell(herdr: &mut dyn HerdrAct, pane: &str, text: &str) -> bool {
    if !matches!(herdr.pane_type(pane, text), Outcome::Ok(())) {
        return false;
    }
    // Let the TUI take the typed text before the Enter arrives (L-71). The
    // wait belongs to the terminal: a fake has no TUI and does not sleep.
    herdr.type_settle();
    !matches!(
        herdr.send_keys(pane, &["enter".into()]),
        Outcome::Refused { .. }
    )
}
