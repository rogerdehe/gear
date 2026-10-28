//! The captain seat's handover (T-104).
//!
//! A seat is proven by the pane the caller really runs in (see
//! [`crate::identity`]). Which pane is the captain is the store's binding, not
//! a process: a captain restarted in the same pane is still the captain. When
//! the captain moves to another pane (a bigger terminal, a new window), the
//! store must follow without a hand edit — that is this command:
//!
//! `gear captain handover --pane <id>` rebinds the captain seat to `<pane>` at
//! a new generation. It is allowed only from the **current captain's real
//! pane**, or from a shell whose real pane hosts **no agent** (the user's own
//! terminal). A lane or reviewer asking is refused, and the captain gets one
//! urgent item saying which pane asked.

use std::path::{Path, PathBuf};

use crate::{config, identity, Env};
use gear_herdr::HerdrRead;
use gear_model::{ErrorCode, GearError, GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{BindingRow, BindingStatus, NewMessage, Store};

/// What a handover did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handover {
    pub project: String,
    pub captain: String,
    /// The pane the captain was bound to.
    pub from: String,
    /// The pane it is bound to now.
    pub to: String,
    pub generation: i64,
    pub kind: String,
    /// Who asked: the captain's own agent, or the user's own terminal.
    pub asker: Asker,
}

/// Who asked for the handover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asker {
    /// An agent in the captain's own pane.
    Captain,
    /// A shell with no agent in its pane: the user's own terminal.
    UserShell,
}

/// Rebind the captain seat of the caller's project to `pane` (T-104).
///
/// The caller's real pane decides who may ask; the target pane must already
/// host an agent (the captain's CLI). The seat keeps its role and profile — the
/// binding is what moves — and gets a new generation, so an old session of the
/// captain cannot write any more.
pub fn handover(
    store: &mut Store,
    env: &Env,
    herdr: &mut dyn HerdrRead,
    ancestors: &[u32],
    // The caller's working directory: a handover from the user's own terminal
    // (no GEAR_CTX) names its project by it.
    cwd: &Path,
    pane: &str,
    now: Ts,
) -> GearResult<Handover> {
    // 1. Where does this process really run? Without a pane nothing can be
    //    proven about the caller.
    let Some(real) = identity::real_pane(herdr, ancestors) else {
        return Err(GearError::new(
            ErrorCode::Forbidden,
            "the handover runs in no pane of this session: run it in the captain's pane or in your own terminal",
        ));
    };
    // 2. The project: the caller's own, else the one whose checkout holds the
    //    working directory (the user's terminal in the project).
    let project = project_of(store, env, cwd)?;
    let captain = format!("{project}:captain");
    let seat = store
        .read(|tx| tx.get_seat(&captain))?
        .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {captain}")))?;
    let config = config::latest(store, &project)?;
    let binding = store
        .read(|tx| tx.get_binding(&captain, seat.generation))?
        .ok_or_else(|| {
            GearError::new(
                ErrorCode::StaleGeneration,
                format!(
                    "the captain of {project} has no binding at generation {}",
                    seat.generation
                ),
            )
        })?;

    // 3. Who asks: an agent may hand the captain over only from the captain's
    //    own pane; a shell with no agent in its pane is the user's terminal.
    let asker = match herdr.agent_get(&real).map_err(|e| {
        GearError::new(
            ErrorCode::BadFormat,
            format!("cannot read pane {real}: {}", e.0),
        )
    })? {
        Some(_info) if real == binding.pane_id => Asker::Captain,
        Some(info) => {
            // A lane or a reviewer asking for the captain's seat.
            let line = format!(
                "a handover of {captain} was refused: it was asked from pane {real}, where the {} agent {} runs, and the captain is bound to pane {}; only the captain's own pane or the user's own terminal may hand it over",
                info.agent.as_deref().unwrap_or("unmanaged"),
                real,
                binding.pane_id
            );
            refused_item(store, &project, &real, &line, now)?;
            return Err(GearError::new(ErrorCode::Forbidden, line));
        }
        None => Asker::UserShell,
    };

    // 4. The target must host the captain's agent already: the binding follows
    //    a running CLI, it never starts one.
    let new_agent = herdr
        .agent_get(pane)
        .map_err(|e| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("cannot read pane {pane}: {}", e.0),
            )
        })?
        .ok_or_else(|| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("pane {pane} hosts no agent: start the captain's CLI there first"),
            )
        })?;
    let kind = new_agent
        .agent
        .clone()
        .filter(|kind| !kind.is_empty())
        .unwrap_or_else(|| config.captain.kind.clone());
    let session_ref = new_agent
        .agent_session
        .as_ref()
        .map(|session| session.value.clone());

    // 5. Rebind: a new generation, the old bindings voided, the seat still the
    //    captain (role and profile stay; only the pane moves).
    let generation = rebind(
        store,
        &captain,
        &seat,
        pane,
        &kind,
        session_ref,
        &config,
        now,
    )?;

    Ok(Handover {
        project,
        captain,
        from: binding.pane_id.clone(),
        to: pane.to_string(),
        generation,
        kind,
        asker,
    })
}

/// What `gear captain take` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    pub project: String,
    pub captain: String,
    /// The pane it left; `None` when it had none recorded.
    pub from: Option<String>,
    pub to: String,
    pub generation: i64,
    pub kind: String,
    /// What naming the new captain's agent did (T-134): Herdr refuses a prompt
    /// to an unnamed agent, so the pane carries the captain's name from the
    /// moment it takes the seat.
    pub named: gear_herdr::Ensured,
}

/// `gear captain take` (T-126): the agent in this pane **becomes** the
/// project's captain, with no environment and no pane id to type.
///
/// The caller's pane is the one it really runs in (ancestry), else
/// `HERDR_PANE_ID`, else — an agent whose tools run in a daemon, so in no pane —
/// the only pane of the session whose agent is of a daemon kind and is bound to
/// no seat. That pane must host an agent (a captain is a running CLI, never a
/// shell) and must not be a lane's or a reviewer's pane. The project comes from
/// the working directory, as for a handover from the user's terminal.
///
/// The captain seat is rebound to that pane at a new generation through the same
/// rebind as a handover, so it works whether the old captain's pane still exists
/// or not; one urgent item tells the captain seat (and so the user) which pane
/// took it and which one it left.
pub fn take(
    store: &mut Store,
    env: &Env,
    herdr: &mut dyn gear_herdr::HerdrAct,
    ancestors: &[u32],
    cwd: &Path,
    named: Option<&str>,
    now: Ts,
) -> GearResult<Taken> {
    // 1. The pane: the caller's own, else the one unbound daemon pane.
    let pane = identity::caller_pane(herdr, ancestors, env.pane_id.as_deref())
        .or(identity::daemon_pane(store, herdr, false)?)
        .ok_or_else(|| {
            GearError::new(
                ErrorCode::Forbidden,
                "`gear captain take` cannot tell which pane this is: run it from the agent's own pane, or from an agent whose CLI runs its tools in a daemon (codex, opencode) with exactly one such pane bound to no seat",
            )
        })?;
    // 2. The project: the one named, else the checkout holding the cwd, else
    //    the only registered one; with several, the list to ask the user from.
    let project = match named {
        Some(name) => {
            if store.read(|tx| tx.get_project(name))?.is_none() {
                return Err(GearError::new(
                    ErrorCode::NoTarget,
                    format!("no project {name:?}: {}", project_list(store, herdr)?),
                ));
            }
            name.to_string()
        }
        None => match project_of(store, env, cwd) {
            Ok(project) => project,
            Err(_) => {
                let ids = store.read(|tx| tx.project_ids())?;
                if ids.len() == 1 {
                    ids[0].clone()
                } else {
                    return Err(GearError::new(
                        ErrorCode::BadFormat,
                        format!("which project? Ask the user, then run `gear captain take <project>`. {}", project_list(store, herdr)?),
                    ));
                }
            }
        },
    };
    let captain = format!("{project}:captain");
    let seat = store
        .read(|tx| tx.get_seat(&captain))?
        .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {captain}")))?;
    let config = config::latest(store, &project)?;
    // 3. The pane must host an agent, and that agent must not be another seat's.
    let occupant = herdr
        .agent_get(&pane)
        .map_err(|e| GearError::new(ErrorCode::BadFormat, format!("cannot read pane {pane}: {e:?}")))?
        .ok_or_else(|| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("pane {pane} hosts no agent: start your CLI there and run `gear captain take` in it"),
            )
        })?;
    if let Some(other) = store.read(|tx| identity::seat_bound_to(tx, &pane))? {
        if other.role != SeatRole::Captain {
            return Err(GearError::new(
                ErrorCode::Forbidden,
                format!(
                    "pane {pane} is the {} seat {}'s: a captain cannot take a lane's or a reviewer's pane; start your own CLI in another pane and run `gear captain take` there",
                    other.role.as_str(),
                    other.id
                ),
            ));
        }
    }
    let kind = occupant
        .agent
        .clone()
        .filter(|kind| !kind.is_empty())
        .unwrap_or_else(|| config.captain.kind.clone());
    let session_ref = occupant
        .agent_session
        .as_ref()
        .map(|session| session.value.clone());
    // 4. The seat's binding may be gone with its pane (the old captain's pane
    //    was closed): `from` is then simply unknown.
    let from = store
        .read(|tx| tx.get_binding(&captain, seat.generation))?
        .map(|binding| binding.pane_id);
    let generation = rebind(
        store,
        &captain,
        &seat,
        &pane,
        &kind,
        session_ref,
        &config,
        now,
    )?;

    // 5. One urgent item to the captain seat (the pane that just took it, so
    //    the user reads it on the captain's home page).
    let line = format!(
        "{captain}: pane {pane} took the captain of {project} ({kind}), {}. This pane answers as the captain now: there is no GEAR_CTX to set — the pane's active binding is the seat.",
        match &from {
            Some(from) => format!("it left pane {from}"),
            None => "it had no pane recorded".to_string(),
        }
    );
    store.write(|tx| {
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.clone(),
            thread: "captain:take".to_string(),
            task: None,
            attempt: None,
            sender: "engine".to_string(),
            sender_gen: 0,
            recipient: captain.clone(),
            kind: MsgKind::System,
            urgent: true,
            body: line.clone(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })
    })?;

    // 6. The new captain's agent carries the captain's name (T-134): Herdr
    //    clears a name when it re-detects the occupant, and a prompt to an
    //    unnamed agent is refused, so the seat must not start out nameless.
    let named = gear_herdr::ensure_agent_name(
        herdr,
        &pane,
        &crate::config::agent_name(&project, "captain"),
    );

    Ok(Taken {
        project,
        captain,
        from,
        to: pane,
        generation,
        kind,
        named,
    })
}

/// The one rebind a handover and a `captain take` share (T-104, T-126): a new
/// generation, every old binding void, the seat `Active` again, and a fresh
/// `Active` binding on `pane` with the CLI it hosts. The seat keeps its role and
/// profile — only the pane moves.
#[allow(clippy::too_many_arguments)]
fn rebind(
    store: &mut Store,
    captain: &str,
    seat: &gear_store::SeatRow,
    pane: &str,
    kind: &str,
    session_ref: Option<String>,
    config: &config::ProjectConfig,
    now: Ts,
) -> GearResult<i64> {
    let generation = seat.generation + 1;
    let reason = format!("moved to pane {pane}");
    store.write(|tx| {
        for old in 1..=seat.generation {
            if tx.get_binding(captain, old)?.is_some() {
                tx.set_binding_status(captain, old, BindingStatus::Void)?;
            }
        }
        tx.set_seat_generation(captain, generation)?;
        tx.set_seat_state(captain, SeatState::Active, Some(&reason), now)?;
        tx.insert_binding(&BindingRow {
            seat: captain.to_string(),
            generation,
            session: config.project.herdr_session.clone(),
            pane_id: pane.to_string(),
            kind: kind.to_string(),
            session_ref,
            status: BindingStatus::Active,
            handover_id: None,
            created: now,
        })
    })?;
    Ok(generation)
}

/// The project a handover is for: the caller's context, else the project whose
/// checkout holds the working directory (the user's own terminal).
fn project_of(store: &mut Store, env: &Env, cwd: &Path) -> GearResult<String> {
    if let Some(ctx) = &env.ctx {
        return Ok(ctx.project.clone());
    }
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    for project in store.read(|tx| tx.project_ids())? {
        let Ok(config) = config::latest(store, &project) else {
            continue;
        };
        let holds = config.repos().iter().any(|repo| {
            let path = PathBuf::from(&repo.path);
            let path = path.canonicalize().unwrap_or(path);
            cwd.starts_with(&path)
        });
        if holds {
            return Ok(project);
        }
    }
    Err(GearError::new(
        ErrorCode::BadFormat,
        format!(
            "no GEAR_CTX and {} belongs to no registered project: run it from the project's checkout (`gear captain take`) or in the captain's pane",
            cwd.display()
        ),
    ))
}

/// One urgent item for the captain: a seat that may not hand the captain over
/// asked for it (T-104). Deduplicated per asking pane while it is open.
fn refused_item(
    store: &mut Store,
    project: &str,
    real: &str,
    line: &str,
    now: Ts,
) -> GearResult<bool> {
    store.write(|tx| {
        let Some(captain) = tx.captain_of(project)? else {
            return Ok(false);
        };
        let thread = format!("handover:{real}");
        let open = tx
            .thread_messages(project, &thread)?
            .into_iter()
            .any(|message| matches!(message.state, MsgState::Open | MsgState::Waiting));
        if open {
            return Ok(false);
        }
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.to_string(),
            thread,
            task: None,
            attempt: None,
            sender: "engine".to_string(),
            sender_gen: 0,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent: true,
            body: line.to_string(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        Ok(true)
    })
}

/// The registered projects with their captain's pane and whether an agent
/// still runs there, for the user to choose from.
fn project_list(store: &mut Store, herdr: &mut dyn HerdrRead) -> GearResult<String> {
    let mut lines = Vec::new();
    for id in store.read(|tx| tx.project_ids())? {
        let captain = format!("{id}:captain");
        let pane = store.read(|tx| {
            let Some(seat) = tx.get_seat(&captain)? else {
                return Ok(None);
            };
            Ok(tx
                .get_binding(&captain, seat.generation)?
                .map(|b| b.pane_id))
        })?;
        let state = match &pane {
            Some(pane) => match herdr.agent_get(pane) {
                Ok(Some(_)) => format!("captain in pane {pane}, running"),
                _ => format!("captain's pane {pane} is gone"),
            },
            None => "no captain".to_string(),
        };
        lines.push(format!("{id} ({state})"));
    }
    Ok(format!("Projects: {}", lines.join("; ")))
}
