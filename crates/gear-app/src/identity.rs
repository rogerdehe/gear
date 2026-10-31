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

/// Agents whose tools run in a shared background process outside the pane:
/// `opencode serve`, and Codex's `codex app-server` daemon (trial, 2026-10-26).
/// A caller in no pane keeps its claim on such an agent's pane, still checked
/// against the seat's binding.
const TOOLS_OUTSIDE_PANE: [&str; 2] = ["opencode", "codex"];

/// The sessions the shared daemons give their tool shells (T-328): `opencode
/// serve` exports `OPENCODE_SESSION_ID` (the session id of the tool call, the
/// id GEAR records when it starts the seat through the API), `codex app-server`
/// exports `CODEX_THREAD_ID`. A tool shell in no pane carries the session it
/// works for, so a daemon claim can be checked against the seat's own sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonSessions {
    pub opencode: Option<String>,
    pub codex: Option<String>,
}

impl DaemonSessions {
    /// Read the daemon sessions from the caller's variables (an empty value is
    /// no value).
    pub fn from_vars(vars: impl Fn(&str) -> Option<String>) -> DaemonSessions {
        let read = |name: &str| vars(name).filter(|value| !value.is_empty());
        DaemonSessions {
            opencode: read("OPENCODE_SESSION_ID"),
            codex: read("CODEX_THREAD_ID"),
        }
    }
}

/// Phase 1 (no transaction). Returns None when there is no GEAR_CTX or no pane.
/// Herdr failure is returned as an error: identity cannot be confirmed.
pub fn observe(
    env: &Env,
    herdr: &mut dyn HerdrRead,
    ancestors: &[u32],
) -> GearResult<Option<Fingerprint>> {
    observe_via(env, herdr, ancestors).map(|found| found.map(|(fp, _)| fp))
}

/// [`observe`], also telling whether the caller's tools run in a shared daemon
/// (T-328): the bool is true exactly when the daemon branch returned, so the
/// claim can be checked against the seat's own session.
pub fn observe_via(
    env: &Env,
    herdr: &mut dyn HerdrRead,
    ancestors: &[u32],
) -> GearResult<Option<(Fingerprint, bool)>> {
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
        // OpenCode (`opencode serve`) and Codex in daemon mode (`codex
        // app-server`) run their tools in a shared background process,
        // not under the agent's pane, so its ancestry reaches no pane at all.
        // Such a caller keeps the env claim for an OpenCode pane, still checked
        // against the seat's binding; a caller that runs in another pane, or
        // that claims a pane whose agent runs its tools in the pane, is refused.
        let tools_outside_pane = occupant
            .as_ref()
            .and_then(|info| info.agent.as_deref())
            .is_some_and(is_daemon_kind);
        if real.is_none() && tools_outside_pane {
            return Ok(occupant.as_ref().map(|info| (fingerprint_of(info), true)));
        }
        let where_ = match real {
            Some(real) if real.as_str() != pane.as_str() => format!("this process runs in pane {real}"),
            Some(_) => "the pane does not list this process".to_string(),
            None => "no pane of this session runs this process (its tools run outside every pane, and the pane's agent is not one GEAR knows to do that)".to_string(),
        };
        // T-314: a refusal has to be traceable afterwards — which Herdr door
        // was asked, and the ancestry that was matched against its panes.
        let asked = herdr
            .socket()
            .unwrap_or_else(|| "an unnamed Herdr".to_string());
        let ancestry = ancestors
            .iter()
            .map(|pid| pid.to_string())
            .collect::<Vec<_>>()
            .join(" < ");
        return Err(GearError::new(
            ErrorCode::NotRegistered,
            format!(
                "pane {pane} is claimed as a seat, but {where_}: GEAR_CTX and HERDR_PANE_ID can be set by any shell; the caller's ancestry and the pane's processes cannot (asked Herdr at {asked}; this process's ancestry: {ancestry})"
            ),
        ));
    }
    Ok(occupant.as_ref().map(|info| (fingerprint_of(info), false)))
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
        GearError::new(
            ErrorCode::NotRegistered,
            format!("seat {seat_id} is not registered"),
        )
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
            format!(
                "seat {seat_id} has no binding for generation {}",
                ctx.generation
            ),
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
        return Ok(if env.admin {
            Caller::Admin
        } else {
            Caller::Anonymous
        });
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
    roles
        .iter()
        .map(|r| r.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// States as a readable list, for error details.
fn state_list(states: &[SeatState]) -> String {
    states
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The process ids this caller runs under, nearest first: its own pid, then
/// its parents (T-104). The parent chain is the OS's own answer — no
/// environment variable can change it, and no `$PATH` either: a caller can put
/// an executable `ps` in front of the real one. T-131: no program is started at
/// all — the parent is read in process, `/proc/<pid>/stat` on Linux and
/// `proc_pidinfo(PROC_PIDTBSDINFO)` on macOS — because a slow system left the
/// old `ps` calls stuck, and `gear` hung behind them.
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

/// The parent of `pid`, read in process — no program started (T-131), so a
/// slow system cannot leave anything stuck behind a `gear` call and no
/// `$PATH` can answer for the OS.
fn parent_of(pid: u32) -> Option<u32> {
    #[cfg(target_os = "linux")]
    return parent_from_proc(pid);
    #[cfg(target_os = "macos")]
    return parent_from_pidinfo(pid);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
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

/// The parent pid from `proc_pidinfo(PROC_PIDTBSDINFO)` (macOS): the kernel's own
/// record of the process (`proc_bsdinfo.pbi_ppid`), read in process. The old
/// `/bin/ps` spawn was the T-131 hang: when the system was slow to launch
/// programs, one `ps` per ancestor piled up stuck and `gear` was killed by its
/// callers. A short answer is the pid gone or the buffer too small — no parent.
#[cfg(target_os = "macos")]
fn parent_from_pidinfo(pid: u32) -> Option<u32> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `proc_pidinfo` writes at most `size` bytes into `info`, and the
    // struct is only read when it reports exactly those `size` bytes filled.
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if got != size {
        return None;
    }
    // SAFETY: `got == size`, so every field of the struct was written.
    let info = unsafe { info.assume_init() };
    // A pid can be recycled between the ask and the answer: the record names
    // the pid it describes, and only its own record is this pid's parent.
    if info.pbi_pid != pid {
        return None;
    }
    Some(info.pbi_ppid)
}

/// Whether the caller runs in `info`'s pane (T-104): the pane's shell or one of
/// its foreground processes is one of the caller's ancestors. Both sides are
/// OS facts — no environment variable can put a pid in a parent chain.
pub fn claims_pane(info: &PaneProcessInfo, ancestors: &[u32]) -> bool {
    let pane_pids = std::iter::once(info.shell_pid).flatten().chain(
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

/// Whether `kind`'s **tools do not run in its pane** — the TUI is a client and
/// the work happens in a shared daemon (`opencode serve`, `codex app-server`),
/// so the caller's ancestry reaches no pane at all (T-104). One list
/// ([`TOOLS_OUTSIDE_PANE`]) serves the claim rule and the pane resolution of
/// T-126 together.
pub fn is_daemon_kind(kind: &str) -> bool {
    TOOLS_OUTSIDE_PANE.contains(&kind)
}

/// The pane this process runs in, without trusting a variable (T-126): its own
/// pane by process ancestry first; `HERDR_PANE_ID` only when the ancestry finds
/// none (a daemon CLI's tools, whose parent is the daemon).
pub fn caller_pane(
    herdr: &mut dyn HerdrRead,
    ancestors: &[u32],
    env_pane: Option<&str>,
) -> Option<String> {
    if let Some(pane) = real_pane(herdr, ancestors) {
        return Some(pane);
    }
    // A caller in no pane runs its tools in a daemon, and the daemon's
    // HERDR_PANE_ID is the pane (or even the Herdr session) it was started
    // from, not the caller's: trust it only when that pane hosts a daemon agent.
    env_pane
        .filter(|pane| {
            matches!(herdr.agent_get(pane), Ok(Some(info)) if info.agent.as_deref().is_some_and(is_daemon_kind))
        })
        .map(str::to_string)
}

/// The seat whose **active** binding is on `pane` (T-126): the pane a caller
/// runs in, and the store's answer to which seat that is. `None` when no seat
/// is bound there (an unmanaged agent, the user's own shell).
pub fn seat_bound_to(tx: &Tx<'_>, pane: &str) -> GearResult<Option<SeatRow>> {
    // A pane can carry an old binding of another project (a fixture whose
    // captain once ran here): the newest active binding is the pane's seat.
    let mut best: Option<(i64, SeatRow)> = None;
    for project in tx.project_ids()? {
        for seat in tx.seats_of_project(&project)? {
            let Some(binding) = tx.get_binding(&seat.id, seat.generation)? else {
                continue;
            };
            if binding.status == BindingStatus::Active
                && binding.pane_id == pane
                && best
                    .as_ref()
                    .is_none_or(|(created, _)| binding.created > *created)
            {
                best = Some((binding.created, seat));
            }
        }
    }
    Ok(best.map(|(_, seat)| seat))
}

/// Every seat with an **active** binding on `pane` (T-485), in project order:
/// one pane may hold several projects' captain seats.
pub fn seats_bound_to(tx: &Tx<'_>, pane: &str) -> GearResult<Vec<SeatRow>> {
    let mut seats = Vec::new();
    for project in tx.project_ids()? {
        for seat in tx.seats_of_project(&project)? {
            let Some(binding) = tx.get_binding(&seat.id, seat.generation)? else {
                continue;
            };
            if binding.status == BindingStatus::Active && binding.pane_id == pane {
                seats.push(seat);
            }
        }
    }
    Ok(seats)
}

/// The captain seats of the projects whose active binding is on `pane`
/// (T-485): what the pane answers as the captain of.
pub fn captains_on_pane(tx: &Tx<'_>, pane: &str) -> GearResult<Vec<SeatRow>> {
    Ok(seats_bound_to(tx, pane)?
        .into_iter()
        .filter(|seat| seat.role == SeatRole::Captain)
        .collect())
}

/// The env a captain command acts with for `project` (T-485): the caller's own
/// context when it names `project`; else the captain seat of `project` whose
/// active binding is on the caller's pane, at that seat's own generation. The
/// command then checks the claim with [`require_seat`], so a stale generation
/// or a wrong pane still fails.
pub fn env_of_captain(
    tx: &Tx<'_>,
    env: &Env,
    fp: Option<&Fingerprint>,
    project: &str,
) -> GearResult<Env> {
    if env.ctx.as_ref().is_some_and(|ctx| ctx.project == project) {
        return Ok(env.clone());
    }
    let pane = fp
        .map(|fp| fp.pane_id.clone())
        .or_else(|| env.pane_id.clone())
        .ok_or_else(|| {
            GearError::new(
                ErrorCode::NotRegistered,
                format!("no pane is known for the captain of {project}"),
            )
        })?;
    let held = captains_on_pane(tx, &pane)?;
    let seat = held
        .iter()
        .find(|seat| seat.project == project)
        .ok_or_else(|| {
            let names: Vec<String> = held.iter().map(|seat| seat.project.clone()).collect();
            GearError::new(
                ErrorCode::Forbidden,
                if names.is_empty() {
                    format!("pane {pane} holds no captain seat, so it cannot act for {project}")
                } else {
                    format!(
                        "pane {pane} holds the captain of {}, not of {project}",
                        names.join(", ")
                    )
                },
            )
        })?;
    Ok(Env {
        ctx: Some(GearCtx {
            project: seat.project.clone(),
            seat: seat.name.clone(),
            generation: seat.generation,
        }),
        pane_id: Some(pane),
        admin: env.admin,
    })
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
    let pane =
        caller_pane(herdr, ancestors, env.pane_id.as_deref()).or(daemon_pane(store, herdr, true)?);
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
        return Ok(Env {
            ctx: None,
            pane_id: Some(pane),
            admin: env.admin,
        });
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
    daemon: &DaemonSessions,
    now: Ts,
) -> GearResult<Option<Fingerprint>> {
    let (fp, from_daemon) = match observe_via(env, herdr, ancestors) {
        Ok(Some((fp, from_daemon))) => (Some(fp), from_daemon),
        Ok(None) => (None, false),
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
            // T-328: a caller whose tools run in a shared daemon has no pane to
            // prove it by — it proves it by the session its tool shell carries.
            if from_daemon {
                let notice = match store.read(|tx| prove_daemon_session(tx, env, fp, daemon)) {
                    Ok(notice) => notice,
                    Err(error) => {
                        if error.code == ErrorCode::NotRegistered {
                            let _ = report_claim(store, env, herdr, &error, ancestors, now);
                        }
                        return Err(error);
                    }
                };
                if let Some((thread, known)) = notice {
                    if let Some(ctx) = env.ctx.as_ref() {
                        let _ = report_codex_thread(store, ctx, &thread, &known, now);
                    }
                }
            }
        }
    }
    Ok(fp)
}

/// The daemon half of the claim (T-328): a caller whose tools run in a shared
/// daemon proves it is the seat's **own** session, not just any tool shell of
/// the same daemon. The seat's known sessions are its binding's `session_ref`,
/// the fingerprint's `session_ref` and the sessions of its open round, empty
/// values dropped; a seat with none has nothing to prove against, as before.
/// An OpenCode tool shell always carries `OPENCODE_SESSION_ID`, so its absence
/// or a foreign id is a refusal. A Codex thread that is not a known session
/// passes — that the thread id equals the session GEAR records is not yet seen
/// live — and owes one notice: `Ok(Some((thread, known)))`.
fn prove_daemon_session(
    tx: &Tx<'_>,
    env: &Env,
    fp: &Fingerprint,
    daemon: &DaemonSessions,
) -> GearResult<Option<(String, String)>> {
    let ctx = env
        .ctx
        .as_ref()
        .expect("a claim: no context, no daemon proof");
    let seat = ctx.seat_id();
    let binding = tx.get_binding(&seat, ctx.generation)?;
    let kind = binding
        .as_ref()
        .map(|binding| binding.kind.clone())
        .or_else(|| fp.kind.clone())
        .unwrap_or_default();
    let round = tx.open_round_of_seat(&seat)?;
    let mut known: Vec<String> = Vec::new();
    let sessions = binding
        .as_ref()
        .and_then(|binding| binding.session_ref.as_deref())
        .into_iter()
        .chain(fp.session_ref.as_deref())
        .chain(
            round
                .iter()
                .flat_map(|round| round.sessions.iter().map(String::as_str)),
        );
    for session in sessions {
        let session = session.trim();
        if !session.is_empty() && !known.iter().any(|known| known.as_str() == session) {
            known.push(session.to_string());
        }
    }
    if known.is_empty() {
        return Ok(None);
    }
    let listed = known.join(", ");
    let mine = |session: &str| known.iter().any(|known| known.as_str() == session);
    match kind.as_str() {
        "opencode" => match daemon.opencode.as_deref() {
            None => Err(GearError::new(
                ErrorCode::NotRegistered,
                format!(
                    "seat {seat} is claimed from outside every pane without OPENCODE_SESSION_ID: an OpenCode tool shell always carries it, so this caller is not the seat's OpenCode session"
                ),
            )),
            Some(session) if mine(session) => Ok(None),
            Some(session) => Err(GearError::new(
                ErrorCode::NotRegistered,
                format!(
                    "seat {seat} is claimed by OpenCode session {session}, but the seat's sessions are {listed}: a caller may act only as the seat of its own session"
                ),
            )),
        },
        "codex" => match daemon.codex.as_deref() {
            Some(thread) if !mine(thread) => Ok(Some((thread.to_string(), listed))),
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}

/// Tell the captain that a Codex thread which is not one of the seat's sessions
/// used the seat (T-328): one item per seat and generation, so a repeat does not
/// pile up. Best effort — the claim still passes.
fn report_codex_thread(
    store: &mut Store,
    ctx: &GearCtx,
    thread_id: &str,
    known: &str,
    now: Ts,
) -> GearResult<bool> {
    let seat = ctx.seat_id();
    store.write(|tx| {
        let Some(captain) = tx.captain_of(&ctx.project)? else {
            return Ok(false);
        };
        let thread = format!("identity-codex:{seat}:{}", ctx.generation);
        if !tx.thread_messages(&ctx.project, &thread)?.is_empty() {
            return Ok(false);
        }
        tx.insert_message(&NewMessage {
            meta: None,
            project: ctx.project.clone(),
            thread,
            task: None,
            attempt: None,
            sender: "engine".to_string(),
            sender_gen: 0,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent: false,
            body: format!(
                "seat {seat} was used by Codex thread {thread_id}, while the seat's sessions are {known}: if this repeats, a Codex agent may be acting as another seat"
            ),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Info,
            created: now,
        })?;
        Ok(true)
    })
}

/// What `gear whoami` reports (design §10.7, row B9): the real pane, the seat
/// bound to it, the store the caller would write to, and why a claim fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhoAmI {
    /// The pane the caller really runs in (its own pane by process ancestry).
    pub pane: Option<String>,
    /// The seat bound to that pane.
    pub seat: Option<String>,
    /// The store the caller would write to.
    pub store: String,
    /// Why the caller cannot act as a seat; `None` when it can.
    pub reason: Option<String>,
}

impl std::fmt::Display for WhoAmI {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "pane: {}", self.pane.as_deref().unwrap_or("unknown"))?;
        writeln!(f, "seat: {}", self.seat.as_deref().unwrap_or("none"))?;
        writeln!(f, "store: {}", self.store)?;
        if let Some(reason) = &self.reason {
            writeln!(f, "reason: {reason}")?;
        }
        Ok(())
    }
}

/// `gear whoami` (design §10.7, row B9): the real pane, the seat bound to it,
/// the store, and why a claim fails.
///
/// It never fails: the answer to "who am I" is a report, not a refusal. The
/// pane comes from the process ancestry (T-104) and the seat from the store's
/// bindings (T-126), so an agent that claims a seat it does not own sees why.
///
/// The reason is the one the commands themselves give: the effective
/// environment is resolved the way every command resolves it (a caller
/// without `GEAR_CTX` that runs in a bound pane **is** the seat of that pane,
/// T-126) and the claim is then checked read-only — the observation and the
/// seat check. `prove_seat` is never used: it writes a claim report, and
/// `whoami` only reports.
pub fn whoami(
    store: &mut Store,
    env: &Env,
    herdr: &mut dyn HerdrRead,
    ancestors: &[i32],
) -> WhoAmI {
    // The pids of the caller's ancestors (the pane's own types are unsigned).
    let ancestors: Vec<u32> = ancestors
        .iter()
        .filter(|pid| **pid >= 0)
        .map(|pid| *pid as u32)
        .collect();
    let store_path = std::env::var(gear_model::ctx::ENV_DB).unwrap_or_default();
    // The effective environment: the same resolution every command does, so
    // whoami explains the commands (a caller in a bound pane without a
    // context is the seat of that pane).
    let effective = match resolve_env(store, env, herdr, &ancestors) {
        Ok(env) => env,
        Err(error) => {
            let pane = caller_pane(herdr, &ancestors, env.pane_id.as_deref());
            return WhoAmI {
                pane,
                seat: None,
                store: store_path,
                reason: Some(error.detail),
            };
        }
    };
    let pane = caller_pane(herdr, &ancestors, effective.pane_id.as_deref());
    let seat = pane.as_deref().and_then(|pane| {
        store
            .read(|tx| seat_bound_to(tx, pane))
            .ok()
            .flatten()
            .map(|seat| seat.id)
    });
    // The claim, checked the way every command checks it and read-only: the
    // observation of the pane, then the seat check (the generation, the
    // binding, the pane, the kind).
    let reason = match observe(&effective, herdr, &ancestors) {
        Err(error) => Some(error.detail),
        Ok(fingerprint) => match effective.ctx.as_ref() {
            // No seat at all: the user’s own shell, or a pane that hosts no
            // seat of this project.
            None => Some(match pane.as_deref() {
                Some(pane) => {
                    format!("no seat is bound to the pane the caller runs in ({pane})")
                }
                None => "no GEAR_CTX in the environment".to_string(),
            }),
            Some(_) if seat.is_none() => Some(match pane.as_deref() {
                Some(pane) => {
                    format!("no seat is bound to the pane the caller runs in ({pane})")
                }
                None => "the seat of the caller is not bound to a pane".to_string(),
            }),
            Some(ctx) => match store
                .read(|tx| require_seat(tx, &effective, fingerprint.as_ref(), None, None))
            {
                Ok(_) => None,
                Err(error) => {
                    // The claim failed: name the seat the pane really has, so
                    // the caller sees both what it claimed and what is true.
                    let claimed = ctx.seat_id();
                    Some(match seat.as_deref() {
                        Some(real) if real != claimed => {
                            format!("{} (the seat of that pane is {real})", error.detail)
                        }
                        _ => error.detail,
                    })
                }
            },
        },
    };
    WhoAmI {
        pane,
        seat,
        store: store_path,
        reason,
    }
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
    let real_label = real
        .clone()
        .unwrap_or_else(|| "a pane this session does not know".to_string());
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
            meta: None,
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
