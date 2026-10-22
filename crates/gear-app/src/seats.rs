//! Seat lifecycle around onboarding (design/03, /10). Transaction-level, like
//! `tasks`: the engine calls these inside its own transactions; Herdr is never
//! touched here. Transitions via `SeatState::can_transition` for the seat's
//! role → `BadTransition`; missing seat/binding → `NoTarget`.

use gear_model::state::require_transition;
use gear_model::{ErrorCode, GearError, GearResult, SeatState, Ts};
use gear_store::{BindingRow, BindingStatus, Tx};

/// Start onboarding: the seat (Queued, or Missing/NeedsCheck for a restart)
/// moves to Starting, its generation becomes `generation + 1`, every earlier
/// binding of the seat that is not Void becomes Void, and a Pending binding
/// (`pane_id`, `kind`, session = the project's Herdr session) is inserted for the
/// new generation. Returns the new generation.
pub fn begin_onboard(tx: &Tx<'_>, seat_id: &str, pane_id: &str, kind: &str, now: Ts) -> GearResult<i64> {
    let seat = tx.get_seat(seat_id)?.ok_or_else(|| {
        GearError::new(ErrorCode::NoTarget, format!("no seat {seat_id}"))
    })?;

    // A seat starts onboarding from Queued (freshly registered) or from
    // Missing/NeedsCheck (a restart). The model's lane table has no
    // Queued -> Starting (a registered seat is Queued, design/03), so Queued
    // is allowed here and every other source state goes through the table.
    if seat.state != SeatState::Queued {
        require_transition(
            SeatState::can_transition(seat.role, seat.state, SeatState::Starting),
            &format!("seat {seat_id}"),
            seat.state.as_str(),
            SeatState::Starting.as_str(),
        )?;
    }

    // The binding records the session the seat lives in: the project's Herdr
    // session (design/03).
    let project = tx.get_project(&seat.project)?.ok_or_else(|| {
        GearError::new(ErrorCode::NoTarget, format!("no project {}", seat.project))
    })?;

    // The old generation is voided with the seat, so a late write from it is
    // rejected (design/03).
    for old_generation in 1..=seat.generation {
        if let Some(binding) = tx.get_binding(seat_id, old_generation)? {
            if binding.status != BindingStatus::Void {
                tx.set_binding_status(seat_id, old_generation, BindingStatus::Void)?;
            }
        }
    }

    let generation = seat.generation + 1;
    tx.set_seat_generation(seat_id, generation)?;
    tx.set_seat_state(seat_id, SeatState::Starting, Some("onboarding"), now)?;
    tx.insert_binding(&BindingRow {
        seat: seat_id.to_string(),
        generation,
        session: project.herdr_session,
        pane_id: pane_id.to_string(),
        kind: kind.to_string(),
        session_ref: None,
        status: BindingStatus::Pending,
        handover_id: None,
        created: now,
    })?;
    Ok(generation)
}

/// The engine saw the expected agent in the bound pane: the Pending binding of
/// `generation` becomes Active (with `session_ref` when Herdr reported one) and
/// the seat goes Starting → Active. A generation that is not the seat's current
/// one → `StaleGeneration`; a binding that is not Pending → `BadTransition`.
pub fn finish_onboard(tx: &Tx<'_>, seat_id: &str, generation: i64, session_ref: Option<&str>, now: Ts) -> GearResult<()> {
    let seat = tx.get_seat(seat_id)?.ok_or_else(|| {
        GearError::new(ErrorCode::NoTarget, format!("no seat {seat_id}"))
    })?;

    // Only the seat's current generation may finish: a call from an older
    // session is stale (design/03).
    if generation != seat.generation {
        return Err(GearError::new(
            ErrorCode::StaleGeneration,
            format!(
                "seat {seat_id} is at generation {}, not {generation}",
                seat.generation
            ),
        ));
    }

    let binding = tx.get_binding(seat_id, generation)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no binding of seat {seat_id} generation {generation}"),
        )
    })?;
    if binding.status != BindingStatus::Pending {
        return Err(GearError::new(
            ErrorCode::BadTransition,
            format!(
                "binding of seat {seat_id} generation {generation} is {}, not pending",
                binding_status_name(binding.status)
            ),
        ));
    }
    require_transition(
        SeatState::can_transition(seat.role, seat.state, SeatState::Active),
        &format!("seat {seat_id}"),
        seat.state.as_str(),
        SeatState::Active.as_str(),
    )?;

    // The binding becomes the seat's live one; Herdr's session ref says which
    // agent session it is (design/03).
    tx.set_binding_status(seat_id, generation, BindingStatus::Active)?;
    if let Some(session_ref) = session_ref {
        tx.set_binding_session_ref(seat_id, generation, session_ref)?;
    }
    tx.set_seat_state(seat_id, SeatState::Active, Some("onboarded"), now)?;
    Ok(())
}

/// The binding status as it is stored (the store keeps it as a string).
fn binding_status_name(status: BindingStatus) -> &'static str {
    match status {
        BindingStatus::Pending => "pending",
        BindingStatus::Active => "active",
        BindingStatus::Void => "void",
    }
}
