//! Rounds: the ownership of a seat's agent records (design §4.1).
//!
//! A round owns every agent record on its seat from its `start_at` until its
//! `end_at`: the start of the next round on the same seat, or the seat's
//! release — whichever comes first. There is no "between rounds" bucket: what
//! a seat does after a hand-in belongs to the round it handed in.
//!
//! The tick of this module keeps that bookkeeping true:
//!
//! - the session the seat's binding carries now joins the open round, so a
//!   `/new`, a restart or a resume inside the round is counted in it (once —
//!   the round keeps the list of the sessions it owns);
//! - a round ends when the next round on its seat starts, or when the seat is
//!   released.

use gear_herdr::HerdrAct;
use gear_model::{GearResult, SeatState, Ts};
use gear_store::{RoundKey, Store, Tx};

use crate::EngineCfg;

/// What one tick of the rounds bookkeeping did (for the tick report).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoundReport {
    /// The rounds that ended this tick (`task/attempt`).
    pub closed: Vec<String>,
    /// The rounds that learned a new session this tick (`task/attempt`).
    pub sessions: Vec<String>,
}

/// The rounds bookkeeping of every project (design §4.1).
///
/// The token counting of the closed rounds (§4.3-4.4) is not here yet: it
/// needs the usage reader, which lands with its own task.
pub fn tick(
    store: &mut Store,
    _herdr: &mut dyn HerdrAct,
    _cfg: &EngineCfg,
    now: Ts,
) -> GearResult<RoundReport> {
    let mut report = RoundReport::default();
    let projects = store.read(|tx| tx.project_ids())?;
    for project in projects {
        store.write(|tx| {
            let rounds = tx.rounds_of_project(&project)?;
            // 1. §4.1: a round ends when the next round on the same seat
            //    starts. The seat's open round is the newest one, so every
            //    other open round of that seat ends here.
            for round in &rounds {
                if round.end_at.is_some() {
                    continue;
                }
                let next = rounds
                    .iter()
                    .filter(|other| other.seat == round.seat && other.start_at > round.start_at)
                    .min_by_key(|other| other.start_at);
                if let Some(next) = next {
                    tx.close_round(&key_of(round), next.start_at)?;
                    report.closed.push(label(round));
                }
            }
            // 2. The session the seat carries now belongs to its open round —
            //    before the release below closes it, so the round keeps the
            //    session of its last work.
            for seat in tx.seats_of_project(&project)? {
                let Some(binding) = tx.get_binding(&seat.id, seat.generation)? else {
                    continue;
                };
                let Some(session) = binding.session_ref.as_deref().filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let Some(round) = tx.open_round_of_seat(&seat.id)? else {
                    continue;
                };
                if round.sessions.iter().any(|owned| owned == session) {
                    continue;
                }
                tx.add_round_session(&key_of(&round), session)?;
                report.sessions.push(label(&round));
            }
            // 3. §4.1: or when the seat is released — the agent is gone and
            //    its records belong to the round it handed in. The round keeps
            //    the release time, not the time of this tick.
            for round in tx.rounds_of_project(&project)? {
                if round.end_at.is_some() {
                    continue;
                }
                let Some(seat) = tx.get_seat(&round.seat)? else {
                    continue;
                };
                if matches!(seat.state, SeatState::Released | SeatState::Retired) {
                    tx.close_round(&key_of(&round), seat.state_since)?;
                    report.closed.push(label(&round));
                }
            }
            Ok(())
        })?;
    }
    let _ = now;
    Ok(report)
}

/// The key of a round (the store's identity of one round).
fn key_of(round: &gear_store::RoundRow) -> RoundKey {
    RoundKey {
        project: round.project.clone(),
        task: round.task.clone(),
        attempt: round.attempt,
        role: round.role.clone(),
    }
}

/// How a round is named in the report.
fn label(round: &gear_store::RoundRow) -> String {
    format!("{}/{}/{}", round.task, round.attempt, round.role)
}

/// The open round of a seat, if it has one (the round it is working on).
pub fn open_round(
    tx: &Tx<'_>,
    seat: &str,
) -> GearResult<Option<gear_store::RoundRow>> {
    tx.open_round_of_seat(seat)
}
