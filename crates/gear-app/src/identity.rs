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
pub fn observe(
    env: &Env,
    herdr: &mut dyn HerdrRead,
    ancestors: &[u32],
) -> GearResult<Option<Fingerprint>> {
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
    // really runs in, found by matching the caller's process ancestry (OS
    // parent pids) against each pane's shell and foreground pids — the fact
    // Herdr has about the pane and a shell cannot forge.
    let processes = herdr.pane_process_info(pane).map_err(|e| {
        GearError::new(
            ErrorCode::NotRegistered,
            format!("cannot read the processes of pane {pane}: {}", e.0),
        )
    })?;
    if !claims_pane(&processes, ancestors) {
        let real = real_pane(herdr, ancestors);
        // A daemon CLI runs its tools in a shared service (`opencode serve`,
        // `codex app-server`), not under the agent's pane, so their ancestry
        // reaches no pane at all. Such a caller keeps the env claim for a pane
        // of a daemon kind, still checked against the seat's binding; a caller
        // that runs in another pane, or that claims a pane whose agent runs
        // its tools in the pane, is refused (T-104, T-126).
        let tools_outside_pane = occupant
            .as_ref()
            .and_then(|info| info.agent.as_deref())
            .is_some_and(is_daemon_kind);
        if real.is_none() && tools_outside_pane {
            return Ok(occupant.as_ref().map(fingerprint_of));
        }
        let where_ = match real {
            Some(real) if real.as_str() != pane.as_str() => format!("this process runs in pane {real}"),
            Some(_) => "the pane does not list this process".to_string(),
            None => "no pane of this session runs this process".to_string(),
        };
        return Err(GearError::new(
            ErrorCode::NotRegistered,
            format!(
                "pane {pane} is claimed as a seat, but {where_}: GEAR_CTX and HERDR_PANE_ID can be set by any shell; the caller's ancestry and the pane's processes cannot"
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
/// its parents (T-104). The parent chain is the OS's own answer — no
/// environment variable can change it, and no `$PATH` either: a caller can put
/// an executable `ps` in front of the real one, so the walk only asks `/proc`
/// (Linux) or the system `ps` by **absolute path**.
pub fn process_ancestors() -> Vec<u32> {
    let mut chain = vec![std::process::id()];
    let mut pid = std::process::id();
    for _ in 0..64 {
        let Some(parent) = parent_of(pid) else {
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

/// The parent of `pid`, from the OS itself (T-104): `/proc/<pid>/stat` where the
/// platform has it, else the system `ps` at an absolute path. Never `$PATH`: a
/// shell that prepends a directory could print another pane's pid and fake a
/// whole ancestry.
fn parent_of(pid: u32) -> Option<u32> {
    #[cfg(target_os = "linux")]
    if let Some(parent) = parent_from_proc(pid) {
        return Some(parent);
    }
    for ps in ["/bin/ps", "/usr/bin/ps"] {
        let Ok(output) = std::process::Command::new(ps)
            .args(["-o", "ppid=", "-p", &pid.to_string()])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        if let Ok(parent) = String::from_utf8_lossy(&output.stdout).trim().parse::<u32>() {
            return Some(parent);
        }
    }
    None
}

/// The parent pid from `/proc/<pid>/stat` (Linux): the third field is the
/// parent, after the state that follows the parenthesised command name (which
/// may itself contain spaces and parentheses).
#[cfg(target_os = "linux")]
fn parent_from_proc(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Whether the caller runs in `info`'s pane (T-104): the pane's shell or one of
/// its foreground processes is one of the caller's ancestors. Both sides are
/// OS facts — no environment variable can put a pid in a parent chain.
pub fn claims_pane(info: &PaneProcessInfo, ancestors: &[u32]) -> bool {
    let pane_pids = std::iter::once(info.shell_pid)
        .flatten()
        .chain(
            info.foreground_processes
                .iter()
                .filter_map(|process| Some(process.pid)),
        );
    pane_pids.into_iter().any(|pid| ancestors.contains(&pid))
}

/// The pane this session runs the caller in (T-104): every pane of the session
/// (agents and the user's own shells) is asked whether its shell or a
/// foreground process is one of the caller's ancestors.
pub fn real_pane(herdr: &mut dyn HerdrRead, ancestors: &[u32]) -> Option<String> {
    let panes = herdr.panes().ok()?;
    for pane in panes {
        if let Ok(info) = herdr.pane_process_info(&pane) {
            if claims_pane(&info, ancestors) {
                return Some(pane);
            }
        }
    }
    None
}

/// CLIs whose **tools do not run in the pane**: the TUI in the pane is a client
/// and the work happens in a shared daemon (`opencode serve`, `codex
/// app-server`), so the caller's ancestry reaches no pane at all (T-104,
/// T-126). One list: the claim rule, the pane resolution and the skill all read
/// it from here.
pub fn is_daemon_kind(kind: &str) -> bool {
    matches!(kind, "opencode" | "codex")
}

/// The pane this process runs in, without trusting a variable (T-126): its own
/// pane by process ancestry first; `HERDR_PANE_ID` only when the ancestry finds
/// none (a daemon CLI's tools, whose parent is the daemon).
pub fn caller_pane(
    herdr: &mut dyn HerdrRead,
    ancestors: &[u32],
    env_pane: Option<&str>,
) -> Option<String> {
    real_pane(herdr, ancestors).or_else(|| env_pane.map(str::to_string))
}

/// The seat whose **active** binding is on `pane` (T-126): the pane a caller
/// runs in, and the store's answer to which seat that is. `None` when no seat
/// is bound there (an unmanaged agent, the user's own shell).
pub fn seat_bound_to(tx: &Tx<'_>, pane: &str) -> GearResult<Option<SeatRow>> {
    for project in tx.project_ids()? {
        for seat in tx.seats_of_project(&project)? {
            let Some(binding) = tx.get_binding(&seat.id, seat.generation)? else {
                continue;
            };
            if binding.status == BindingStatus::Active && binding.pane_id == pane {
                return Ok(Some(seat));
            }
        }
    }
    Ok(None)
}

/// The **one** pane of the session that hosts an agent of a daemon kind and is
/// (`bound`) or is not (`!bound`) bound to a seat (T-126). More than one such
/// pane is no answer: the caller has to name itself then.
pub fn daemon_pane(
    store: &mut Store,
    herdr: &mut dyn HerdrRead,
    bound: bool,
) -> GearResult<Option<String>> {
    let panes = herdr.panes().unwrap_or_default();
    let mut found = None;
    for pane in panes {
        let Ok(Some(info)) = herdr.agent_get(&pane) else {
            continue;
        };
        if !info.agent.as_deref().is_some_and(is_daemon_kind) {
            continue;
        }
        let is_bound = store.read(|tx| seat_bound_to(tx, &pane))?.is_some();
        if is_bound != bound {
            continue;
        }
        if found.is_some() {
            return Ok(None);
        }
        found = Some(pane);
    }
    Ok(found)
}

/// Whether any pane of the session hosts an agent at all: the difference
/// between "a shell outside Herdr" (anonymous) and "an agent whose tools run
/// in a daemon, so in no pane" (a caller that needs `gear captain take`).
fn any_agent_pane(herdr: &mut dyn HerdrRead) -> bool {
    herdr
        .panes()
        .unwrap_or_default()
        .iter()
        .any(|pane| matches!(herdr.agent_get(pane), Ok(Some(_))))
}

/// The caller's environment, resolved without `GEAR_CTX` (T-126).
///
/// `GEAR_CTX` stays an **override**: when it is set, nothing here runs. When it
/// is not, the caller's pane is its real pane (ancestry), else `HERDR_PANE_ID`,
/// else — a caller in no pane, whose tools run in a daemon — the one pane of the
/// session whose agent is of a daemon kind and is bound to a seat. The seat
/// whose active binding is on that pane gives the project, the seat and the
/// generation: the same [`GearCtx`] the environment would have carried.
///
/// A pane without a seat leaves the caller anonymous (the user's own shell). A
/// caller in no pane at all, in a session that runs agent panes, gets the error
/// that names the one command that fixes it.
pub fn resolve_env(
    store: &mut Store,
    env: &Env,
    herdr: &mut dyn HerdrRead,
    ancestors: &[u32],
) -> GearResult<Env> {
    if env.ctx.is_some() {
        return Ok(env.clone());
    }
    let pane = caller_pane(herdr, ancestors, env.pane_id.as_deref())
        .or(daemon_pane(store, herdr, true)?);
    let Some(pane) = pane else {
        if any_agent_pane(herdr) {
            return Err(GearError::new(
                ErrorCode::NotRegistered,
                "this process runs in no pane of the Herdr session — an agent whose tools run in a shared daemon (codex, opencode) does. To become the project's captain: start your CLI in the project's checkout and run `gear captain take`",
            ));
        }
        return Ok(env.clone());
    };
    let seat = store.read(|tx| seat_bound_to(tx, &pane))?;
    let Some(seat) = seat else {
        // The pane holds no seat: the caller stays anonymous, with its pane.
        return Ok(Env { ctx: None, pane_id: Some(pane), admin: env.admin });
    };
    Ok(Env {
        ctx: Some(GearCtx {
            project: seat.project,
            seat: seat.name,
            generation: seat.generation,
        }),
        pane_id: Some(pane),
        admin: env.admin,
    })
}

/// Observe the caller and prove its whole claim (T-104): the pane it really
/// runs in is the claimed pane **and** the seat it claims is bound to that pane.
/// A claim refused at either step is reported once to the captain, naming the
/// real pane — a correct pane id with someone else's context (the captain's
/// `GEAR_CTX`, the lane's own pane id) included. `Ok(None)` = no context to
/// prove (the user's own shell).
pub fn prove_seat(
    store: &mut Store,
    env: &Env,
    herdr: &mut dyn HerdrRead,
    ancestors: &[u32],
    now: Ts,
) -> GearResult<Option<Fingerprint>> {
    let fp = match observe(env, herdr, ancestors) {
        Ok(fp) => fp,
        Err(error) => {
            if error.code == ErrorCode::NotRegistered {
                let _ = report_claim(store, env, herdr, &error, ancestors, now);
            }
            return Err(error);
        }
    };
    if env.ctx.is_some() {
        if let Some(fp) = fp.as_ref() {
            if let Err(error) = store.read(|tx| require_seat(tx, env, Some(fp), None, None)) {
                if error.code == ErrorCode::NotRegistered {
                    let _ = report_claim(store, env, herdr, &error, ancestors, now);
                }
                return Err(error);
            }
        }
    }
    Ok(fp)
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
    ancestors: &[u32],
    now: Ts,
) -> GearResult<bool> {
    let Some(ctx) = env.ctx.as_ref() else {
        return Ok(false);
    };
    let real = real_pane(herdr, ancestors);
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
