//! `gear admin …` (design/01 registration, /13 ops commands).
//!
//! `init` may be run by the user's own shell and, since T-86, by the project's
//! own captain: `GEAR_ADMIN=1` is the user's consent in both cases (the
//! confirmed kickoff sheet is the captain's). Any other seat is `Forbidden`.
//! Every other `admin` command stays user-only.

use crate::config;
use crate::Env;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, Ts};
use gear_store::{BindingRow, BindingStatus, ProjectRow, SeatRow, Store, Tx};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InitResult {
    pub project: String,
    /// The config version this call left behind (1 after the first
    /// registration, the next one after a config change).
    pub config_version: i64,
    /// The seats of the project after this call (captain first, then the lanes
    /// in config order).
    pub seats: Vec<String>,
}

/// The seat that may register a project: the user's own shell (no ctx), or
/// the project's captain (T-86).
const CAPTAIN: &str = "captain";

/// `gear admin init <config>` — register the project, or apply a changed
/// config to a project that is registered already (T-6).
///
/// 1. `GEAR_ADMIN=1` (the user's consent);
/// 2. `config::parse` (errors as is) and `config::validate` — any issue →
///    `BadFormat` whose detail lists every `field: problem`; then the caller:
///    the user's shell, or the captain of the project the config names;
/// 3. one transaction, one of two ways:
///    - **first registration**: `insert_project` (config text verbatim,
///      config_version 1, quotas from `[quota]`, plan_version 0) and one seat
///      per role — `<id>:captain` (Captain, profile = `captain.profile`) and
///      `<id>:<lane>` for each lane (Lane, its profile) — all generation 0,
///      state Queued, `state_reason` "registered". The captain is the user's
///      running agent in `captain.pane` and is bound Active as generation 1
///      (T-63); onboarding the lanes is the engine's job afterwards.
///    - **config change** (the project is registered): the new config becomes
///      the next config version (see [`re_register`]).
/// Checks that need Herdr or git (session, repo, branch) are not part of this
/// cut.
pub fn init(
    store: &mut Store,
    env: &Env,
    config_text: &str,
    now: Ts,
) -> GearResult<InitResult> {
    init_with_home(store, env, config_text, None, now)
}

/// `gear admin init` with an explicit home for the skill check (T-83): the CLI
/// passes the user's `HOME`; a test passes a scratch home with the skills
/// installed. `None` means the process's `HOME`.
pub fn init_with_home(
    store: &mut Store,
    env: &Env,
    config_text: &str,
    home: Option<&std::path::Path>,
    now: Ts,
) -> GearResult<InitResult> {
    // 1. `GEAR_ADMIN=1` is the user's consent, and only the user's own shell
    //    or the project's captain may act on it (T-86). Which project is
    //    checked once its config is parsed, below.
    if !env.admin {
        return Err(GearError::new(
            ErrorCode::Forbidden,
            "`gear admin init` needs GEAR_ADMIN=1: it runs in the user's own shell, or in the captain's pane",
        ));
    }

    // 2. parse and validate the config; every issue is reported at once.
    let config = config::parse(config_text)?;
    let issues = config::validate(&config);
    if !issues.is_empty() {
        let detail = issues
            .iter()
            .map(|issue| format!("{}: {}", issue.field, issue.problem))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!("the config has {} issue(s): {detail}", issues.len()),
        ));
    }

    let project_id = config.project.id.clone();

    // 2b. A caller with a context may only be this project's captain: the
    //     seat's name, the project the context names, and — when the seat is
    //     registered already — its role and generation. A first registration
    //     has no seat yet, so the name and the project are what can be asked.
    if let Some(ctx) = &env.ctx {
        if ctx.seat != CAPTAIN || ctx.project != project_id {
            return Err(GearError::new(
                ErrorCode::Forbidden,
                format!(
                    "`gear admin init` in an agent runs only in the captain's pane of {project_id}, at generation 1; {} carries {}",
                    ctx.seat_id(),
                    ctx.generation
                ),
            ));
        }
        if let Some(seat) = store.read(|tx| tx.get_seat(&ctx.seat_id()))? {
            if seat.role != SeatRole::Captain || seat.generation != ctx.generation {
                return Err(GearError::new(
                    ErrorCode::Forbidden,
                    format!(
                        "`gear admin init` runs in the captain's pane: {} is {}, generation {}",
                        seat.id,
                        seat.role.as_str(),
                        seat.generation
                    ),
                ));
            }
        }
    }

    // Every seat's profile must resolve against the table (built-ins + the
    // project's own) and must be able to run unattended (T-31): an agent that
    // stops on a permission dialog cannot be driven by GEAR.
    let table = config::profile_table(&gear_herdr::builtins(), &config)?;
    let seats_to_check: Vec<(&str, &str)> = std::iter::once(("captain", config.captain.profile.as_str()))
        .chain(config.lanes.iter().map(|lane| (lane.name.as_str(), lane.profile.as_str())))
        .chain(
            config
                .temps
                .iter()
                .map(|temps| ("temps", temps.profile.as_str())),
        )
        .collect();
    for (name, profile_name) in &seats_to_check {
        let Some(profile) = table.get(*profile_name) else {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "{name} uses the profile {profile_name:?}, which is neither built in nor configured under `[profiles.{profile_name}]`"
                ),
            ));
        };
        if profile.unattended_args.is_empty() {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "the profile {profile_name:?} has no unattended args: an agent that stops on permission dialogs cannot run unattended (T-31)"
                ),
            ));
        }
    }

    // Every kind the project can start must have the GEAR skill installed
    // (T-83, trial G06): an agent that never read it does not know how to work
    // as a seat, and the voyage would need the captain's hands. The kinds are
    // every seat's resolved profile **and** every CLI an effective model chain
    // names (T-35): an entry may run its model on another CLI than its seat's
    // kind, and a chain's fallback may name a third, so the profile alone does
    // not say what an unattended run will start. The home is the caller's (the
    // CLI's user); an unset one is the process's. GEAR never installs a skill
    // behind the user's back — it names the command.
    let home = match home {
        Some(home) => home.to_path_buf(),
        None => match std::env::var_os("HOME") {
            Some(home) => std::path::PathBuf::from(home),
            None => {
                return Err(GearError::new(
                    ErrorCode::BadFormat,
                    "HOME is not set: `gear admin init` cannot check that the GEAR skill is installed",
                ))
            }
        },
    };
    let mut kinds: Vec<(String, String)> = Vec::new();
    for (name, profile_name) in &seats_to_check {
        let Some(profile) = table.get(*profile_name) else {
            continue; // reported above
        };
        let who = match *name {
            "captain" => "the captain".to_string(),
            "temps" => "the reviewer".to_string(),
            lane => format!("lane {lane}"),
        };
        note_kind(&mut kinds, &profile.kind, &who);
    }
    for lane in &config.lanes {
        note_chain(
            &mut kinds,
            config::lane_chain(&config, lane),
            &format!("the model chain of lane {}", lane.name),
        );
    }
    note_chain(&mut kinds, &config.models.review, "the reviewers' model chain");
    note_chain(&mut kinds, &config.models.captain, "the captain's model chain");

    for (kind, who) in &kinds {
        let Ok(agent) = crate::skill::SkillAgent::parse(kind) else {
            let known = crate::skill::SkillAgent::ALL
                .iter()
                .map(|agent| agent.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "{who} runs {kind:?}, which GEAR has no skill for: the kinds with a skill are {known}"
                ),
            ));
        };
        if !crate::skill::installed(agent, &home) {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "{who} runs {kind}, whose GEAR skill is not installed in {}: run `gear admin install-skill --agent {kind}` and init again",
                    home.display()
                ),
            ));
        }
    }

    store.write(|tx| {
        match tx.get_project(&project_id)? {
            // The project is registered: the config changes.
            Some(project) => re_register(tx, &project, &config, config_text, now),
            // The project is new: it is registered.
            None => register(tx, &config, config_text, now),
        }
    })
}

/// `gear admin init` behind the seat proof (T-104).
///
/// A context on an **already registered** project is the captain's seat claim:
/// only a caller that really runs in the captain's bound pane may change the
/// configuration, and a claim from anywhere else is refused with the same one
/// urgent item the other seat writes raise. The first registration (no seats
/// yet) and the user's own shell (no context) keep their flow; `herdr` is only
/// needed when there is a claim to prove.
pub fn init_checked(
    store: &mut Store,
    env: &Env,
    config_text: &str,
    herdr: Option<&mut dyn gear_herdr::HerdrRead>,
    ancestors: &[u32],
    home: Option<&std::path::Path>,
    now: Ts,
) -> GearResult<InitResult> {
    if let Some(ctx) = env.ctx.clone() {
        let registered = store.read(|tx| Ok(tx.get_project(&ctx.project)?.is_some()))?;
        if registered {
            let Some(herdr) = herdr else {
                return Err(GearError::new(
                    ErrorCode::NotRegistered,
                    format!(
                        "a registered project's `gear admin init` runs in the captain's pane, which cannot be read: Herdr is unreachable for {}",
                        ctx.project
                    ),
                ));
            };
            prove_captain(store, env, herdr, ancestors, &ctx.project, now)?;
        }
    }
    init_with_home(store, env, config_text, home, now)
}

/// Prove that the caller is the captain of `project` in its own bound pane
/// (T-104): the real pane is the seat's pane. A refusal is reported once to the
/// captain — the same item every seat write raises.
fn prove_captain(
    store: &mut Store,
    env: &Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
    ancestors: &[u32],
    project: &str,
    now: Ts,
) -> GearResult<()> {
    let result = (|| -> GearResult<()> {
        let Some(fp) = crate::identity::observe(env, herdr, ancestors)? else {
            return Err(GearError::new(
                ErrorCode::NotRegistered,
                format!(
                    "the captain's pane hosts no agent, so the caller cannot be the captain of {project}"
                ),
            ));
        };
        store
            .read(|tx| {
                crate::identity::require_seat(tx, env, Some(&fp), Some(&[SeatRole::Captain]), None)
            })
            .map(|_| ())
    })();
    if let Err(error) = &result {
        if error.code == ErrorCode::NotRegistered {
            let _ = crate::identity::report_claim(store, env, herdr, error, ancestors, now);
        }
    }
    result
}

/// First registration: the project row and one seat per role. The captain is
/// bound Active as generation 1 (T-63): it is the user's running agent in
/// `captain.pane`. The lanes stay Queued — the engine onboards them.
fn register(
    tx: &Tx<'_>,
    config: &config::ProjectConfig,
    config_text: &str,
    now: Ts,
) -> GearResult<InitResult> {
    let project_id = config.project.id.clone();

    tx.insert_project(&ProjectRow {
        id: project_id.clone(),
        herdr_session: config.project.herdr_session.clone(),
        config_toml: config_text.to_string(),
        config_version: 1,
        quota_lanes: i64::from(config.quota.lanes),
        quota_temps: i64::from(config.quota.temps),
        quota_includes_captain: config.quota.includes_captain,
        plan_version: 0,
        created: now,
    })?;

    let mut seats: Vec<String> = Vec::new();
    let mut register = |name: &str, role: SeatRole, profile: &str| -> GearResult<()> {
        let seat_id = format!("{project_id}:{name}");
        tx.insert_seat(&SeatRow {
            id: seat_id.clone(),
            project: project_id.clone(),
            role,
            name: name.to_string(),
            profile: profile.to_string(),
            generation: 0,
            state: SeatState::Queued,
            state_since: now,
            state_reason: Some("registered".to_string()),
            task_id: None,
        })?;
        seats.push(seat_id);
        Ok(())
    };

    register("captain", SeatRole::Captain, &config.captain.profile)?;
    for lane in &config.lanes {
        register(&lane.name, SeatRole::Lane, &lane.profile)?;
    }

    // The captain is the user's own agent, already running in `captain.pane`;
    // bind it as generation 1 instead of onboarding it.
    let captain = format!("{project_id}:captain");
    tx.set_seat_generation(&captain, 1)?;
    tx.set_seat_state(
        &captain,
        SeatState::Active,
        Some("bound to the existing captain pane at init"),
        now,
    )?;
    tx.insert_binding(&BindingRow {
        seat: captain,
        generation: 1,
        session: config.project.herdr_session.clone(),
        pane_id: config.captain.pane.clone(),
        kind: config.captain.kind.clone(),
        session_ref: None,
        status: BindingStatus::Active,
        handover_id: None,
        created: now,
    })?;

    Ok(InitResult {
        project: project_id,
        config_version: 1,
        seats,
    })
}

/// Apply a changed config to a registered project (T-6, design/01 "repeated
/// init: apply only changes; do not recreate seats").
///
/// - the Herdr session does not change: the agents and their bindings are in
///   it → `BadFormat` naming it;
/// - every registered lane seat is still in the config → `BadFormat` naming
///   the seat (retiring a lane is the captain's later task, not a config edit);
/// - the captain's pane and kind do not change: the captain is bound, and
///   replacing it goes through the re-registration of design/03 → `BadFormat`
///   naming the field;
/// - the project row takes the new config text, the next config version and
///   the new quotas;
/// - a lane that is not registered yet becomes a Queued seat (generation 0) —
///   the engine onboards it;
/// - a registered lane keeps its seat: its generation, state and bindings stay,
///   only its profile follows the config;
/// - a **retired** lane that is in the new config is registered again: it goes
///   back to Queued ("registered again"), so the engine onboards it once more
///   (T-54).
///
/// The plan, the tasks and the attempts are untouched: a config change is not
/// a plan change.
fn re_register(
    tx: &Tx<'_>,
    project: &ProjectRow,
    config: &config::ProjectConfig,
    config_text: &str,
    now: Ts,
) -> GearResult<InitResult> {
    let project_id = project.id.clone();

    // The session the project lives in does not change mid-run.
    if config.project.herdr_session != project.herdr_session {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "the Herdr session cannot change mid-run: the project runs in {:?}, the config says {:?}",
                project.herdr_session, config.project.herdr_session
            ),
        ));
    }

    // Every registered lane is still in the config — except a retired one: a
    // new voyage may start with new lanes once the old ones are reclaimed.
    for seat in tx.seats_of_project(&project_id)? {
        if seat.role != SeatRole::Lane || seat.state == gear_model::SeatState::Retired {
            continue;
        }
        if !config.lanes.iter().any(|lane| lane.name == seat.name) {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "the lane {} (seat {}) is not in the new config: retiring a lane is a later task",
                    seat.name, seat.id
                ),
            ));
        }
    }

    // The captain is bound: its pane and kind do not change mid-run.
    let captain_id = format!("{project_id}:captain");
    if let Some(captain) = tx.get_seat(&captain_id)? {
        if let Some(binding) = tx.get_binding(&captain.id, captain.generation)? {
            if binding.pane_id != config.captain.pane {
                return Err(GearError::new(
                    ErrorCode::BadFormat,
                    format!(
                        "the captain's pane cannot change mid-run: it is bound to {:?}, the config says {:?}",
                        binding.pane_id, config.captain.pane
                    ),
                ));
            }
            if binding.kind != config.captain.kind {
                return Err(GearError::new(
                    ErrorCode::BadFormat,
                    format!(
                        "the captain's kind cannot change mid-run: it is bound to {:?}, the config says {:?}",
                        binding.kind, config.captain.kind
                    ),
                ));
            }
        }
    }

    // The new config is the next config version.
    let version = project.config_version + 1;
    let mut row = project.clone();
    row.config_toml = config_text.to_string();
    row.config_version = version;
    row.quota_lanes = i64::from(config.quota.lanes);
    row.quota_temps = i64::from(config.quota.temps);
    row.quota_includes_captain = config.quota.includes_captain;
    tx.update_project(&row)?;

    // The roster: the captain first, then the lanes in config order.
    let mut seats: Vec<String> = vec![captain_id.clone()];
    for lane in &config.lanes {
        let seat_id = format!("{project_id}:{}", lane.name);
        match tx.get_seat(&seat_id)? {
            // A registered lane keeps its seat; only its profile follows the
            // config (the kind is read from the config when it is onboarded).
            Some(seat) => {
                if seat.role != SeatRole::Lane {
                    return Err(GearError::new(
                        ErrorCode::BadFormat,
                        format!(
                            "the lane {} would take the seat id of the {} seat {}",
                            lane.name,
                            seat.role.as_str(),
                            seat.id
                        ),
                    ));
                }
                if seat.profile != lane.profile {
                    tx.set_seat_profile(&seat_id, &lane.profile)?;
                }
                // A retired lane that is in the new config is registered again:
                // a new voyage may reuse the name, and the engine onboards
                // the seat once more (T-54).
                if seat.state == SeatState::Retired {
                    tx.set_seat_state(
                        &seat_id,
                        SeatState::Queued,
                        Some("registered again"),
                        now,
                    )?;
                }
            }
            // A new lane is registered Queued; the engine onboards it.
            None => {
                tx.insert_seat(&SeatRow {
                    id: seat_id.clone(),
                    project: project_id.clone(),
                    role: SeatRole::Lane,
                    name: lane.name.clone(),
                    profile: lane.profile.clone(),
                    generation: 0,
                    state: SeatState::Queued,
                    state_since: now,
                    state_reason: Some("registered".to_string()),
                    task_id: None,
                })?;
            }
        }
        seats.push(seat_id);
    }

    // The captain's profile follows the config as well (its binding does not).
    if let Some(captain) = tx.get_seat(&captain_id)? {
        if captain.profile != config.captain.profile {
            tx.set_seat_profile(&captain_id, &config.captain.profile)?;
        }
    }

    Ok(InitResult {
        project: project_id,
        config_version: version,
        seats,
    })
}

/// Note that `who` may start the CLI `kind` (T-83): once per kind, the first
/// seat that names it.
fn note_kind(kinds: &mut Vec<(String, String)>, kind: &str, who: &str) {
    let kind = kind.trim();
    if kind.is_empty() || kinds.iter().any(|(seen, _)| seen == kind) {
        return;
    }
    kinds.push((kind.to_string(), who.to_string()));
}

/// Note every CLI a chain may start (T-83, T-35): the entry form of a model
/// names the CLI that runs it, and a fallback may name another one — each of
/// them must have the GEAR skill too.
fn note_chain(kinds: &mut Vec<(String, String)>, chain: &[config::ModelEntry], who: &str) {
    for entry in chain {
        if let Some(cli) = entry.cli() {
            note_kind(kinds, cli, who);
        }
    }
}
