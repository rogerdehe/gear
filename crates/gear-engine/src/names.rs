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
use gear_model::SeatRole;
use gear_model::{GearResult, MsgKind, MsgState, Ts};
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
    // T-605: the old pane of a seat's own CLI switch is not a live agent of
    // the quota while the plan runs; it belongs to the switch.
    let switching = switching_old_panes(store, project);
    names
        .into_iter()
        .filter(|(pane, _)| !switching.iter().any(|old| old == pane))
        .filter(|(_, name)| {
            name.as_deref()
                .is_some_and(|name| wanted.iter().any(|wanted| wanted == name))
        })
        .map(|(pane, _)| pane)
        .collect()
}

/// The old panes of the seats' own CLI switches (T-605): while a split restart
/// plan runs, the old agent's pane belongs to the seat's switch — it is not a
/// live agent of the lane or temp quota, and no sweep closes it as a stray.
pub(crate) fn switching_old_panes(store: &mut Store, project: &str) -> Vec<String> {
    let mut panes = Vec::new();
    let Ok(seats) = store.read(|tx| tx.seats_of_project(project)) else {
        return panes;
    };
    for seat in seats {
        let Ok(Some(plan)) = crate::restart::pending(store, &seat.id) else {
            continue;
        };
        if plan.new_pane.is_none() {
            continue;
        }
        let generation = if plan.old_generation > 0 {
            plan.old_generation
        } else {
            plan.generation.saturating_sub(1)
        };
        let Ok(Some(binding)) = store.read(|tx| tx.get_binding(&seat.id, generation)) else {
            continue;
        };
        panes.push(binding.pane_id);
    }
    panes
}

/// Whether the seat's name moves to another pane later in this switch (T-605):
/// until the Finish phase has moved it, the name pass must not rename the new
/// pane and report the old pane as a holder.
fn name_moves_later(store: &mut Store, seat: &str) -> bool {
    match crate::restart::pending(store, seat) {
        Ok(Some(plan)) => {
            plan.new_pane.is_some()
                && matches!(
                    plan.phase,
                    crate::restart::Phase::Split
                        | crate::restart::Phase::Start
                        | crate::restart::Phase::Finish
                )
        }
        _ => false,
    }
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

/// The meta key that names the captain seat a pane took last: written by
/// `gear captain take` and by a handover (gear_app::captain), read here.
pub use gear_app::captain::pane_named_key;

/// When `seat` came to `pane` (its current run of generations on that pane):
/// a rebind of the same pane (a new session, a restart) is not an arrival, so
/// the earliest binding of the run counts, not the newest.
fn arrived_on_pane(tx: &Tx<'_>, seat: &SeatRow, pane: &str, newest: Ts) -> GearResult<Ts> {
    let mut arrived = newest;
    let mut generation = seat.generation;
    while generation > 1 {
        generation -= 1;
        match tx.get_binding(&seat.id, generation)? {
            Some(binding) if binding.pane_id == pane => arrived = arrived.min(binding.created),
            Some(_) => break,
            None => continue,
        }
    }
    Ok(arrived)
}

/// The name the agent on `pane` carries: the name of the captain seat the pane
/// took last. A captain that takes over another fleet carries that fleet's
/// name from then on, never the name of the fleet it came from (user,
/// 2026-11-17; T-552 and T-570 kept whichever name the agent had, so every
/// takeover went on showing the old fleet). The seat named by the last take
/// wins; without one (a take before this rule, a released seat), the seat that
/// came to the pane last, then the seat id.
fn primary_name(store: &mut Store, pane: &str, on_pane: &[(SeatRow, Ts)]) -> GearResult<String> {
    store.read(|tx| {
        if let Some(taken) = tx.get_meta(&pane_named_key(pane))? {
            if let Some((seat, _)) = on_pane.iter().find(|(seat, _)| seat.id == taken) {
                return Ok(desired(seat));
            }
        }
        let mut best: Option<(Ts, &SeatRow)> = None;
        for (seat, created) in on_pane {
            let arrived = arrived_on_pane(tx, seat, pane, *created)?;
            if best
                .is_none_or(|(at, current)| (arrived, seat.id.as_str()) > (at, current.id.as_str()))
            {
                best = Some((arrived, seat));
            }
        }
        Ok(best.map(|(_, seat)| desired(seat)).unwrap_or_default())
    })
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
        // T-605: a CLI switch moves the seat's name to its new pane in the
        // Finish phase; until then the old pane holds it and the new one
        // carries a temporary name, so the pass leaves the seat alone.
        if name_moves_later(store, &seat.id) {
            continue;
        }
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
        // Another agent holds the pane: it keeps its own name (pharos,
        // 2026-11-16 — the seat's name on it is what aimed the seat's prompts
        // at it).
        if store.read(|tx| crate::observe::is_foreign(tx, &seat.id, seat.generation))? {
            continue;
        }
        // A pane may hold the seats of several projects (T-485, T-570): its one
        // agent carries one name, the name of the captain seat the pane took
        // last ([`primary_name`]). Every pass of every project picks the same
        // one, so the name never flips; a delivery to the other seat goes to
        // the pane's live name ([`resolve_target`]).
        let on_pane = store.read(|tx| seats_on_pane(tx, &binding.pane_id))?;
        let name = primary_name(store, &binding.pane_id, &on_pane)?;
        match gear_herdr::ensure_agent_name_keeping(herdr, &names, &binding.pane_id, &name, &[]) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_herdr::AgentStatus;
    use gear_store::testing;

    const NOW: Ts = 1_800_000_000;
    const PANE: &str = "w2:p10";

    /// Two captain seats on one pane: `first` came at NOW, `second` an hour
    /// later (the captain of `first` took over `second`'s fleet).
    fn took_over(first: &str, second: &str) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                for (project, at) in [(first, NOW), (second, NOW + 3600)] {
                    testing::seed_project(tx, project, "sess", NOW)?;
                    testing::seed_active_seat(
                        tx,
                        project,
                        "captain",
                        SeatRole::Captain,
                        PANE,
                        "claude",
                        at,
                    )?;
                }
                Ok(())
            })
            .unwrap();
        store
    }

    fn name_of(herdr: &mut FakeHerdr) -> Option<String> {
        read(herdr).unwrap().get(PANE).cloned().flatten()
    }

    /// A captain that took over another fleet carries that fleet's name from
    /// the next pass, and keeps it pass after pass; a delivery to either seat
    /// still reaches the agent by the name it carries.
    #[test]
    fn a_captain_that_took_over_another_fleet_carries_its_name() {
        let mut store = took_over("prometheus", "lawrence");
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(PANE, "claude", AgentStatus::Idle, "prometheus-captain");

        for pass in 0..3 {
            for project in ["prometheus", "lawrence"] {
                let names = read(&mut herdr).unwrap();
                restore(&mut store, &mut herdr, project, &names, NOW + pass).unwrap();
                assert_eq!(
                    name_of(&mut herdr).as_deref(),
                    Some("lawrence-captain"),
                    "pass {pass}, {project}"
                );
            }
        }
        assert_eq!(
            herdr.renames_to("lawrence-captain"),
            vec![PANE],
            "renamed once"
        );
        assert!(
            herdr.renames_to("prometheus-captain").is_empty(),
            "never back"
        );

        let names = read(&mut herdr).unwrap();
        for wanted in ["prometheus-captain", "lawrence-captain"] {
            assert_eq!(
                resolve_target(&mut herdr, &names, PANE, wanted, "claude"),
                Target::Prompt {
                    name: "lawrence-captain".to_string()
                },
                "{wanted}"
            );
            assert_eq!(
                ensure_for(&mut store, &mut herdr, PANE, wanted),
                Ensure::Named
            );
        }
    }

    /// A rebind of the same pane (a new session) is not an arrival: the fleet
    /// taken over keeps the name though the first seat's binding is newer.
    #[test]
    fn a_rebind_on_the_same_pane_does_not_take_the_name_back() {
        let mut store = took_over("prometheus", "lawrence");
        store
            .write(|tx| {
                let seat = tx.get_seat("prometheus:captain")?.unwrap();
                let mut binding = tx.get_binding(&seat.id, seat.generation)?.unwrap();
                binding.generation = seat.generation + 1;
                binding.created = NOW + 7200;
                tx.insert_binding(&binding)?;
                tx.set_seat_generation(&seat.id, seat.generation + 1)
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "claude", AgentStatus::Idle);

        let names = read(&mut herdr).unwrap();
        restore(&mut store, &mut herdr, "prometheus", &names, NOW).unwrap();

        assert_eq!(name_of(&mut herdr).as_deref(), Some("lawrence-captain"));
    }

    /// The seat named by the last `captain take` wins over arrival order: the
    /// captain took back the first fleet.
    #[test]
    fn the_last_take_names_the_pane() {
        let mut store = took_over("prometheus", "lawrence");
        store
            .write(|tx| tx.set_meta(&pane_named_key(PANE), "prometheus:captain"))
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(PANE, "claude", AgentStatus::Idle, "lawrence-captain");

        let names = read(&mut herdr).unwrap();
        restore(&mut store, &mut herdr, "lawrence", &names, NOW).unwrap();

        assert_eq!(name_of(&mut herdr).as_deref(), Some("prometheus-captain"));
    }
}
