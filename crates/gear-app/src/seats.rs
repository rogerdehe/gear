//! Seat lifecycle around onboarding (design/03, /10). Transaction-level, like
//! `tasks`: the engine calls these inside its own transactions; Herdr is never
//! touched here. Transitions via `SeatState::can_transition` for the seat's
//! role → `BadTransition`; missing seat/binding → `NoTarget`.

use gear_model::state::require_transition;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, Ts};
use gear_store::{BindingRow, BindingStatus, MessageRow, SeatRow, Tx};

/// Start onboarding: the seat (Queued, or Missing/NeedsCheck for a restart)
/// moves to Starting, its generation becomes `generation + 1`, every earlier
/// binding of the seat that is not Void becomes Void, and a Pending binding
/// (`pane_id`, `kind`, session = the project's Herdr session) is inserted for the
/// new generation. Returns the new generation.
pub fn begin_onboard(
    tx: &Tx<'_>,
    seat_id: &str,
    pane_id: &str,
    kind: &str,
    now: Ts,
) -> GearResult<i64> {
    let seat = tx
        .get_seat(seat_id)?
        .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {seat_id}")))?;

    // A seat starts onboarding from Queued (freshly registered) or from
    // Missing/NeedsCheck (a restart); the model says which is legal for the
    // seat's role.
    require_transition(
        SeatState::can_transition(seat.role, seat.state, SeatState::Starting),
        &format!("seat {seat_id}"),
        seat.state.as_str(),
        SeatState::Starting.as_str(),
    )?;

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
    // T-217: the attempt the seat is working on moves to the new generation
    // with it, so a lane that is restarted (or retried) can still hand the
    // attempt in.
    crate::tasks::carry_attempt_generation(tx, seat_id, seat.generation, generation, now)?;
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
pub fn finish_onboard(
    tx: &Tx<'_>,
    seat_id: &str,
    generation: i64,
    session_ref: Option<&str>,
    now: Ts,
) -> GearResult<()> {
    let seat = tx
        .get_seat(seat_id)?
        .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {seat_id}")))?;

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

/// The moment a seat's current generation started (T-71): the binding of that
/// generation is opened then, in the same transaction that bumps the
/// generation. A warm seat keeps its id across jobs, so this is what tells an
/// earlier job's messages from this job's.
///
/// `None` for a seat without a binding of that generation (never onboarded).
pub fn generation_started(tx: &Tx<'_>, seat_id: &str, generation: i64) -> GearResult<Option<Ts>> {
    Ok(tx
        .get_binding(seat_id, generation)?
        .map(|binding| binding.created))
}

/// The time from which a message belongs to the seat's current generation: one
/// second before the generation started, because the store's message queries
/// take a strict `created > since` and a message written in the same second as
/// the binding is this generation's (T-71).
///
/// `0` for a seat without a binding of that generation: nothing is known
/// about its start, so nothing is hidden.
pub fn generation_since(tx: &Tx<'_>, seat_id: &str, generation: i64) -> GearResult<Ts> {
    Ok(generation_started(tx, seat_id, generation)?.map_or(0, |started| started.saturating_sub(1)))
}

/// Whether a message is the seat's own job (T-71): written after its current
/// generation started, and — for a temp seat — about the task it works on now.
///
/// A warm temp seat keeps its id across jobs, so an answer about the task it
/// reviewed before is not this job's, even when it was written after the new
/// job started (F1 of the T-71 review). The worker seats use this one rule for
/// the doorbell and for the home page alike; the captain's open action items
/// are not limited by it (its inbox is the engine's business, F2).
pub fn message_belongs(seat: &SeatRow, message: &MessageRow, since: Ts) -> bool {
    if message.created <= since {
        return false;
    }
    if seat.role == SeatRole::Temp {
        return message.task.as_deref() == seat.task_id.as_deref();
    }
    true
}
