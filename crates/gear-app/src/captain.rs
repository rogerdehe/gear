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
//!
//! `gear captain handover --facts` prints the template the outgoing captain
//! writes the document from, with its part 1 filled from the store
//! (`gear/voyages/M6/handover.md` part B). `gear captain take` asks the
//! previous captain for that document; when it does not arrive within a
//! bounded time, the new captain is pointed at the previous session to read
//! and summarise.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{config, identity, plan, Env};
use gear_herdr::{HerdrRead, Outcome};
use gear_model::{
    layout, AttemptState, ErrorCode, GearError, GearResult, MsgKind, MsgState, SeatRole, SeatState,
    TaskState, Ts,
};
use gear_store::{BindingRow, BindingStatus, NewMessage, Store, Tx};

/// How long the outgoing captain has to answer the request for its handover
/// before the new captain is pointed at its session (handover.md part B).
pub const HANDOVER_ASK_MIN: i64 = 10;

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
    /// The previous captain's handover document, when one was already there.
    pub document: Option<String>,
    /// The request for a handover document, when none was there yet.
    pub request: Option<HandoverRequest>,
}

/// The request `gear captain take` sends the previous captain for its
/// handover document, and the bounded time it has to answer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HandoverRequest {
    /// The pane the previous captain runs in.
    pub pane: String,
    pub kind: String,
    /// The CLI session of the previous captain, to read when it cannot answer.
    pub session_ref: Option<String>,
    /// Whether Herdr took the prompt (an agent was there and answered the
    /// target).
    pub asked: bool,
    pub at: Ts,
    pub deadline: Ts,
}

/// What `gear captain handover --facts` prints: the template's part 1, filled
/// from the store, and the skeleton of parts 2 and 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoverFacts {
    pub project: String,
    pub voyage: String,
    /// The folder the handover documents live in, as the project's conventions
    /// name it.
    pub directory: String,
    /// The newest handover document found there, when there is one.
    pub document: Option<String>,
    pub text: String,
}

/// What the caller knows about the machine and the store does not: the
/// engine's binary and liveness (handover.md part B).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    /// One line on the engine: running or down, its binary, its version.
    pub engine: Option<String>,
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
    //    was closed): `from` is then simply unknown. The binding also carries
    //    the session the previous captain can be asked from.
    let previous = store.read(|tx| tx.get_binding(&captain, seat.generation))?;
    let from = previous.as_ref().map(|binding| binding.pane_id.clone());
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

    // 5. The previous captain's handover (handover.md part B): a document it
    //    already wrote is named to the new captain; when there is none, the
    //    previous captain is asked for it, and after a bounded time the new
    //    captain is pointed at its session to read and summarise.
    let mut document: Option<String> = None;
    let mut directory: Option<String> = None;
    let mut request: Option<HandoverRequest> = None;
    if let Some(previous) = &previous {
        let voyage = store.read(|tx| plan::current_voyage(tx, &project))?;
        if !voyage.is_empty() {
            let dir = format!(
                "{}/handovers",
                layout::voyage_dir(&config.conventions.voyage_dir, &project, &voyage)
            );
            document = newest_handover(cwd, &dir);
            if document.is_none() {
                // Ask the agent still running in the previous captain's pane.
                // A pane that lost its agent (or its name) is not a failure:
                // the session is named for the new captain to read.
                let mut asked = false;
                if herdr.agent_get(&previous.pane_id).ok().flatten().is_some() {
                    let target = herdr
                        .agent_names()
                        .ok()
                        .and_then(|names| names.get(&previous.pane_id).cloned().flatten())
                        .unwrap_or_else(|| previous.pane_id.clone());
                    let text = request_text(&project, &pane, generation, &kind, &dir);
                    asked = matches!(herdr.prompt(&target, &text), Outcome::Ok(()));
                }
                request = Some(HandoverRequest {
                    pane: previous.pane_id.clone(),
                    kind: previous.kind.clone(),
                    session_ref: previous.session_ref.clone(),
                    asked,
                    at: now,
                    deadline: now + HANDOVER_ASK_MIN * 60,
                });
            }
            directory = Some(dir);
        }
    }

    // 6. One urgent item to the captain seat (the pane that just took it, so
    //    the user reads it on the captain's home page), with the handover
    //    pointer the new captain needs before its task packet.
    let mut line = format!(
        "{captain}: pane {pane} took the captain of {project} ({kind}), {}. This pane answers as the captain now: there is no GEAR_CTX to set — the pane's active binding is the seat.",
        match &from {
            Some(from) => format!("it left pane {from}"),
            None => "it had no pane recorded".to_string(),
        }
    );
    if let Some(document) = &document {
        line.push_str(&format!(
            "\n\nThe previous captain left its handover at {document}: read it before the task packet."
        ));
    } else if let (Some(request), Some(directory)) = (&request, &directory) {
        line.push_str(&handover_pointer(request, directory));
    }
    store.write(|tx| {
        if let Some(request) = &request {
            let raw = serde_json::to_string(request).map_err(|error| {
                GearError::new(
                    ErrorCode::BadFormat,
                    format!("cannot record the handover request: {error}"),
                )
            })?;
            tx.set_meta(&request_key(&project), &raw)?;
        }
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

    // 7. The new captain's agent carries the captain's name (T-134): Herdr
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
        document,
        request,
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

/// The key of the request `gear captain take` left for the previous captain.
fn request_key(project: &str) -> String {
    format!("captain-handover:{project}")
}

/// The request recorded for `project`, when there is one.
fn load_request(tx: &Tx<'_>, project: &str) -> GearResult<Option<HandoverRequest>> {
    let Some(raw) = tx.get_meta(&request_key(project))? else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    Ok(serde_json::from_str(&raw).ok())
}

/// The newest handover document under `directory` (relative to `root`), when
/// there is one. The names carry the date and time, so the last one in name
/// order is the newest.
fn newest_handover(root: &Path, directory: &str) -> Option<String> {
    let mut names: Vec<String> = std::fs::read_dir(root.join(directory))
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.starts_with("captain-") && name.ends_with(".md"))
        .collect();
    names.sort();
    names
        .pop()
        .map(|name| format!("{directory}/{name}"))
}

/// The prompt `gear captain take` sends the previous captain: write the
/// handover document from the template, commit it, and the new captain reads
/// it before its task packet.
fn request_text(project: &str, pane: &str, generation: i64, kind: &str, directory: &str) -> String {
    format!(
        "Captain handover of {project}: pane {pane} took the captain seat at generation {generation} ({kind}). Write your handover document now, from `gear captain handover --facts`, under {directory} (named captain-<date>-<time>.md as the template says), following part B of the voyage's handover.md; commit and push it. The new captain reads it before its task packet. If it is not there within {HANDOVER_ASK_MIN} minutes, the new captain reads your session and writes part 2 itself."
    )
}

/// The line a take item carries when no document was there: what was asked,
/// and where the new captain reads the previous session after the deadline.
fn handover_pointer(request: &HandoverRequest, directory: &str) -> String {
    let session = match &request.session_ref {
        Some(session) => format!("session {session} ({})", request.kind),
        None => "its session is not recorded".to_string(),
    };
    if request.asked {
        format!(
            "\n\nThe previous captain (pane {}) was asked for its handover under {directory}. If no captain-*.md document is there by {} ({} minutes), read its {session} and write part 2 yourself.",
            request.pane,
            utc_date_time(request.deadline),
            HANDOVER_ASK_MIN
        )
    } else {
        format!(
            "\n\nThe previous captain's pane {} hosts no agent to ask: no handover document is under {directory}. Read its {session} and write part 2 yourself.",
            request.pane
        )
    }
}

/// `gear captain handover --facts` (handover.md part B): the template the
/// outgoing captain writes the document from, with its part 1 filled from the
/// store, and the skeleton of parts 2 and 3.
///
/// Read-only. It works for a caller whose generation is already stale (the
/// previous captain after a `captain take`): the project comes from the
/// context or the working directory, and no seat is proven.
pub fn handover_facts(
    store: &mut Store,
    env: &Env,
    cwd: &Path,
    machine: &Machine,
    now: Ts,
) -> GearResult<HandoverFacts> {
    let project = project_of(store, env, cwd)?;
    let config = config::latest(store, &project)?;
    let milestone = store.read(|tx| plan::current_voyage(tx, &project))?;
    if milestone.is_empty() {
        return Err(GearError::new(
            ErrorCode::NoTarget,
            format!("the project {project} has no voyage: a captain handover is a voyage's document"),
        ));
    }
    let voyage = store
        .read(|tx| tx.get_voyage(&project, &milestone))?
        .ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!("no voyage {milestone} in project {project}"),
            )
        })?;
    let directory = format!(
        "{}/handovers",
        layout::voyage_dir(&config.conventions.voyage_dir, &project, &milestone)
    );
    let document = newest_handover(cwd, &directory);
    let request = store.read(|tx| load_request(tx, &project))?;
    let plan_path = layout::plan_file(&config.conventions.voyage_dir, &project, &milestone);
    let plan_text = std::fs::read_to_string(cwd.join(&plan_path)).ok();
    let plan_base = plan_text.as_deref().and_then(plan_base_of);
    let plan_state = git_plan_state(&config.root_repo().path, &plan_path);
    let releases = git_releases(&config.root_repo().path);
    let text = store.read(|tx| {
        render_facts(
            tx,
            &project,
            &milestone,
            &voyage,
            &config,
            &directory,
            document.as_deref(),
            request.as_ref(),
            &plan_path,
            plan_text.is_some(),
            plan_base,
            plan_state.as_deref(),
            releases.as_deref(),
            machine.engine.as_deref(),
            now,
        )
    })?;
    Ok(HandoverFacts {
        project,
        voyage: milestone,
        directory,
        document,
        text,
    })
}

/// The `base=` version of a plan file, from its first line.
fn plan_base_of(text: &str) -> Option<i64> {
    let first = text.lines().next()?;
    let start = first.find("base=")? + "base=".len();
    let digits: String = first[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// `clean`, `modified` or `untracked` for the plan file; `None` when git
/// cannot answer (not a checkout, a missing repo).
fn git_plan_state(repo: &str, plan_path: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain", "--", plan_path])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let status = String::from_utf8_lossy(&output.stdout);
    if status.trim().is_empty() {
        return Some("clean".to_string());
    }
    if status.lines().any(|line| line.starts_with("??")) {
        return Some("untracked".to_string());
    }
    Some("modified".to_string())
}

/// The releases tagged in the checkout, newest first; `None` when git cannot
/// answer. GEAR records no releases itself.
fn git_releases(repo: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["tag", "--list", "v*", "--sort=-version:refname"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let tags: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .take(5)
        .collect();
    if tags.is_empty() {
        return Some("none tagged in this checkout".to_string());
    }
    Some(format!(
        "{} (from `git tag`; GEAR records no releases itself)",
        tags.join(", ")
    ))
}

/// `YYYY-MM-DD HH:MM UTC`.
fn utc_date_time(ts: i64) -> String {
    let secs = ts.rem_euclid(86_400);
    format!(
        "{} {:02}:{:02} UTC",
        plan::utc_date(ts),
        secs / 3_600,
        (secs % 3_600) / 60
    )
}

/// `YYYY-MM-DD-HHMM`, the stamp of a handover document's name.
fn file_stamp(ts: i64) -> String {
    let secs = ts.rem_euclid(86_400);
    format!(
        "{}-{:02}{:02}",
        plan::utc_date(ts),
        secs / 3_600,
        (secs % 3_600) / 60
    )
}

/// One line of text, at most `max` characters.
fn flat(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let mut cut: String = line.chars().take(max).collect();
    cut.push('…');
    cut
}

/// `12h 30m` for a duration in seconds.
fn hours_min(seconds: i64) -> String {
    let seconds = seconds.max(0);
    format!("{}h {}m", seconds / 3_600, (seconds % 3_600) / 60)
}

/// The template and its part 1 facts (handover.md part B), read from the
/// store inside one transaction.
#[allow(clippy::too_many_arguments)]
fn render_facts(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
    voyage: &gear_store::VoyageRow,
    config: &config::ProjectConfig,
    directory: &str,
    document: Option<&str>,
    request: Option<&HandoverRequest>,
    plan_path: &str,
    plan_present: bool,
    plan_base: Option<i64>,
    plan_state: Option<&str>,
    releases: Option<&str>,
    engine: Option<&str>,
    now: Ts,
) -> GearResult<String> {
    let captain = format!("{project}:captain");
    let board = gear_board::project_with_agents(tx, project, now, None)?;
    let seats = tx.seats_of_project(project)?;
    let tasks = tx.tasks_of_project(project)?;
    let open_items = tx.actionable_for(project, &captain, now)?;
    let stations = gear_board::stations::progress(tx, project, milestone)?;
    let plan_version = tx.plan_version(project)?;
    let plan_row = tx.get_plan(project, plan_version)?;
    let mut lines: Vec<String> = Vec::new();

    lines.push(format!("# Captain handover — {project} · voyage {milestone}"));
    lines.push(String::new());
    lines.push(format!(
        "Write the document from this template at {directory}/captain-{}.md, in three parts as handover.md part B says: the facts below, your own summary, free notes. Commit it before the captain seat moves. The incoming captain reads the whole document before its task packet.",
        file_stamp(now)
    ));
    lines.push(String::new());

    lines.push("## 1. Facts (GEAR printed them; check each against the worktree or the store)".to_string());
    let departure = match voyage.departure {
        Some(departure) => format!(
            "departed {} with {} tasks",
            utc_date_time(departure),
            voyage.departure_tasks.unwrap_or(0)
        ),
        None => "not departed yet".to_string(),
    };
    lines.push(format!(
        "- Project: {project} · voyage {milestone} {:?} · {departure}",
        voyage.title
    ));
    if let Some(departure) = voyage.departure {
        lines.push(format!(
            "- Elapsed: {} since departure · eta {}",
            hours_min(now - departure),
            voyage.eta.as_deref().unwrap_or("not set")
        ));
    }
    lines.push(format!(
        "- Goal: {}",
        voyage.done.as_deref().unwrap_or("not described yet")
    ));
    let clauses = crate::voyage::report(
        tx,
        project,
        milestone,
        false,
        now,
    )?
    .done_clauses;
    lines.push(format!(
        "- Done clauses: {}",
        if clauses.is_empty() {
            "none".to_string()
        } else {
            clauses.join(" · ")
        }
    ));
    lines.push(format!(
        "- Bar: {}",
        voyage.bar.as_deref().unwrap_or("not set")
    ));
    lines.push(format!(
        "- Eta: {}",
        voyage.eta.as_deref().unwrap_or("not set")
    ));
    if stations.is_empty() {
        lines.push("- Stations: none in the plan".to_string());
    } else {
        let mut parts = Vec::new();
        for station in &stations {
            let state = match station.state {
                gear_board::stations::StationState::Reached => "reached",
                gear_board::stations::StationState::Now => "now",
                gear_board::stations::StationState::Next => "next",
                gear_board::stations::StationState::Later => "later",
            };
            let reached = match station.reached_at_min {
                Some(min) => format!(", reached at {min} min"),
                None => String::new(),
            };
            parts.push(format!(
                "{} {state}: {} ({} of {} merged, {} min left{reached})",
                station.id,
                station.needs,
                station.merged,
                station.tasks,
                station.left_min
            ));
        }
        lines.push(format!("- Stations: {}", parts.join(" · ")));
    }
    lines.push(format!(
        "- Releases published: {}",
        releases.unwrap_or("unknown (git could not answer)")
    ));

    // Staffing (handover.md part B): the seats, their CLIs and models, the
    // quotas, and the role charters (a lane's duty and owned paths).
    lines.push("- Staffing:".to_string());
    let model_of = |seat: &str| -> String {
        tx.get_meta(&format!("engine.model.{seat}"))
            .ok()
            .flatten()
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    };
    let head = |chain: &[config::ModelEntry]| -> String {
        chain
            .first()
            .map(|entry| entry.describe())
            .unwrap_or_else(|| "not managed".to_string())
    };
    if let Some(seat) = seats.iter().find(|seat| seat.id == captain) {
        let binding = tx.get_binding(&captain, seat.generation)?;
        lines.push(format!(
            "  - captain {}: {}/{} · model {} · seat {} · binding {} generation {}",
            seat.id,
            config.captain.kind,
            config.captain.profile,
            model_of(&seat.id),
            seat.state.as_str(),
            binding.as_ref().map(|b| b.pane_id.as_str()).unwrap_or("-"),
            seat.generation
        ));
    }
    let lanes: Vec<&config::LaneSection> = config.lanes.iter().collect();
    lines.push(format!(
        "  - lanes: {} configured, {} running",
        lanes.len(),
        seats
            .iter()
            .filter(|seat| seat.role == SeatRole::Lane && seat.state == SeatState::Active)
            .count()
    ));
    for lane in lanes {
        let seat = seats
            .iter()
            .find(|seat| seat.role == SeatRole::Lane && seat.name == lane.name);
        let chain = if lane.models.is_empty() {
            &config.models.lanes
        } else {
            &lane.models
        };
        let binding = match seat {
            Some(seat) => tx.get_binding(&seat.id, seat.generation)?,
            None => None,
        };
        lines.push(format!(
            "    - {}: {} · owns {} · {}/{} · model {} · seat {} · pane {}",
            lane.name,
            lane.duty,
            lane.owns.join(", "),
            lane.kind,
            lane.profile,
            match seat {
                Some(seat) => model_of(&seat.id),
                None => head(chain),
            },
            seat.map(|seat| seat.state.as_str()).unwrap_or("no seat"),
            binding.as_ref().map(|b| b.pane_id.as_str()).unwrap_or("-")
        ));
    }
    let temps: Vec<_> = seats.iter().filter(|seat| seat.role == SeatRole::Temp).collect();
    lines.push(format!(
        "  - reviewers: {} seats · model {} · quota temps {}",
        temps.len(),
        head(&config.models.review),
        config.quota.temps
    ));
    for seat in temps {
        lines.push(format!(
            "    - {}: seat {} · model {} · pane {}",
            seat.id,
            seat.state.as_str(),
            model_of(&seat.id),
            tx.get_binding(&seat.id, seat.generation)?
                .map(|b| b.pane_id)
                .unwrap_or_else(|| "-".to_string())
        ));
    }
    lines.push(format!(
        "  - quota: lanes {}, temps {} (configured)",
        config.quota.lanes, config.quota.temps
    ));

    // The board state: what is done, in progress, in review, waiting for the
    // captain, and the reworks queued.
    let mut reworks: Vec<String> = Vec::new();
    for task in &tasks {
        if !matches!(task.state, TaskState::Ready | TaskState::Running | TaskState::Waiting) {
            continue;
        }
        if tx
            .attempts_of_task(project, &task.id)?
            .last()
            .is_some_and(|attempt| attempt.state == AttemptState::Rejected)
        {
            reworks.push(task.id.clone());
        }
    }
    lines.push(format!(
        "- Board: {} tasks · {} done · {} in progress · {} waiting · {} handed in, {} sent back",
        board.totals.tasks,
        board.totals.done,
        board.totals.in_progress,
        board.totals.waiting,
        board.totals.handed_in,
        board.totals.rejected
    ));
    lines.push(format!(
        "- Waiting for the captain: {} items · reworks queued: {}",
        board.waiting.count,
        if reworks.is_empty() {
            "none".to_string()
        } else {
            reworks.join(", ")
        }
    ));
    if !board.waiting_on.is_empty() {
        lines.push("- In review or waiting on the captain:".to_string());
        for line in &board.waiting_on {
            lines.push(format!("    - {line}"));
        }
    }
    if !board.lanes.is_empty() {
        lines.push("- Lanes now:".to_string());
        for lane in &board.lanes {
            lines.push(format!("    - {}", gear_board::lane_line(lane)));
        }
    }

    // The open threads (handover.md part B): every inbox item by id, its text,
    // and what it needs.
    lines.push(format!("- Open items ({}):", open_items.len()));
    for message in &open_items {
        lines.push(format!(
            "    - {}{} {} ({} attempt {}): {} — needs: {}",
            message.id,
            if message.urgent { " [urgent]" } else { "" },
            message.thread,
            message.task.as_deref().unwrap_or("no task"),
            message.attempt.unwrap_or(0),
            flat(&message.body, 240),
            needs_of(tx, project, message, plan_path)?
        ));
    }

    // The plan (handover.md part B): the applied version, and whether the file
    // has edits not applied or not committed.
    let mut plan_line = format!(
        "- Plan: applied version {plan_version}{} · plan file {plan_path} ({})",
        plan_row
            .as_ref()
            .map(|row| format!(" applied {}", utc_date_time(row.applied)))
            .unwrap_or_default(),
        match plan_state {
            Some(state) => state.to_string(),
            None => "git could not answer".to_string(),
        }
    );
    if !plan_present {
        plan_line.push_str(" · plan file missing");
    } else if let Some(base) = plan_base {
        if base < plan_version {
            plan_line.push_str(&format!(
                " · edits not applied (file base={base}, applied {plan_version})"
            ));
        }
    }
    lines.push(plan_line);

    // The shared machine (handover.md part B): the other projects' captains
    // and how to reach them, and the engine.
    let mut others: Vec<String> = Vec::new();
    for id in tx.project_ids()? {
        if id == project {
            continue;
        }
        let other_captain = format!("{id}:captain");
        let pane = tx
            .get_seat(&other_captain)?
            .and_then(|seat| tx.get_binding(&other_captain, seat.generation).ok().flatten())
            .map(|binding| binding.pane_id);
        let session = tx
            .get_project(&id)?
            .map(|row| row.herdr_session)
            .unwrap_or_default();
        others.push(format!(
            "{id} (session {session}, captain pane {})",
            pane.unwrap_or_else(|| "-".to_string())
        ));
    }
    lines.push(format!(
        "- Shared machine: other projects: {} · reach another captain with `gear captain take <project>` from its checkout or in its pane · engine: {}",
        if others.is_empty() {
            "none".to_string()
        } else {
            others.join("; ")
        },
        engine.unwrap_or("not known to this caller")
    ));

    // The previous captain's handover: the document when it is there, else the
    // request and the session to read after its bounded time.
    if let Some(document) = document {
        lines.push(format!(
            "- Previous captain's handover: {document} (read it before the task packet)"
        ));
    } else if let Some(request) = request {
        if request.deadline <= now {
            lines.push(format!(
                "- Previous captain's handover: none under {directory} after the request ({}); read its {} and write part 2 yourself",
                utc_date_time(request.deadline),
                match &request.session_ref {
                    Some(session) => format!("session {session} ({})", request.kind),
                    None => "session (not recorded)".to_string(),
                }
            ));
        } else {
            lines.push(format!(
                "- Previous captain's handover: asked under {directory} (deadline {}); none yet, read its {} if it does not arrive",
                utc_date_time(request.deadline),
                match &request.session_ref {
                    Some(session) => format!("session {session} ({})", request.kind),
                    None => "session (not recorded)".to_string(),
                }
            ));
        }
    }

    lines.push(String::new());
    lines.push("## 2. Summary (you write it; the incoming captain writes it from your session if you cannot)".to_string());
    lines.push("- The user's standing orders this voyage (never quoted), and the reason behind each when known:".to_string());
    lines.push("- Decisions in flight and why (what is decided and not yet in the plan; what was rejected):".to_string());
    lines.push("- What is fragile right now (a lane that loops, a reviewer to watch, a recovery to repeat, a known engine bug and its workaround):".to_string());
    lines.push("- Promises made (to the user, to other captains: a rebuild, a release, an answer):".to_string());
    lines.push("- The next three things to do, in order:".to_string());
    lines.push("- Lessons this voyage not yet in lessons.md:".to_string());
    lines.push("- Files to read first:".to_string());
    lines.push(String::new());
    lines.push("## 3. Free notes (optional)".to_string());

    Ok(lines.join("\n"))
}

/// What an open item needs (handover.md part B): the command that settles it.
fn needs_of(
    tx: &Tx<'_>,
    project: &str,
    message: &gear_store::MessageRow,
    plan_path: &str,
) -> GearResult<String> {
    if crate::station::is_station(message) {
        let milestone = message.thread.trim_start_matches("station:");
        return Ok(format!(
            "gear plan apply {plan_path} (# Decisions: station {milestone}: keep course|time-box|cut|move)"
        ));
    }
    if message.needs_ack {
        return Ok(format!(
            "gear send --kind ack --reply-to {}",
            message.id
        ));
    }
    if let Some(hint) = crate::tasks::verdict_hint(tx, project, message)? {
        return Ok(crate::home::verdict_command(&hint));
    }
    Ok(format!(
        "gear reply {} --resolve -m \"<answer>\"",
        message.id
    ))
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
