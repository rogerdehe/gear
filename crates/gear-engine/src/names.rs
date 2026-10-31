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
//! Names are `<project>-<seat>` (17 §2/§3), the same name `agent.start` gives
//! — except a temp reviewer, whose agent is named after the task it reviews
//! (17 §2: `<project>-review-<task>`), not after its warm seat.

use gear_herdr::{HerdrAct, Outcome};
use gear_model::{GearResult, MsgKind, MsgState, SeatRole, Ts};
use gear_store::{BindingStatus, NewMessage, SeatRow, Store, Tx};
use std::collections::BTreeMap;

/// The Herdr agent name of a seat (17 §2/§3): `<project>-<seat>`. One
/// convention, shared with the config's own length check.
pub use gear_app::config::agent_name;

/// The agent name of a task's reviewer (17 §2): `<project>-review-<task>`,
/// the task id lowercased without its dash (`gear-review-t134`). This is the
/// name `review::start_reviewer` starts the agent with, and the name a review
/// seat's agent must keep (T-134 review F1).
pub fn review_agent_name(project: &str, task: &str) -> String {
    format!("{project}-review-{}", task.to_lowercase().replace('-', ""))
}

/// The name a seat's agent must carry (T-134 review F1) — derived from the
/// store's seat, never from what Herdr happens to report now (a cleared or
/// wrong name is exactly what is being repaired).
///
/// - an assigned Temp reviewer: [`review_agent_name`] of the task it reviews,
///   the name its review started with (the seat's warm name `review-10` is the
///   *seat's* own name, not its agent's);
/// - the captain and every lane: [`agent_name`], `<project>-<seat name>`.
pub fn desired(seat: &SeatRow) -> String {
    match (seat.role, seat.task_id.as_deref()) {
        (SeatRole::Temp, Some(task)) => review_agent_name(&seat.project, task),
        _ => agent_name(&seat.project, &seat.name),
    }
}

/// The naming rules are Herdr's: [`gear_herdr::ensure_agent_name`] and
/// [`gear_herdr::ensure_agent_name_with`]. Re-exported so the engine's call
/// sites name one thing.
pub use gear_herdr::{
    ensure_agent_name as ensure, ensure_agent_name_with as ensure_snapshot, Ensured as Ensure,
};

/// What a text aimed at a seat's pane may do (design §10.1: never type into
/// the wrong thing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The pane's agent carries `name`: the text goes to that name. Herdr
    /// refuses a prompt when no agent of that name runs, so a pane that lost
    /// its agent never receives the text.
    Prompt { name: String },
    /// The pane's agent had lost its name (M3.1): it was renamed now and the
    /// text goes **next tick**. Nothing is typed and nothing is restarted.
    Renamed,
    /// No agent of the seat's kind is in the pane: the seat's agent is gone.
    /// The text is not sent at all.
    Gone(String),
    /// The name is held by another pane: nothing may be typed into it.
    Blocked(String),
    /// The pane could not be read: nothing is typed; the next tick looks
    /// again (an observation failure is never "the agent is gone").
    Unreadable(String),
}

/// The live name of the agent in `pane_id` (design §10.1: a prompt targets
/// the agent's name, never the pane id).
///
/// `None` when the pane hosts no agent, when the agent carries no name, or
/// when the names could not be read: the caller must not type then — a pane
/// whose agent is gone is a shell, and the text would be typed into it.
pub fn live_name(herdr: &mut dyn HerdrAct, pane_id: &str) -> Option<String> {
    let names = herdr.agent_names().ok()?;
    names.get(pane_id).and_then(|name| name.clone())
}

/// Aim a text at the agent of `pane_id` (design §10.1).
///
/// The agent of the pane is read first (`agent_get`): only an agent of the
/// seat's kind is the seat's agent. Then the name it carries decides:
/// - it carries a name → [`Target::Prompt`]: the text goes to **that** name,
///   and Herdr refuses it if no agent of that name runs. The name belongs to
///   the agent of this pane, so the text cannot reach another pane;
/// - it lost its name (M3.1) → it is renamed to the seat's name now and the
///   text goes next tick ([`Target::Renamed`]);
/// - the name cannot be given → nothing is typed ([`Target::Blocked`]);
/// - there is no agent of the seat's kind → the seat's agent is gone
///   ([`Target::Gone`]).
///
/// `names` is a snapshot of [`HerdrRead::agent_names`] read by the caller, so
/// one read serves every op of a tick.
pub fn resolve_target(
    herdr: &mut dyn HerdrAct,
    names: &BTreeMap<String, Option<String>>,
    pane_id: &str,
    name: &str,
    kind: &str,
) -> Target {
    // The pane's occupant: an agent of another kind is not the seat's agent.
    let occupant = match herdr.agent_get(pane_id) {
        Ok(occupant) => occupant,
        Err(error) => return Target::Unreadable(error.0),
    };
    let Some(info) = occupant else {
        return Target::Gone(format!("no agent in the pane {pane_id}"));
    };
    if info.agent.as_deref() != Some(kind) {
        return Target::Gone(format!(
            "the pane {pane_id} hosts a {} agent, not the seat's {kind}",
            info.agent.as_deref().unwrap_or("unknown")
        ));
    }
    // The name the agent carries now (`None`: Herdr cleared it).
    match names.get(pane_id) {
        // The live name of this pane's agent: the text goes to it, never to
        // the pane id (design §10.1).
        Some(Some(live)) => Target::Prompt { name: live.clone() },
        // It lost its name: give it back now; the text goes next tick.
        Some(None) => match herdr.agent_rename(pane_id, name) {
            Outcome::Ok(()) => Target::Renamed,
            Outcome::Refused { code, message } => Target::Blocked(format!(
                "the name {name} could not be given to the agent of {pane_id}: {code}: {message}"
            )),
            Outcome::Uncertain { reason } => Target::Unreadable(reason),
        },
        // The pane is not in Herdr's list: the read is partial, so nothing is
        // typed and the next tick looks again.
        None => Target::Unreadable(format!("the pane {pane_id} is not in Herdr's agent list")),
    }
}

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
    // T-270: two passes, the seats that **give a name back** before the seats
    // that ask for one. A finished reviewer goes Standby but its agent keeps
    // the task name until this very tick's restore renames it to its seat name
    // (`review-9` → `<project>-review-9`); another seat asking for that task
    // name in the same pass would see it held and the captain got an item the
    // next tick resolved by itself. Within a pass the order stays as it was (the
    // store's own `ORDER BY id`).
    let (own, task_named): (Vec<SeatRow>, Vec<SeatRow>) = seats
        .into_iter()
        .partition(|seat| desired(seat) == agent_name(&seat.project, &seat.name));
    // The names Herdr reported, carried forward through the pass: a rename below
    // frees the name a later seat asks for.
    let mut names = names.clone();
    for seat in own.into_iter().chain(task_named) {
        let Some(binding) = store.read(|tx| tx.get_binding(&seat.id, seat.generation))? else {
            continue;
        };
        // A **Pending** binding is a seat that expects its agent: the engine
        // asked Herdr to start it and has not confirmed it yet (the review of
        // T-18: an agent Herdr accepted while the seat stayed `Starting`). Its
        // agent carries the seat's name like every other one.
        if !matches!(
            binding.status,
            BindingStatus::Active | BindingStatus::Pending
        ) {
            continue;
        }
        let name = desired(&seat);
        match ensure_snapshot(herdr, &names, &binding.pane_id, &name) {
            Ensure::Named => {
                if let Some(open) = open_taken(store, project, &seat.id)? {
                    store.write(|tx| resolve_taken(tx, project, &open, now))?;
                }
            }
            Ensure::Renamed => {
                restored.renamed.push(seat.id.clone());
                // The pane carries its new name from now on: the name it freed
                // is free for the rest of this pass.
                names.insert(binding.pane_id.clone(), Some(name.clone()));
                if let Some(open) = open_taken(store, project, &seat.id)? {
                    store.write(|tx| resolve_taken(tx, project, &open, now))?;
                }
            }
            Ensure::Taken { holder } => {
                restored.taken.push(seat.id.clone());
                store
                    .write(|tx| report_taken(tx, project, &seat, &name, holder.as_deref(), now))?;
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
    // T-285: the captain seat gets its item like any seat. Its agent is the
    // one that types for the captain, so an unnamed captain pane is exactly
    // what an item has to unblock — leaving it without one left it unnamed for
    // ever.
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
    let by = holder
        .map(|pane| format!(" (pane {pane})"))
        .unwrap_or_default();
    tx.insert_message(&NewMessage {
        meta: None,
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
