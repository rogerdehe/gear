//! Agent names (T-134): every seat's agent keeps its Herdr name.
//!
//! Herdr clears an agent's name when it re-detects the occupant — a Herdr
//! server restart, a CLI that changes its session — and `agent.prompt` refuses
//! a pane whose agent is unnamed ("agent <pane> is not an active named agent"):
//! after the M3.1 restart hull, deck and keel all lost their first deliveries to
//! it. Two halves:
//!
//! - [`restore`], once per engine tick: give every seat with an active binding
//!   its name back. A name another pane holds is reported to the captain once
//!   and never stolen.
//! - [`ensure`], before every prompt, doorbell or key send to a seat's pane:
//!   rename the agent first when it does not carry the seat's name, so delivery
//!   never fails on a cleared name.
//!
//! Names are `<project>-<seat>` (17 §2/§3), the same name `agent.start` gives.

use gear_herdr::HerdrAct;
use gear_model::{GearResult, MsgKind, MsgState, SeatRole, Ts};
use gear_store::{BindingStatus, NewMessage, SeatRow, Store, Tx};
use std::collections::BTreeMap;

/// The Herdr agent name of a seat (17 §2/§3): `<project>-<seat>`. One
/// convention, shared with the config's own length check.
pub use gear_app::config::agent_name;

/// The naming rules are Herdr's: [`gear_herdr::ensure_agent_name`] and
/// [`gear_herdr::ensure_agent_name_with`]. Re-exported so the engine's call
/// sites name one thing.
pub use gear_herdr::{
    ensure_agent_name as ensure, ensure_agent_name_with as ensure_snapshot, Ensured as Ensure,
};

/// Every agent-bearing pane and the name Herdr holds for it, in one call.
pub fn read(
    herdr: &mut dyn HerdrAct,
) -> Result<std::collections::BTreeMap<String, Option<String>>, gear_herdr::ObserveError> {
    herdr.agent_names()
}

/// What one [`restore`] did, for the tick report.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Restored {
    /// `seat` ids whose agent was renamed this tick.
    pub renamed: Vec<String>,
    /// `seat` ids whose name another pane holds (reported to the captain once).
    pub taken: Vec<String>,
    /// `seat: reason` of a rename Herdr refused or left unknown.
    pub failed: Vec<String>,
}

/// Give every seat of `project` with an active binding its agent name back
/// (T-134). The name in a taken case is not stolen: the captain gets one item
/// per seat in the `name:<seat>` thread, resolved when the name is back.
pub fn restore(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    names: &BTreeMap<String, Option<String>>,
    now: Ts,
) -> GearResult<Restored> {
    let mut restored = Restored::default();
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats {
        let Some(binding) = store.read(|tx| tx.get_binding(&seat.id, seat.generation))? else {
            continue;
        };
        if binding.status != BindingStatus::Active {
            continue;
        }
        let name = agent_name(&seat.project, &seat.name);
        match ensure_snapshot(herdr, names, &binding.pane_id, &name) {
            Ensure::Named => {
                if let Some(open) = open_taken(store, project, &seat.id)? {
                    store.write(|tx| resolve_taken(tx, project, &open, now))?;
                }
            }
            Ensure::Renamed => {
                restored.renamed.push(seat.id.clone());
                if let Some(open) = open_taken(store, project, &seat.id)? {
                    store.write(|tx| resolve_taken(tx, project, &open, now))?;
                }
            }
            Ensure::Taken { holder } => {
                restored.taken.push(seat.id.clone());
                store.write(|tx| report_taken(tx, project, &seat, &name, holder.as_deref(), now))?;
            }
            Ensure::NoAgent => {}
            Ensure::Failed(reason) => {
                restored.failed.push(format!("{}: {reason}", seat.id));
            }
        }
    }
    Ok(restored)
}

/// The thread a seat's taken-name item lives in.
fn thread(seat: &str) -> String {
    format!("name:{seat}")
}

/// One item for the captain that the seat's name is held by another pane — and
/// only one while that item is open.
fn report_taken(
    tx: &Tx<'_>,
    project: &str,
    seat: &SeatRow,
    name: &str,
    holder: Option<&str>,
    now: Ts,
) -> GearResult<()> {
    // The captain's own name cannot be reported to the captain: its doorbell
    // is the only signal there is, and the next tick looks again.
    if seat.role == SeatRole::Captain {
        return Ok(());
    }
    let Some(captain) = tx.captain_of(project)? else {
        return Ok(()); // no captain: nobody to tell
    };
    let open = tx
        .thread_messages(project, &thread(&seat.id))?
        .into_iter()
        .any(|message| matches!(message.state, MsgState::Open | MsgState::Waiting));
    if open {
        return Ok(()); // already said: one item per spell
    }
    let by = holder.map(|pane| format!(" (pane {pane})")).unwrap_or_default();
    tx.insert_message(&NewMessage {
        project: project.to_string(),
        thread: thread(&seat.id),
        task: None,
        attempt: None,
        sender: "engine".to_string(),
        sender_gen: 0,
        recipient: captain.id.clone(),
        kind: MsgKind::System,
        urgent: false,
        body: format!(
            "the agent name {name} of seat {} is held by another pane{by}, so GEAR cannot name its agent: Herdr refuses a prompt to an unnamed agent, and GEAR never steals a name. Rename or close that other agent (`herdr agent rename <pane> --clear`), then resolve this item; the engine names {} again on its next tick.",
            seat.name, seat.name
        ),
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created: now,
    })?;
    Ok(())
}

/// The seat's open taken-name item, if it has one: the only case in which a
/// rename has an item to resolve.
fn open_taken(store: &mut Store, project: &str, seat: &str) -> GearResult<Option<String>> {
    store.read(|tx| {
        Ok(tx
            .thread_messages(project, &thread(seat))?
            .into_iter()
            .find(|message| matches!(message.state, MsgState::Open | MsgState::Waiting))
            .map(|message| message.id))
    })
}

/// Resolve the seat's taken-name item once its own agent carries the name.
fn resolve_taken(tx: &Tx<'_>, project: &str, id: &str, now: Ts) -> GearResult<()> {
    tx.update_message_state(
        project,
        id,
        MsgState::Resolved,
        Some("the agent carries its name again"),
        None,
        None,
        now,
    )
}
