//! Caller identity (design/03).
//!
//! Two phases, never interleaved:
//! 1. [`observe`] — outside any transaction: read-only Herdr `agent_get` on the
//!    caller's pane to capture the occupant fingerprint.
//! 2. [`require_seat`] — inside the write transaction: re-read seat + binding and
//!    check generation, binding status, pane and fingerprint.

use crate::Env;
use gear_herdr::{AgentInfo, HerdrRead};
use gear_model::{ErrorCode, GearCtx, GearError, GearResult, SeatRole, SeatState};
use gear_store::{BindingStatus, SeatRow, Tx};

/// Occupant of the caller's pane as seen by Herdr just before the transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub pane_id: String,
    /// Herdr agent kind, None if the pane hosts no recognized agent.
    pub kind: Option<String>,
    pub session_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// A GEAR-onboarded agent whose context checked out.
    Seat { ctx: GearCtx, seat: SeatRow },
    /// The user's own shell with GEAR_ADMIN=1 and no GEAR_CTX.
    Admin,
    /// Anything else: read-only `gear --all` at most.
    Anonymous,
}

/// Phase 1 (no transaction). Returns None when there is no GEAR_CTX or no pane.
/// Herdr failure is returned as an error: identity cannot be confirmed.
pub fn observe(env: &Env, herdr: &mut dyn HerdrRead) -> GearResult<Option<Fingerprint>> {
    if env.ctx.is_none() {
        return Ok(None);
    }
    let pane = match &env.pane_id {
        Some(pane) => pane,
        None => return Ok(None),
    };
    let occupant = herdr.agent_get(pane).map_err(|e| {
        GearError::new(
            ErrorCode::NotRegistered,
            format!("cannot read the occupant of pane {pane}: {}", e.0),
        )
    })?;
    Ok(occupant.as_ref().map(fingerprint_of))
}

/// Phase 2 (inside tx). Errors:
/// - no GEAR_CTX → NotRegistered (never falls back to Admin);
/// - seat missing → NotRegistered;
/// - ctx.generation != seat.generation or binding not Active → StaleGeneration;
/// - pane or agent kind differs from the binding → NotRegistered;
/// - seat role/state not in `roles`/`states` (when given) → Forbidden.
pub fn require_seat(
    tx: &Tx<'_>,
    env: &Env,
    fp: Option<&Fingerprint>,
    roles: Option<&[SeatRole]>,
    states: Option<&[SeatState]>,
) -> GearResult<(GearCtx, SeatRow)> {
    // No GEAR_CTX: no role at all. Never the user.
    let ctx = env.ctx.as_ref().ok_or_else(|| {
        GearError::new(ErrorCode::NotRegistered, "no GEAR_CTX in the environment")
    })?;
    let seat_id = ctx.seat_id();
    let seat = tx.get_seat(&seat_id)?.ok_or_else(|| {
        GearError::new(ErrorCode::NotRegistered, format!("seat {seat_id} is not registered"))
    })?;
    // The generation is fixed at onboarding: an old session may never write.
    if seat.generation != ctx.generation {
        return Err(GearError::new(
            ErrorCode::StaleGeneration,
            format!(
                "seat {seat_id} is at generation {}, the caller carries {}",
                seat.generation, ctx.generation
            ),
        ));
    }
    let binding = tx.get_binding(&seat_id, ctx.generation)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::StaleGeneration,
            format!("seat {seat_id} has no binding for generation {}", ctx.generation),
        )
    })?;
    if binding.status != BindingStatus::Active {
        return Err(GearError::new(
            ErrorCode::StaleGeneration,
            format!(
                "the binding of {seat_id} at generation {} is not active",
                ctx.generation
            ),
        ));
    }
    // The pane is only a consistency check, never the source of identity.
    if let Some(pane) = &env.pane_id {
        if pane != &binding.pane_id {
            return Err(GearError::new(
                ErrorCode::NotRegistered,
                format!(
                    "the caller runs in pane {pane}, the binding of {seat_id} is in pane {}",
                    binding.pane_id
                ),
            ));
        }
    }
    if let Some(fp) = fp {
        if fp.pane_id != binding.pane_id {
            return Err(GearError::new(
                ErrorCode::NotRegistered,
                format!(
                    "the occupant is in pane {}, the binding of {seat_id} is in pane {}",
                    fp.pane_id, binding.pane_id
                ),
            ));
        }
        if let Some(kind) = &fp.kind {
            if kind != &binding.kind {
                return Err(GearError::new(
                    ErrorCode::NotRegistered,
                    format!(
                        "pane {} hosts {kind}, the binding of {seat_id} expects {}",
                        fp.pane_id, binding.kind
                    ),
                ));
            }
        }
    }
    if let Some(roles) = roles {
        if !roles.contains(&seat.role) {
            return Err(GearError::new(
                ErrorCode::Forbidden,
                format!(
                    "seat {seat_id} has role {}, only {} may do this",
                    seat.role.as_str(),
                    role_list(roles)
                ),
            ));
        }
    }
    if let Some(states) = states {
        if !states.contains(&seat.state) {
            return Err(GearError::new(
                ErrorCode::Forbidden,
                format!(
                    "seat {seat_id} is {}, only {} may do this",
                    seat.state.as_str(),
                    state_list(states)
                ),
            ));
        }
    }
    Ok((ctx.clone(), seat))
}

/// Classify without failing (for read-only paths like `gear --all`).
pub fn classify(tx: &Tx<'_>, env: &Env, fp: Option<&Fingerprint>) -> GearResult<Caller> {
    if env.ctx.is_none() {
        return Ok(if env.admin { Caller::Admin } else { Caller::Anonymous });
    }
    match require_seat(tx, env, fp, None, None) {
        Ok((ctx, seat)) => Ok(Caller::Seat { ctx, seat }),
        // A caller that cannot prove its identity gets no role: never the user.
        Err(_) => Ok(Caller::Anonymous),
    }
}

/// Convert a Herdr agent row into a fingerprint.
pub fn fingerprint_of(info: &AgentInfo) -> Fingerprint {
    Fingerprint {
        pane_id: info.pane_id.clone(),
        kind: info.agent.clone(),
        session_ref: info.agent_session.as_ref().map(|s| s.value.clone()),
    }
}

/// Roles as a readable list, for error details.
fn role_list(roles: &[SeatRole]) -> String {
    roles.iter().map(|r| r.as_str()).collect::<Vec<_>>().join(", ")
}

/// States as a readable list, for error details.
fn state_list(states: &[SeatState]) -> String {
    states.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
}
