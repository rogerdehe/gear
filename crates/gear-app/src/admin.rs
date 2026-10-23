//! `gear admin …` (design/01 registration, /13 ops commands). User only:
//! `Env.admin` must be true and `Env.ctx` None (an agent never runs these),
//! else `Forbidden`.

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

/// `gear admin init <config>` — register the project, or apply a changed
/// config to a project that is registered already (T-6).
///
/// 1. caller check (see module doc);
/// 2. `config::parse` (errors as is) and `config::validate` — any issue →
///    `BadFormat` whose detail lists every `field: problem`;
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
pub fn init(store: &mut Store, env: &Env, config_text: &str, now: Ts) -> GearResult<InitResult> {
    // 1. only the user runs this: an agent never does.
    if env.ctx.is_some() || !env.admin {
        return Err(GearError::new(
            ErrorCode::Forbidden,
            "`gear admin init` runs in the user's own shell with GEAR_ADMIN=1, never in an agent",
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
    for (name, profile_name) in seats_to_check {
        let Some(profile) = table.get(profile_name) else {
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

    store.write(|tx| {
        match tx.get_project(&project_id)? {
            // The project is registered: the config changes.
            Some(project) => re_register(tx, &project, &config, config_text, now),
            // The project is new: it is registered.
            None => register(tx, &config, config_text, now),
        }
    })
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

    Ok(InitResult { project: project_id, config_version: 1, seats })
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
///   only its profile follows the config.
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

    // Every registered lane is still in the config.
    for seat in tx.seats_of_project(&project_id)? {
        if seat.role != SeatRole::Lane {
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

    Ok(InitResult { project: project_id, config_version: version, seats })
}
