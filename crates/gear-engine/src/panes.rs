//! Closing a seat's pane (Prometheus, 2026-11-05).
//!
//! The engine closes a pane to end a seat's agent: a released reviewer, a
//! dead lane's shell, a retired seat whose agent still runs. A pane can hold
//! more than one seat (a captain pane holds the captain seats of several
//! projects), and a name or a stale binding can point at a pane another seat
//! now uses. The captain of Prometheus was killed that way: a released
//! Lawrence captain seat's name was on the Prometheus captain's pane, and the
//! retired-seat sweep closed it.
//!
//! One rule, at every close: a pane on which another seat's current binding is
//! active is never closed. The close is refused with `pane_in_use`, naming the
//! seats, and nothing is typed into it either.
use gear_herdr::{HerdrAct, Outcome};
use gear_model::GearResult;
use gear_store::Store;

/// The code of a close refused because another seat uses the pane.
pub const PANE_IN_USE: &str = "pane_in_use";
/// The refusal code of a pane another agent holds.
pub const PANE_FOREIGN: &str = "pane_foreign";

/// The seats other than `own` whose active binding is on `pane`.
pub fn others_on_pane(store: &mut Store, pane: &str, own: &str) -> GearResult<Vec<String>> {
    Ok(store
        .read(|tx| tx.active_seats_on_pane(pane))?
        .into_iter()
        .filter(|seat| seat != own)
        .collect())
}

/// Close `pane` for the seat `own`, unless another seat uses it.
pub fn close_seat_pane(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    pane: &str,
    own: &str,
) -> GearResult<Outcome<()>> {
    // Another agent holds the pane (pharos, 2026-11-16): closing it would end
    // somebody else's work.
    let foreign = store.read(|tx| {
        let Some(seat) = tx.get_seat(own)? else {
            return Ok(false);
        };
        let bound_here = tx
            .get_binding(own, seat.generation)?
            .is_some_and(|binding| binding.pane_id == pane);
        Ok(bound_here && crate::observe::is_foreign(tx, own, seat.generation)?)
    })?;
    if foreign {
        return Ok(Outcome::Refused {
            code: PANE_FOREIGN.to_string(),
            message: format!("pane {pane} is not closed for {own}: another agent holds it"),
        });
    }
    let others = others_on_pane(store, pane, own)?;
    if !others.is_empty() {
        return Ok(Outcome::Refused {
            code: PANE_IN_USE.to_string(),
            message: format!(
                "pane {pane} is not closed for {own}: {} is bound there",
                others.join(", ")
            ),
        });
    }
    Ok(herdr.pane_close(pane))
}
