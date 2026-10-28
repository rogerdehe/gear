//! The friction ring (design §6.4): the captain hears about the open friction
//! reports when it has nothing else to do.
//!
//! A lane or a reviewer that was slowed down writes a friction report at its
//! hand-in (`--friction`, design §6.1). The captain reads them with `gear
//! friction` and closes them one by one (§6.4). Nobody tells the captain that
//! a report arrived: it finds out when it has no actionable item — one item on
//! the thread `friction`, naming the open reports.
//!
//! The rules (design §6.4):
//! - the captain is idle when it has no actionable item (`actionable_for` is
//!   empty); only then does the ring open;
//! - one item per project, at most once per 30 minutes;
//! - a new report while the item is open updates its list;
//! - the item closes itself when every report it lists is closed.

use gear_model::{GearResult, MsgKind, MsgState, Ts};
use gear_store::{NewMessage, Store, Tx};

/// The thread the friction item lives in (one item per project).
const THREAD: &str = "friction";

/// The smallest gap between two rings (design §6.4: never more than once per
/// 30 min).
const COOLDOWN_S: i64 = 30 * 60;

/// What the ring did in one tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The item was opened: the captain had nothing to do and reports waited.
    Opened,
    /// The item's list changed: a report arrived while it was open.
    Updated,
    /// The item closed itself: every report it listed is closed.
    Closed,
}

impl Change {
    pub fn as_str(&self) -> &'static str {
        match self {
            Change::Opened => "opened",
            Change::Updated => "updated",
            Change::Closed => "closed",
        }
    }
}

/// The state of the ring, kept in the project's meta so a restart does not
/// ring twice (and does not lose the item it already opened).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct State {
    /// When the item was opened (the cooldown's clock).
    at: Option<Ts>,
    /// The id of the item the engine opened.
    item: Option<String>,
    /// The reports an item has already named (design §6.4: the ring rings
    /// again when a **new** report arrives — a report the captain has not
    /// been told about yet). It outlives the item, so a captain who resolved
    /// the item does not get the same reports again.
    listed: Vec<String>,
}

fn state_key(project: &str) -> String {
    format!("friction-ring:{project}")
}

/// Ring the captain about the open friction reports of a project.
pub fn ring(store: &mut Store, project: &str, now: Ts) -> GearResult<Option<Change>> {
    store.write(|tx| ring_in(tx, project, now))
}

/// The work, over an open transaction.
fn ring_in(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Option<Change>> {
    let Some(captain) = tx.captain_of(project)? else {
        return Ok(None); // no captain: nobody to tell
    };
    let state = read_state(tx, project)?;
    let open: Vec<String> = tx
        .open_frictions(project)?
        .into_iter()
        .map(|report| report.id)
        .collect();

    // The item the engine opened, while it is still open.
    let item = match state.item.as_deref() {
        Some(id) => tx.get_message(project, id)?.filter(|message| {
            matches!(message.state, MsgState::Open | MsgState::Waiting)
                && message.thread == THREAD
        }),
        None => None,
    };

    if let Some(item) = item {
        if open.is_empty() {
            // Every report it lists is closed: the item closes itself.
            tx.update_message_state(
                project,
                &item.id,
                MsgState::Resolved,
                Some("every friction report is closed"),
                None,
                None,
                now,
            )?;
            write_state(tx, project, &State { at: state.at, item: None, listed: state.listed })?;
            return Ok(Some(Change::Closed));
        }
        // A report arrived while the item was open: its list changes.
        let body = body(&open);
        let changed = item.body != body;
        if changed {
            tx.update_message_body(project, &item.id, &body)?;
        }
        // The reports the item names are the ones the captain has heard of.
        let listed = with_listed(&state.listed, &open);
        if listed != state.listed {
            write_state(tx, project, &State { at: state.at, item: state.item, listed })?;
        }
        return Ok(if changed { Some(Change::Updated) } else { None });
    }

    // No item is open. A ring needs reports to name and an idle captain: it
    // has no actionable item (design §6.4).
    if open.is_empty() {
        return Ok(None);
    }
    // A ring needs a report the captain has not been told about yet: it rings
    // again when a **new** report arrives, not for the same ones again
    // (design §6.4).
    if !open.iter().any(|id| !state.listed.contains(id)) {
        return Ok(None);
    }
    if !tx.actionable_for(project, &captain.id, now)?.is_empty() {
        return Ok(None); // the captain has work of its own
    }
    // At most once per 30 min (design §6.4).
    if let Some(at) = state.at {
        if now - at < COOLDOWN_S {
            return Ok(None);
        }
    }
    let body = body(&open);
    let message = tx.insert_message(&NewMessage {
        meta: None,
        project: project.to_string(),
        thread: THREAD.to_string(),
        task: None,
        attempt: None,
        sender: "engine".to_string(),
        sender_gen: 0,
        recipient: captain.id.clone(),
        kind: MsgKind::System,
        // An information item: it needs no ack and it does not ring like a
        // stuck lane — it waits in the inbox until its reports are closed.
        urgent: false,
        body,
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created: now,
    })?;
    write_state(
        tx,
        project,
        &State {
            at: Some(now),
            item: Some(message.id),
            listed: with_listed(&state.listed, &open),
        },
    )?;
    Ok(Some(Change::Opened))
}

/// The reports an item has named: the ones it already named, plus the ones it
/// names now.
fn with_listed(listed: &[String], open: &[String]) -> Vec<String> {
    let mut listed = listed.to_vec();
    for id in open {
        if !listed.contains(id) {
            listed.push(id.clone());
        }
    }
    listed
}

/// The item's text (design §6.4): how many reports wait and which ones.
fn body(open: &[String]) -> String {
    let count = open.len();
    let word = if count == 1 { "report" } else { "reports" };
    format!(
        "{count} friction {word}: {}. Run gear friction.",
        open.join(" ")
    )
}

/// The ring's state, as it is kept in the project's meta.
fn read_state(tx: &Tx<'_>, project: &str) -> GearResult<State> {
    let Some(raw) = tx.get_meta(&state_key(project))? else {
        return Ok(State::default());
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(State::default());
    };
    Ok(State {
        at: value.get("at").and_then(|at| at.as_i64()),
        item: value
            .get("item")
            .and_then(|item| item.as_str())
            .map(|item| item.to_string()),
        listed: value
            .get("listed")
            .and_then(|listed| listed.as_array())
            .map(|listed| {
                listed
                    .iter()
                    .filter_map(|id| id.as_str().map(|id| id.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Keep the ring's state.
fn write_state(tx: &Tx<'_>, project: &str, state: &State) -> GearResult<()> {
    let value = serde_json::json!({
        "at": state.at,
        "item": state.item,
        "listed": state.listed,
    });
    tx.set_meta(&state_key(project), &value.to_string())
}
