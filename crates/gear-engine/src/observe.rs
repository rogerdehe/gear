//! Observation and reconciliation (design/08).

use gear_herdr::{AgentInfo, AgentStatus, HerdrRead, ScreenFlag};
use gear_model::{GearResult, SeatState};
use gear_store::{SeatRow, Tx};
use std::collections::BTreeSet;

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
    profiles: &std::collections::BTreeMap<String, gear_herdr::Profile>,
) -> GearResult<Vec<SeatObs>> {
    todo!("slice1")
}

/// States considered "may receive a doorbell/delivery".
pub fn reachable(state: SeatState) -> bool {
    matches!(state, SeatState::Active | SeatState::Standby | SeatState::Retiring)
}
