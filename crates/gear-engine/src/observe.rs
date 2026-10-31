//! Observation and reconciliation (design/08).

use gear_herdr::{classify_screen, AgentInfo, AgentStatus, HerdrRead, Profile, ScreenFlag};
use gear_model::{GearResult, SeatState};
use gear_store::{BindingStatus, SeatRow, Store, Tx};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    /// Expected agent is in the bound pane (kind matches binding).
    Present,
    /// Pane gone or no agent, and the seat expects one.
    Missing,
    /// A different agent occupies the pane with no pending onboard/release op.
    Changed,
    /// Seat expects no agent (`SeatState::expects_agent()` is false) or is mid onboard/release.
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
    /// The last screen lines of the agent, when the screen was read: an idle
    /// or done agent, a blocked one (the dialog it waits on) or a working
    /// agent of a kind that shows a provider back-off (T-75). None when the
    /// screen was not read.
    pub screen: Option<String>,
    /// The session the agent in the pane reports (T-97). A hand restart of the
    /// agent shows a new one: that is how a delivery that was never confirmed
    /// gets its second chance without a store edit.
    pub session: Option<String>,
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
    if !seat.state.expects_agent() {
        // The seat expects no agent (queued/released, mid onboard/release, or
        // already missing/retired/needs-check): nothing is judged about it.
        return Ok(SeatObs {
            seat,
            presence: Presence::NotExpected,
            status: None,
            flags: BTreeSet::new(),
            idle: false,
            screen: None,
            session: None,
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
            screen: None,
            session: None,
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
            screen: None,
            session: None,
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
            screen: None,
            session: None,
        });
    }

    let (flags, idle, screen) =
        screen_of(herdr, &seat, &binding.pane_id, agent.agent_status, profiles);
    Ok(SeatObs {
        seat,
        presence: Presence::Present,
        status: Some(agent.agent_status),
        flags,
        idle,
        screen,
        session: agent
            .agent_session
            .as_ref()
            .map(|session| session.value.clone()),
    })
}

/// The screen of the seat's agent, the flags it shows and whether the agent
/// is idle.
///
/// The screen is read for an idle or done agent (its flags say what the seat
/// may do next) and for a blocked one (the captain's alert quotes the dialog
/// it waits on). A **working** agent's screen is read only for a kind that
/// shows a provider back-off (T-75): such an agent waits for the provider
/// for minutes without any output, and that is not a stall. Every other
/// working agent costs no read.
///
/// A screen that cannot be read, and a seat whose profile is unknown, fail
/// closed: no flags and no idle.
fn screen_of(
    herdr: &mut dyn HerdrRead,
    seat: &SeatRow,
    pane_id: &str,
    status: AgentStatus,
    profiles: &BTreeMap<String, Profile>,
) -> (BTreeSet<ScreenFlag>, bool, Option<String>) {
    let profile = profiles.get(&seat.profile);
    let idle_or_done = matches!(status, AgentStatus::Idle | AgentStatus::Done);
    let blocked = status == AgentStatus::Blocked;
    // A working agent is read only for a kind that shows a back-off.
    let shows_backoff = profile
        .map(|profile| !profile.backoff_markers.is_empty())
        .unwrap_or(false);
    if !idle_or_done && !blocked && !shows_backoff {
        return (BTreeSet::new(), false, None);
    }
    let lines = profile.map(|profile| profile.tail_lines).unwrap_or(16);
    let Ok(tail) = herdr.read_tail(pane_id, lines) else {
        // The screen could not be read: fail closed.
        return (BTreeSet::new(), false, None);
    };
    if idle_or_done {
        let Some(profile) = profile else {
            // Unknown profile: the screen cannot be read the way it must be.
            return (BTreeSet::new(), false, None);
        };
        let flags = classify_screen(profile, &tail);
        // M4 §10.2: whether an agent is busy is Herdr's state, never the text
        // on its screen. An idle or done agent is idle.
        let idle = idle_or_done;
        return (flags, idle, Some(tail));
    }
    if blocked {
        // A blocked agent is judged by its status, not by its flags.
        return (BTreeSet::new(), false, Some(tail));
    }
    // A working agent: only a provider back-off is of interest (T-75).
    let mut flags = BTreeSet::new();
    if let Some(profile) = profile {
        if profile.backoff(&tail) {
            flags.insert(ScreenFlag::Backoff);
        }
        // A guardrails refusal (T-113) is read on every screen that is read:
        // the agent goes on after it, so the screen is the only place it is
        // ever visible.
        if profile.blocked(&tail) {
            flags.insert(ScreenFlag::Blocked);
        }
    }
    (flags, false, Some(tail))
}

/// Keep each seat's binding session equal to the one its agent reports (T-97).
///
/// An agent restarted by hand in its pane is a new session with the same
/// generation: the store must say so, because the record of a delivery that was
/// never confirmed is compared with the session it was sent into. Only a
/// present agent with a session value updates the binding; a kind that reports
/// none leaves it as it is.
pub fn record_sessions(store: &mut Store, observed: &[SeatObs]) -> GearResult<()> {
    let updates: Vec<(String, i64, String)> = observed
        .iter()
        .filter(|obs| obs.presence == Presence::Present)
        .filter_map(|obs| {
            let session = obs.session.as_ref().filter(|session| !session.is_empty())?;
            Some((obs.seat.id.clone(), obs.seat.generation, session.clone()))
        })
        .collect();
    if updates.is_empty() {
        return Ok(());
    }
    store.write(|tx| {
        for (seat, generation, session) in updates {
            let current = tx
                .get_binding(&seat, generation)?
                .and_then(|binding| binding.session_ref);
            if current.as_deref() != Some(session.as_str()) {
                tx.set_binding_session_ref(&seat, generation, &session)?;
            }
        }
        Ok(())
    })
}

/// States considered "may receive a doorbell/delivery".
pub fn reachable(state: SeatState) -> bool {
    matches!(
        state,
        SeatState::Active | SeatState::Standby | SeatState::Retiring
    )
}
