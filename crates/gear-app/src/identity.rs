//! Caller identity (design/03).
//!
//! Two phases, never interleaved:
//! 1. [`observe`] — outside any transaction: read-only Herdr `agent_get` on the
//!    caller's pane to capture the occupant fingerprint.
//! 2. [`require_seat`] — inside the write transaction: re-read seat + binding and
//!    check generation, binding status, pane and fingerprint.

use crate::Env;
use gear_herdr::{AgentInfo, HerdrRead, PaneProcessInfo};
use gear_model::{
    ErrorCode, GearCtx, GearError, GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts,
};
use gear_store::{BindingStatus, NewMessage, SeatRow, Store, Tx};

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
    // T-104: GEAR_CTX and HERDR_PANE_ID are environment variables, and any
    // agent can set them in its own shell. The seat is the pane this process
    // really runs in; the pane's own processes are the fact Herdr has about it
    // and a shell cannot forge.
    let processes = herdr.pane_process_info(pane).map_err(|e| {
        GearError::new(
            ErrorCode::NotRegistered,
            format!("cannot read the processes of pane {pane}: {}", e.0),
        )
    })?;
    if !claims_pane(&processes) {
        let real = pane_of_this_process(herdr);
        let where_ = match &real {
            Some(real) if real != pane => format!("this process runs in pane {real}"),
            Some(_) => "the pane does not list this process".to_string(),
            None => "no pane of this session runs this process".to_string(),
        };
        return Err(GearError::new(
            ErrorCode::NotRegistered,
            format!(
                "pane {pane} is claimed as a seat, but {where_}: GEAR_CTX and HERDR_PANE_ID can be set by any shell; the pane's processes cannot"
            ),
        ));
    }
    Ok(occupant.as_ref().map(fingerprint_of))
}

/// Phase 2 (inside tx). Errors:
/// - no GEAR_CTX → NotRegistered (never falls back to Admin);
/// - seat missing → NotRegistered;
/// - ctx.generation != seat.generation or binding not Active → StaleGeneration;
/// - the pane hosts no agent, or the pane or agent kind differs from the
///   binding → NotRegistered (a shell left in the pane is not the seat);
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
    // A seat pane must host its agent: the shell left behind after the agent
    // exited still carries GEAR_CTX, but it is not the seat (design/03).
    let fp = fp.ok_or_else(|| {
        GearError::new(
            ErrorCode::NotRegistered,
            format!("the pane of {seat_id} hosts no agent, so the caller cannot be the seat"),
        )
    })?;
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

/// The process ids this caller runs under, nearest first: its own pid, then
/// its parents (T-104). `ps` is in every POSIX userland; the walk stops at the
/// first pid it cannot read, at init, or when it loops.
pub fn process_ancestors() -> Vec<u32> {
    let mut chain = vec![std::process::id()];
    let mut pid = std::process::id();
    for _ in 0..64 {
        let Ok(output) = std::process::Command::new("ps")
            .args(["-o", "ppid=", "-p", &pid.to_string()])
            .output()
        else {
            break;
        };
        if !output.status.success() {
            break;
        }
        let Ok(parent) = String::from_utf8_lossy(&output.stdout).trim().parse::<u32>() else {
            break;
        };
        if parent == 0 || chain.contains(&parent) {
            break;
        }
        chain.push(parent);
        if parent == 1 {
            break;
        }
        pid = parent;
    }
    chain
}

/// This process's controlling terminal as `/dev/<tty>`, when it has one
/// (T-104).
fn current_tty() -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "tty=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let tty = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let tty = tty.trim_start_matches("/dev/").to_string();
    if tty.is_empty() || tty == "?" || tty == "??" {
        None
    } else {
        Some(format!("/dev/{tty}"))
    }
}

/// Whether the caller really runs in `info`'s pane (T-104): the pane's shell
/// or one of its foreground processes is an ancestor of this process, or the
/// pane's terminal is this process's controlling terminal. Both are Herdr's
/// facts; neither can be set with an environment variable.
pub fn claims_pane(info: &PaneProcessInfo) -> bool {
    let ancestors = process_ancestors();
    let pane_pids = std::iter::once(info.shell_pid)
        .flatten()
        .chain(
            info.foreground_processes
                .iter()
                .map(|process| Some(process.pid))
                .flatten(),
        );
    if pane_pids.into_iter().any(|pid| ancestors.contains(&pid)) {
        return true;
    }
    match (info.tty.as_deref(), current_tty()) {
        (Some(pane_tty), Some(our_tty)) => {
            let pane_tty = pane_tty.trim_start_matches("/dev/");
            let our_tty = our_tty.trim_start_matches("/dev/");
            !pane_tty.is_empty() && pane_tty == our_tty
        }
        _ => false,
    }
}

/// The pane of this session whose processes are this caller's own (T-104):
/// where a caller that claims another pane really runs.
pub fn pane_of_this_process(herdr: &mut dyn HerdrRead) -> Option<String> {
    let panes: Vec<String> = herdr
        .agents()
        .ok()?
        .into_iter()
        .map(|agent| agent.pane_id)
        .collect();
    for pane in panes {
        if let Ok(info) = herdr.pane_process_info(&pane) {
            if claims_pane(&info) {
                return Some(pane);
            }
        }
    }
    None
}

/// Tell the captain that a caller in `real_pane` claimed `seat` of its project
/// (T-104): one urgent item per real pane while it is open, so a repeat claim
/// does not pile up. Best effort by design — a refusal is still the answer when
/// the item cannot be written.
pub fn report_claim(
    store: &mut Store,
    env: &Env,
    herdr: &mut dyn HerdrRead,
    error: &GearError,
    now: Ts,
) -> GearResult<bool> {
    let Some(ctx) = env.ctx.as_ref() else {
        return Ok(false);
    };
    let real = pane_of_this_process(herdr);
    let claimed = env.pane_id.clone().unwrap_or_else(|| "no pane".to_string());
    let real_label = real.clone().unwrap_or_else(|| "a pane this session does not know".to_string());
    store.write(|tx| {
        let Some(captain) = tx.captain_of(&ctx.project)? else {
            return Ok(false);
        };
        let thread = format!("identity:{}", real.as_deref().unwrap_or("unknown"));
        let open = tx
            .thread_messages(&ctx.project, &thread)?
            .into_iter()
            .any(|message| matches!(message.state, MsgState::Open | MsgState::Waiting));
        if open {
            return Ok(false);
        }
        tx.insert_message(&NewMessage {
            project: ctx.project.clone(),
            thread,
            task: None,
            attempt: None,
            sender: "engine".to_string(),
            sender_gen: 0,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent: true,
            body: format!(
                "an identity claim was refused: seat {} was claimed with the pane {claimed}, but the caller runs in {real_label}. A process may act only as the seat of the pane it runs in; ask the agent in {real_label} what it is doing — it may not set another seat's GEAR_CTX. ({})",
                ctx.seat_id(),
                error.detail
            ),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        Ok(true)
    })
}
