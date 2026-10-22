//! Observation and reconciliation (design/08).

use gear_herdr::{classify_screen, AgentInfo, AgentStatus, HerdrRead, Profile, ScreenFlag};
use gear_model::{GearResult, SeatState};
use gear_store::{BindingStatus, SeatRow, Tx};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    /// Expected agent is in the bound pane (kind matches binding).
    Present,
    /// Pane gone or no agent, and the seat expects one.
    Missing,
    /// A different agent occupies the pane with no pending onboard/release op.
    Changed,
    /// Seat expects no agent (Queued/Released) or is mid onboard/release.
    NotExpected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatObs {
    pub seat: SeatRow,
    pub presence: Presence,
    pub status: Option<AgentStatus>,
    /// Only filled when status is idle/done (one tail read).
    pub flags: BTreeSet<ScreenFlag>,
    /// Idle/done with no Busy flag.
    pub idle: bool,
}

/// One `agents()` call for the whole session. `Err` = observation failed:
/// the caller must make **no** decisions for this project this tick.
pub fn snapshot(herdr: &mut dyn HerdrRead) -> Result<Vec<AgentInfo>, gear_herdr::ObserveError> {
    herdr.agents()
}

/// Compare seats with the snapshot (reads bindings inside `tx`; reads screen
/// tails via `herdr` only for idle/done agents — callers must not hold a write
/// transaction while doing so, so this takes a read `Tx`).
/// Screen flags use `profiles[seat.profile]`; a seat whose profile is missing
/// is never `idle` (fail closed).
/// Slice 1 does not change seat states; slice 2+ acts on Missing/Changed.
pub fn reconcile(
    tx: &Tx<'_>,
    herdr: &mut dyn HerdrRead,
    project: &str,
    agents: &[AgentInfo],
    profiles: &BTreeMap<String, Profile>,
) -> GearResult<Vec<SeatObs>> {
    let mut observed = Vec::new();
    for seat in tx.seats_of_project(project)? {
        observed.push(observe_seat(tx, herdr, seat, agents, profiles)?);
    }
    Ok(observed)
}

/// One seat against the snapshot: is the expected agent in its pane, and is it
/// idle?
fn observe_seat(
    tx: &Tx<'_>,
    herdr: &mut dyn HerdrRead,
    seat: SeatRow,
    agents: &[AgentInfo],
    profiles: &BTreeMap<String, Profile>,
) -> GearResult<SeatObs> {
    if !expects_agent(seat.state) {
        // The seat expects no agent (queued/released, or mid onboard/release):
        // nothing is judged about it.
        return Ok(SeatObs {
            seat,
            presence: Presence::NotExpected,
            status: None,
            flags: BTreeSet::new(),
            idle: false,
        });
    }

    let binding = tx
        .get_binding(&seat.id, seat.generation)?
        .filter(|b| b.status == BindingStatus::Active);
    let Some(binding) = binding else {
        // The seat expects an agent but has no active binding to look at.
        return Ok(SeatObs {
            seat,
            presence: Presence::Missing,
            status: None,
            flags: BTreeSet::new(),
            idle: false,
        });
    };

    let agent = agents.iter().find(|a| a.pane_id == binding.pane_id);
    let Some(agent) = agent else {
        // The pane is gone or hosts no agent.
        return Ok(SeatObs {
            seat,
            presence: Presence::Missing,
            status: None,
            flags: BTreeSet::new(),
            idle: false,
        });
    };

    if agent.agent.as_deref() != Some(binding.kind.as_str()) {
        // Another agent occupies the pane: the seat's agent is not there.
        return Ok(SeatObs {
            seat,
            presence: Presence::Changed,
            status: Some(agent.agent_status),
            flags: BTreeSet::new(),
            idle: false,
        });
    }

    let (flags, idle) = screen_flags(herdr, &seat, &binding.pane_id, agent.agent_status, profiles);
    Ok(SeatObs { seat, presence: Presence::Present, status: Some(agent.agent_status), flags, idle })
}

/// Whether the seat expects an agent to be in its pane. Queued and Released
/// seats have none, and a seat that is being onboarded or released is in a
/// change that must not count as "missing" (design/08).
fn expects_agent(state: SeatState) -> bool {
    !matches!(state, SeatState::Queued | SeatState::Released | SeatState::Starting | SeatState::Releasing)
}

/// The screen flags of the seat's agent and whether it is idle.
///
/// The screen is read only for an idle/done agent, and only when the profile
/// is known. Every other case fails closed: the seat is not idle, so no
/// doorbell goes out on a guess.
fn screen_flags(
    herdr: &mut dyn HerdrRead,
    seat: &SeatRow,
    pane_id: &str,
    status: AgentStatus,
    profiles: &BTreeMap<String, Profile>,
) -> (BTreeSet<ScreenFlag>, bool) {
    if !matches!(status, AgentStatus::Idle | AgentStatus::Done) {
        // A working agent is not idle, and its screen is not read.
        return (BTreeSet::new(), false);
    }
    let Some(profile) = profiles.get(&seat.profile) else {
        // Unknown profile: the screen cannot be read the way it must be.
        return (BTreeSet::new(), false);
    };
    let Ok(tail) = herdr.read_tail(pane_id, profile.tail_lines) else {
        // The screen could not be read: fail closed.
        return (BTreeSet::new(), false);
    };
    let flags = classify_screen(profile, &tail);
    let idle = !flags.contains(&ScreenFlag::Busy);
    (flags, idle)
}

/// States considered "may receive a doorbell/delivery".
pub fn reachable(state: SeatState) -> bool {
    matches!(state, SeatState::Active | SeatState::Standby | SeatState::Retiring)
}
