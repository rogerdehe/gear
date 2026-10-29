//! The friction ring (design §6.4): the captain hears about the open friction
//! reports even while it is busy.
//!
//! A lane or a reviewer that was slowed down writes a friction report at its
//! hand-in (`--friction`, design §6.1). The captain reads them with `gear
//! friction` and closes them one by one (§6.4). Nobody tells the captain that
//! a report arrived: the engine opens one item on the thread `friction`,
//! naming the open reports.
//!
//! The rules (design §6.4, T-416):
//! - the ring does not wait for an idle captain: a captain on a long voyage
//!   always has an actionable item, and the reports kept waiting;
//! - one item per project, at most once per 30 minutes since the last ring;
//! - a new report while the item is open updates its list;
//! - the item closes itself when every report it lists is closed;
//! - an item resolved by the captain while any report it named is still open
//!   counts as rung, not as handled: the next ring after the cooldown names
//!   the reports still open again.

use gear_model::{GearResult, MsgKind, MsgState, Ts};
use gear_store::{FrictionRow, NewMessage, Store, Tx};

/// The thread the friction item lives in (one item per project).
const THREAD: &str = "friction";

/// The smallest gap between two rings (design §6.4: never more than once per
/// 30 min).
const COOLDOWN_S: i64 = 30 * 60;

/// How many reports one item names before it says how many are left out.
const MAX_LISTED: usize = 10;

/// How much of a report's body the item quotes.
const SNIPPET_CHARS: usize = 160;

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
    // Newest first (design §6.4, T-416): the store lists them oldest first,
    // by creation and then by the (lexicographic) id.
    let mut open = tx.open_frictions(project)?;
    open.sort_by(|a, b| {
        (b.created, id_no(&b.id)).cmp(&(a.created, id_no(&a.id)))
    });

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
            write_state(tx, project, &State { at: state.at, item: None })?;
            return Ok(Some(Change::Closed));
        }
        // A report arrived while the item was open: its list changes.
        let body = body(&open);
        let changed = item.body != body;
        if changed {
            tx.update_message_body(project, &item.id, &body)?;
        }
        return Ok(if changed { Some(Change::Updated) } else { None });
    }

    // No item is open. The ring needs reports to name (design §6.4); it does
    // **not** wait for an idle captain: a captain on a long voyage always has
    // an actionable item, and the reports were never rung (T-416).
    if open.is_empty() {
        return Ok(None);
    }
    // At most once per 30 min since the last ring (design §6.4). An item the
    // captain resolved while its reports were still open counts as rung, not
    // as handled: the next ring names the reports still open again (T-416).
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
        &State { at: Some(now), item: Some(message.id) },
    )?;
    Ok(Some(Change::Opened))
}

/// The item's text (design §6.4): how many reports wait and which ones, each
/// as `F-<n> <task> <role>: <the start of its body>`, newest first, at most
/// [`MAX_LISTED`] of them (T-416).
fn body(open: &[FrictionRow]) -> String {
    let count = open.len();
    let word = if count == 1 { "report" } else { "reports" };
    let mut text = format!("{count} friction {word}:");
    for report in open.iter().take(MAX_LISTED) {
        text.push('\n');
        text.push_str(&report.id);
        text.push(' ');
        text.push_str(&report.task);
        text.push(' ');
        text.push_str(&report.role);
        text.push_str(": ");
        text.push_str(&snippet(&report.body));
    }
    text.push('\n');
    if count > MAX_LISTED {
        text.push_str(&format!("and {} more: run gear friction", count - MAX_LISTED));
    } else {
        text.push_str("run gear friction");
    }
    text
}

/// The number of a report's id (`F-<n>`): the newest-first order breaks ties
/// within a second by it (T-416).
fn id_no(id: &str) -> i64 {
    id.trim_start_matches("F-").parse().unwrap_or(0)
}

/// The first [`SNIPPET_CHARS`] characters of a report's body (T-416).
fn snippet(body: &str) -> String {
    body.chars().take(SNIPPET_CHARS).collect()
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
    })
}

/// Keep the ring's state.
fn write_state(tx: &Tx<'_>, project: &str, state: &State) -> GearResult<()> {
    let value = serde_json::json!({
        "at": state.at,
        "item": state.item,
    });
    tx.set_meta(&state_key(project), &value.to_string())
}
