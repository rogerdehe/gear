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
//! A reviewer's agent carries its seat's name too (`<project>-review-<k>`,
//! with the task in its title), never the task's: a warm seat keeps its name
//! across the tasks it reviews (T-482).

use gear_herdr::{HerdrAct, Outcome};
use gear_model::{GearResult, MsgKind, MsgState, Ts};
use gear_model::{SeatRole, TaskState};
use gear_store::{BindingStatus, NewMessage, SeatRow, Store, Tx};
use std::collections::BTreeMap;

/// The Herdr agent name of a seat (17 §2/§3): `<project>-<seat>`. One
/// convention, shared with the config's own length check.
pub use gear_app::config::agent_name;

/// The name a seat's agent must carry (T-134 review F1, T-482) — derived from
/// the store's seat, never from what Herdr happens to report now (a cleared or
/// wrong name is exactly what is being repaired).
///
/// Every seat, the captain, a lane and a temp reviewer: [`agent_name`],
/// `<project>-<seat name>` — a reviewer's seat is long-lived and its agent
/// keeps the seat's name across the tasks it reviews.
pub fn desired(seat: &SeatRow) -> String {
    agent_name(&seat.project, &seat.name)
}

/// The naming rules are Herdr's: [`gear_herdr::ensure_agent_name`] and
/// [`gear_herdr::ensure_agent_name_keeping`]. Re-exported so the engine's call
/// sites name one thing.
pub use gear_herdr::{
    ensure_agent_name as ensure, ensure_agent_name_keeping as ensure_keeping, Ensured as Ensure,
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

/// The panes of `project` whose agent carries the name of a seat of `role`
/// (T-561), as Herdr shows them now.
///
/// The quota names **live agents**, not seats: a seat the store calls
/// `Released` whose agent is still in its pane is a reviewer the user sees, and
/// a start must wait for it. Counted from the same snapshot `restore` works
/// from, so one read serves the tick. An unreadable list is no evidence
/// either way: the caller gets no panes and its own seat-state count stands.
/// `except` leaves one seat out: a seat that is resumed or reused does not add
/// a pane, so its own agent must not stop its own start.
pub fn live_of_role(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    role: SeatRole,
    except: Option<&str>,
) -> Vec<String> {
    let Ok(names) = read(herdr) else {
        return Vec::new();
    };
    let Ok(seats) = store.read(|tx| tx.seats_of_project(project)) else {
        return Vec::new();
    };
    let wanted: Vec<String> = seats
        .iter()
        .filter(|seat| seat.role == role)
        .filter(|seat| except.is_none_or(|except| except != seat.id))
        .map(desired)
        .collect();
    names
        .into_iter()
        .filter(|(_, name)| {
            name.as_deref()
                .is_some_and(|name| wanted.iter().any(|wanted| wanted == name))
        })
        .map(|(pane, _)| pane)
        .collect()
}

/// Every seat whose binding is `pane`, with when that binding was made, across
/// every project of the store (T-570): one pane can hold two seats — w2:p10
/// held prometheus:captain and lawrence:captain — and the one agent in it
/// carries one name, the name of any of them.
fn seats_on_pane(tx: &Tx<'_>, pane: &str) -> GearResult<Vec<(SeatRow, Ts)>> {
    let mut seats = Vec::new();
    for project in tx.project_ids()? {
        for seat in tx.seats_of_project(&project)? {
            let Some(binding) = tx.get_binding(&seat.id, seat.generation)? else {
                continue;
            };
            if !matches!(
                binding.status,
                BindingStatus::Active | BindingStatus::Pending
            ) {
                continue;
            }
            if binding.pane_id == pane {
                seats.push((seat, binding.created));
            }
        }
    }
    Ok(seats)
}

/// Whether `project` has an open voyage (T-570): one that has departed and
/// whose work is not all settled — the board's "sailing" voyage, the project
/// the user is working on.
fn has_open_voyage(tx: &Tx<'_>, project: &str) -> GearResult<bool> {
    let tasks = tx.tasks_of_project(project)?;
    for voyage in tx.voyages_of_project(project)? {
        if voyage.departure.is_none() {
            continue;
        }
        let open = tasks.iter().any(|task| {
            task.milestone.as_deref() == Some(voyage.milestone.as_str())
                && !matches!(
                    task.state,
                    TaskState::Done | TaskState::Failed | TaskState::Cancelled
                )
        });
        if open {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The name an unnamed agent on `pane` gets (T-570): the name of a seat whose
/// project has an open voyage first — that is the project being worked on —
/// and, of the seats that tie, the one bound first (the older rule of T-485).
fn primary_name(store: &mut Store, on_pane: &[(SeatRow, Ts)]) -> GearResult<String> {
    let mut best: Option<(&SeatRow, Ts, bool)> = None;
    for (seat, at) in on_pane {
        let open = store.read(|tx| has_open_voyage(tx, &seat.project))?;
        let better = match &best {
            None => true,
            Some((current, current_at, current_open)) => {
                (open, std::cmp::Reverse(*at), seat.id.as_str())
                    > (
                        *current_open,
                        std::cmp::Reverse(*current_at),
                        current.id.as_str(),
                    )
            }
        };
        if better {
            best = Some((seat, *at, open));
        }
    }
    Ok(best.map(|(seat, _, _)| desired(seat)).unwrap_or_else(|| {
        on_pane
            .first()
            .map(|(seat, _)| desired(seat))
            .unwrap_or_default()
    }))
}

/// [`ensure`] for a pane that holds the seats of one or several projects
/// (T-570): the agent keeps the name of **any** seat bound to that pane, and
/// only an agent with no name — or with a name no seat of the pane has — is
/// renamed to `name`.
///
/// This is the naming a delivery, a doorbell or a key send does before it
/// types: an agent Herdr renamed after the other project's captain must not be
/// renamed back, or the prompts by the kept name fail with `agent_not_found`.
pub fn ensure_for(store: &mut Store, herdr: &mut dyn HerdrAct, pane: &str, name: &str) -> Ensure {
    let keep = kept_on_pane(store, pane);
    match read(herdr) {
        Ok(names) => gear_herdr::ensure_agent_name_keeping(herdr, &names, pane, name, &keep),
        Err(error) => Ensure::Failed(error.0),
    }
}

/// The names an agent in `pane` may keep (T-570): the desired name of every
/// seat bound to it. Empty when the store cannot say — the plain rule applies
/// then, and the agent is renamed to the seat's own name.
pub fn kept_on_pane(store: &mut Store, pane: &str) -> Vec<String> {
    store
        .read(|tx| {
            Ok(seats_on_pane(tx, pane)?
                .into_iter()
                .map(|(seat, _)| desired(&seat))
                .collect::<Vec<String>>())
        })
        .unwrap_or_default()
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
    // T-482: every seat's name is its own, so a finished reviewer and the next
    // job of the same seat ask for the same name and no pass has to give one
    // back before another asks. The order stays as the store gives it.
    let mut names = names.clone();
    for seat in seats {
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
        // A pane may hold the seats of several projects (T-485, T-570): its one
        // agent carries one name, and every seat bound to that pane accepts it.
        // Naming it after each seat in turn flipped the Prometheus captain
        // between `prometheus-captain` and `lawrence-captain` every pass, and
        // every prompt by the other name failed. Where the pass has to choose —
        // the agent has no name — the seat whose project has an open voyage
        // goes first, and the seat bound first after it.
        let on_pane = store.read(|tx| seats_on_pane(tx, &binding.pane_id))?;
        let keep: Vec<String> = on_pane.iter().map(|(seat, _)| desired(seat)).collect();
        let name = primary_name(store, &on_pane)?;
        match gear_herdr::ensure_agent_name_keeping(herdr, &names, &binding.pane_id, &name, &keep) {
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
            // T-570's keep-set is `Named`, not `Kept`; `Kept` stays the
            // `captain take` on a pane that already holds another project's
            // captain seat (T-552), and the engine has nothing to do about it.
            Ensure::Kept => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_herdr::AgentStatus;
    use gear_store::testing;

    const NOW: Ts = 1_800_000_000;
    const PANE: &str = "w2:p10";

    /// Two projects, each with its captain seat bound to the same pane — the
    /// incident: one agent carries two seats.
    fn two_captains() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "prometheus", "sess", NOW)?;
                testing::seed_project(tx, "lawrence", "sess", NOW)?;
                testing::seed_active_seat(
                    tx,
                    "prometheus",
                    "captain",
                    SeatRole::Captain,
                    PANE,
                    "claude",
                    NOW,
                )?;
                testing::seed_active_seat(
                    tx,
                    "lawrence",
                    "captain",
                    SeatRole::Captain,
                    PANE,
                    "claude",
                    NOW,
                )?;
                Ok(())
            })
            .unwrap();
        store
    }

    /// A departed voyage of `project`, with one task still running (T-570's
    /// "open voyage").
    fn open_voyage(store: &mut Store, project: &str) {
        store
            .write(|tx| {
                tx.upsert_voyage(&gear_store::VoyageRow {
                    project: project.to_string(),
                    milestone: "M1".to_string(),
                    title: "a voyage".to_string(),
                    done: None,
                    bar: None,
                    eta: None,
                    departure: Some(NOW),
                    departure_tasks: Some(1),
                    departure_estimate_min: Some(30),
                    updated: NOW,
                })?;
                tx.insert_task(&gear_store::TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: project.to_string(),
                    id: "T-1".to_string(),
                    rank: 1,
                    state: TaskState::Running,
                    priority: "must".to_string(),
                    lane: Some("heat".to_string()),
                    milestone: Some("M1".to_string()),
                    required: false,
                    contract_version: 1,
                    deps: Vec::new(),
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(30),
                    issue: None,
                    updated: NOW,
                })
            })
            .unwrap();
    }

    /// T-570: a pane that holds two seats keeps the name the agent carries,
    /// pass after pass, and a delivery to either seat still finds it.
    #[test]
    fn a_pane_with_two_seats_keeps_one_agent_name() {
        let mut store = two_captains();
        let mut herdr = FakeHerdr::new();
        // The user named the agent after its own project (prometheus-captain);
        // the engine's restore must not set it back to lawrence-captain.
        herdr.add_agent_named(PANE, "claude", AgentStatus::Idle, "prometheus-captain");

        for pass in 0..3 {
            let names = read(&mut herdr).unwrap();
            let restored =
                restore(&mut store, &mut herdr, "prometheus", &names, NOW + pass).unwrap();
            assert!(restored.renamed.is_empty(), "pass {pass}: {restored:?}");
            let restored = restore(&mut store, &mut herdr, "lawrence", &names, NOW + pass).unwrap();
            assert!(restored.renamed.is_empty(), "pass {pass}: {restored:?}");
            assert_eq!(
                read(&mut herdr)
                    .unwrap()
                    .get(PANE)
                    .cloned()
                    .flatten()
                    .as_deref(),
                Some("prometheus-captain"),
                "pass {pass}: the name stayed"
            );
        }

        // A delivery to either seat finds the agent by the name it carries.
        let names = read(&mut herdr).unwrap();
        for seat in ["prometheus:captain", "lawrence:captain"] {
            let wanted = format!("{}-captain", seat.split(':').next().unwrap_or_default());
            let target = resolve_target(&mut herdr, &names, PANE, &wanted, "claude");
            assert_eq!(
                target,
                Target::Prompt {
                    name: "prometheus-captain".to_string()
                },
                "{seat}"
            );
            // And the naming before a delivery keeps the name: it is a seat's.
            assert_eq!(
                ensure_for(&mut store, &mut herdr, PANE, &wanted),
                Ensure::Named,
                "{seat}"
            );
            assert_eq!(
                read(&mut herdr)
                    .unwrap()
                    .get(PANE)
                    .cloned()
                    .flatten()
                    .as_deref(),
                Some("prometheus-captain"),
                "{seat}: the name stayed"
            );
        }
    }

    /// T-570: an unnamed agent on a pane with two seats gets the name of the
    /// seat whose project has an open voyage — that is the project being worked
    /// on.
    #[test]
    fn an_unnamed_agent_gets_the_open_voyages_seat_name() {
        let mut store = two_captains();
        open_voyage(&mut store, "lawrence");
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "claude", AgentStatus::Idle);

        let names = read(&mut herdr).unwrap();
        let restored = restore(&mut store, &mut herdr, "prometheus", &names, NOW).unwrap();

        assert_eq!(restored.renamed, vec!["prometheus:captain".to_string()]);
        assert_eq!(
            read(&mut herdr)
                .unwrap()
                .get(PANE)
                .cloned()
                .flatten()
                .as_deref(),
            Some("lawrence-captain"),
            "the open voyage's project names the pane"
        );
        // A second pass keeps it: both seats accept the name.
        let names = read(&mut herdr).unwrap();
        let restored = restore(&mut store, &mut herdr, "lawrence", &names, NOW + 1).unwrap();
        assert!(restored.renamed.is_empty(), "{restored:?}");
    }
}
