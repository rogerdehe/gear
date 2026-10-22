//! Seat lifecycle around onboarding (design/03, /10). Transaction-level, like
//! `tasks`: the engine calls these inside its own transactions; Herdr is never
//! touched here. Transitions via `SeatState::can_transition` for the seat's
//! role → `BadTransition`; missing seat/binding → `NoTarget`.

use gear_model::{GearResult, Ts};
use gear_store::Tx;

/// Start onboarding: the seat (Queued, or Missing/NeedsCheck for a restart)
/// moves to Starting, its generation becomes `generation + 1`, every earlier
/// binding of the seat that is not Void becomes Void, and a Pending binding
/// (`pane_id`, `kind`, session = the project's Herdr session) is inserted for the
/// new generation. Returns the new generation.
pub fn begin_onboard(tx: &Tx<'_>, seat_id: &str, pane_id: &str, kind: &str, now: Ts) -> GearResult<i64> {
    let _ = (tx, seat_id, pane_id, kind, now);
    todo!("T-59")
}

/// The engine saw the expected agent in the bound pane: the Pending binding of
/// `generation` becomes Active (with `session_ref` when Herdr reported one) and
/// the seat goes Starting → Active. A generation that is not the seat's current
/// one → `StaleGeneration`; a binding that is not Pending → `BadTransition`.
pub fn finish_onboard(tx: &Tx<'_>, seat_id: &str, generation: i64, session_ref: Option<&str>, now: Ts) -> GearResult<()> {
    let _ = (tx, seat_id, generation, session_ref, now);
    todo!("T-59")
}
