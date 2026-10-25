//! Onboarding a seat (design/03, /10): what the engine does to make a fresh
//! agent session the occupant of a seat. This module only *plans* the steps
//! (pure); executing them through Herdr is a later task.

use gear_herdr::{HerdrAct, Outcome, Profile};
use gear_model::state::require_transition;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, Ts};
use gear_store::{SeatRow, Store};

use crate::model::{lane_primary, model_start};

/// Where a lane's long-lived worktree comes from (17 §3–§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeStep {
    pub repo: String,
    /// `home/<lane>`
    pub branch: String,
    /// the project's integration branch
    pub base: String,
    /// `<project>-<lane>`
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardPlan {
    /// Lanes only; the captain and temps get their pane elsewhere.
    pub worktree: Option<WorktreeStep>,
    /// One shell line: `export GEAR_CTX=<project>:<seat>:<gen>; export GEAR_DB=<db>`.
    pub env_command: String,
    /// `<project>-<seat name>` (17 §2), at most 32 chars.
    pub agent_name: String,
    pub kind: String,
    /// The profile's `unattended_args`, then `resume_args` with `{session}`
    /// replaced when `resume` is given.
    pub args: Vec<String>,
}

/// Plan onboarding of `seat` as generation `generation` (the next one):
/// - Lane seats get a `WorktreeStep` (branch `home/<name>`, base
///   `integration`, label `<project>-<name>`); other roles `None`;
/// - `env_command` exactly `export GEAR_CTX=<project>:<name>:<generation>; export GEAR_DB=<db_path>`;
/// - `args` = `profile.unattended_args` (+ resume args when `resume` is Some and
///   the profile has them; a resume request for a profile without resume
///   support → Err with a reason);
/// - an unverified profile, or an agent name longer than 32 chars → Err.
pub fn plan_onboard(
    seat: &SeatRow,
    kind: &str,
    profile: &Profile,
    generation: i64,
    repo: &str,
    integration: &str,
    db_path: &str,
    resume: Option<&str>,
) -> Result<OnboardPlan, String> {
    // An unverified profile may not take over a real project (design/04).
    if !profile.verified {
        return Err(format!(
            "profile {kind} is not verified, so it may not onboard a seat"
        ));
    }
    // The agent name and the workspace label are one name (17 §3).
    let agent_name = format!("{}-{}", seat.project, seat.name);
    if agent_name.chars().count() > 32 {
        return Err(format!(
            "agent name {agent_name} is longer than 32 chars"
        ));
    }
    // The agent is started unattended; a resume adds the profile's resume args
    // with the session reference in place of `{session}`.
    let mut args = profile.unattended_args.clone();
    if let Some(session) = resume {
        let resume_args = profile.resume_args.as_ref().ok_or_else(|| {
            format!("profile {kind} cannot resume a session")
        })?;
        args.extend(resume_args.iter().map(|arg| arg.replace("{session}", session)));
    }
    // Only a lane holds a long-lived worktree; the captain and the temps get
    // their pane elsewhere (17 §3-§4).
    let worktree = match seat.role {
        SeatRole::Lane => Some(WorktreeStep {
            repo: repo.to_string(),
            branch: format!("home/{}", seat.name),
            base: integration.to_string(),
            label: agent_name.clone(),
        }),
        _ => None,
    };
    Ok(OnboardPlan {
        worktree,
        env_command: format!(
            "export GEAR_CTX={}:{}:{}; export GEAR_DB={}",
            seat.project, seat.name, generation, db_path
        ),
        agent_name,
        kind: kind.to_string(),
        args,
    })
}

/// Onboard every **Lane** seat in state Queued, in every project (slice-3
/// walking skeleton; captain and temps are not onboarded by the engine yet).
///
/// The config text and the seat roster of a project are read in **one** read
/// transaction, so both see the same snapshot: a config change (T-6) is either
/// entirely visible to this tick or not at all, even when another connection
/// commits while the engine runs. A lane seat whose lane the snapshot's config
/// does not name is skipped this tick — it is never parked for a config that
/// may be about to arrive (R-63).
///
/// Per seat, in this order:
/// 1. the project's config (`config_toml`, `gear_app::config::parse`, outside
///    the read transaction) for `repo`, `conventions.integration` and the
///    lane's `kind`; the profile is `cfg.profiles[seat.profile]` (missing →
///    seat NeedsCheck, reason says so);
/// 2. `plan_onboard(seat, kind, profile, generation = seat.generation + 1, repo,
///    integration, cfg.db_path, None)` (Err → NeedsCheck with the reason);
/// 3. Herdr `worktree_create` → the root pane;
/// 4. `gear_app::seats::begin_onboard(seat, pane, kind)` in a write transaction;
/// 5. Herdr `pane_run(pane, env_command)`, then `agent_start(agent_name, kind, pane, args)`;
/// 6. agent started → `gear_app::seats::finish_onboard(seat, gen, session_ref)`.
/// Any Herdr step not `Ok` → the seat goes NeedsCheck with a reason naming the
/// step (never retried automatically). Returns the seat ids that became Active.
pub fn onboard_queued(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &crate::EngineCfg,
    now: Ts,
) -> GearResult<Vec<String>> {
    let mut active = Vec::new();
    for project in store.read(|tx| tx.project_ids())? {
        // One snapshot: the config text and the seats it is read against.
        let (config_text, seats) = store.read(|tx| {
            let config_text = tx.get_project(&project)?.map(|row| row.config_toml);
            let seats = tx.seats_of_project(&project)?;
            Ok((config_text, seats))
        })?;
        // The project is gone between the two reads: nothing to do.
        let Some(config_text) = config_text else { continue };

        // Parsing happens outside the transaction: it is pure and the read
        // snapshot is released. A registered project always parses; if it does
        // not, its lanes are parked instead of retried.
        let config = match gear_app::config::parse(&config_text) {
            Ok(config) => config,
            Err(e) => {
                park_queued_lanes(
                    store,
                    &project,
                    &format!("the project config does not parse: {}", e.detail),
                    now,
                )?;
                continue;
            }
        };
        // The profile table of this project: the built-ins with the project's
        // own `[profiles.<name>]` on top (T-31).
        let profiles = match gear_app::config::profile_table(&cfg.profiles, &config) {
            Ok(table) => table,
            Err(e) => {
                park_queued_lanes(
                    store,
                    &project,
                    &format!("the project profiles do not parse: {}", e.detail),
                    now,
                )?;
                continue;
            }
        };

        for seat in seats {
            // Only lanes are onboarded by the engine in this cut; the captain
            // and the temps get their pane elsewhere.
            if seat.role != SeatRole::Lane {
                continue;
            }
            if crate::restart::pending(store, &seat.id)?.is_some() {
                continue;
            }
            // A lane whose start is waiting on its model (T-33, F5) is resumed:
            // the switch plan is durable, so an engine restart continues it.
            let resuming =
                seat.state == SeatState::Starting && crate::switch::pending(store, &seat.id)?.is_some();
            if seat.state != SeatState::Queued && !resuming {
                continue;
            }
            // A lane the snapshot's config does not name is not parked: the
            // next tick reads a newer config (T-6, R-63).
            if !config.lanes.iter().any(|lane| lane.name == seat.name) {
                continue;
            }
            match onboard_seat(store, herdr, cfg, &seat, &config, &profiles, now) {
                // The seat is Active now.
                Ok(true) => active.push(seat.id.clone()),
                // The start waits for the model switch: the next tick continues.
                Ok(false) => {}
                // A failed step parks the seat; it is never retried blindly.
                Err(reason) => park(store, &seat.id, &reason, now)?,
            }
        }
    }
    Ok(active)
}

/// Onboard one queued lane: the plan, the worktree with its root pane, the
/// seat's binding, the environment line and the agent. `Err(reason)` names the
/// step that failed.
fn onboard_seat(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &crate::EngineCfg,
    seat: &SeatRow,
    config: &gear_app::config::ProjectConfig,
    profiles: &std::collections::BTreeMap<String, gear_herdr::Profile>,
    now: Ts,
) -> Result<bool, String> {
    // 1. the kind of the lane comes from the project config; the profile is
    //    the one that drives it.
    let lane = config
        .lanes
        .iter()
        .find(|lane| lane.name == seat.name)
        .ok_or_else(|| format!("lane {} is not in the project config", seat.name))?;
    // The chain entry of the lane's model: its own chain first, else the lane
    // chain (17 §0).
    let entry = lane_primary(lane, &config.models.lanes);
    // The CLI that runs the model (T-35): the entry names it, else the lane's
    // own kind. The profile is the one that drives that CLI.
    let kind = entry
        .and_then(|entry| entry.cli())
        .unwrap_or(lane.kind.as_str())
        .to_string();
    let selected = if kind == lane.kind {
        profiles.get(&seat.profile).map(|profile| (seat.profile.as_str(), profile))
    } else {
        crate::restart::profile_for_kind(profiles, &kind)
    };
    let (profile_name, profile) = selected
        .ok_or_else(|| format!("no profile drives the CLI {kind:?}"))?;

    // 2. the plan: worktree, environment line, agent name and start args.
    let mut plan = plan_onboard(
        seat,
        &kind,
        profile,
        seat.generation + 1,
        &config.project.repo,
        &config.conventions.integration,
        &cfg.db_path,
        None,
    )?;
    // The lane's model goes into the environment line, so the agent GEAR
    // starts runs on the model GEAR chose (17 §0), and the entry's own
    // arguments follow the model flag (T-35).
    if let Some(entry) = entry {
        if let Some(start) = model_start(&kind, entry.model()) {
            plan.env_command = format!("{}; {}", plan.env_command, start.env);
            plan.args.extend(start.args);
        }
        plan.args.extend(entry.args().iter().cloned());
    }
    let worktree = plan
        .worktree
        .clone()
        .ok_or_else(|| "a lane gets a worktree, but the plan has none".to_string())?;

    // A start that is waiting on its model (T-33, F5): continue the switch and
    // finish onboarding only once the status bar confirms.
    if seat.state == SeatState::Starting
        && crate::switch::pending(store, &seat.id).map_err(|e| e.detail)?.is_some()
    {
        return resume_start(store, herdr, seat, profile, config, now);
    }

    // 3. the worktree, with the root pane the agent starts in. A lane's home
    // worktree is long-lived (17 §3): reopen it when its branch exists (a
    // restart, or a lane GEAR takes over), create it otherwise.
    let created = match herdr.worktree_open(&worktree.repo, &worktree.branch, &worktree.label) {
        Outcome::Ok(worktree) => worktree,
        Outcome::Refused { code, .. } if code == "worktree_not_found" => match herdr.worktree_create(
            &worktree.repo,
            &worktree.branch,
            &worktree.base,
            &worktree.label,
        ) {
            Outcome::Ok(worktree) => worktree,
            other => return Err(step_failure("worktree_create", &other)),
        },
        other => return Err(step_failure("worktree_open", &other)),
    };
    let pane = created.pane_id;
    // The worktree's path is kept: a fallback to another CLI may have to trust
    // it (grok asks before its first start in a directory, T-32).
    store
        .write(|tx| tx.set_meta(&worktree_path_key(&seat.id), &created.path))
        .map_err(|e| format!("worktree path: {}", e.detail))?;

    // 4. the seat moves to Starting with a pending binding for the pane. The
    //    seat's profile follows the CLI that runs its model (T-35).
    let generation = store
        .write(|tx| {
            let generation = gear_app::seats::begin_onboard(tx, &seat.id, &pane, &kind, now)?;
            tx.set_seat_profile(&seat.id, profile_name)?;
            Ok(generation)
        })
        .map_err(|e| format!("begin_onboard: {}", e.detail))?;

    if kind == "opencode" {
        if let Some(entry) = entry {
            plan.args.extend(crate::opencode::start_args(
                store, herdr, &seat.id, generation, &created.path, config, entry.model(),
            )?);
        }
    }

    // 5. the environment in the pane, then the agent itself.
    match herdr.pane_run(&pane, &plan.env_command) {
        Outcome::Ok(()) => {}
        other => return Err(step_failure("pane_run", &other)),
    }
    // A grok worktree is trusted before its agent starts: grok asks on the
    // first start, while Herdr reports it idle, and the prompt would be lost.
    crate::trust::trust_worktree(&plan.kind, &created.path, now)
        .map_err(|reason| format!("trust: {reason}"))?;
    // The shell may still be running the environment line when the start
    // comes (a slow shell init): Herdr then refuses with agent_pane_busy.
    // That is a race, not a failure — wait a moment and try again.
    let mut tries = 0;
    let started = loop {
        match herdr.agent_start(&plan.agent_name, &plan.kind, &pane, &plan.args) {
            Outcome::Ok(info) => break info,
            Outcome::Refused { code, .. } if code == "agent_pane_busy" && tries < PANE_BUSY_RETRIES => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(PANE_BUSY_WAIT_MS));
            }
            other => return Err(step_failure("agent_start", &other)),
        }
    };

    // 6. an OpenCode agent runs on the chain's model before the seat is marked
    //    started (T-33, F5): the shared service ignores the start environment,
    //    so the status bar decides here. Waiting keeps the start pending.
    if plan.kind == "opencode" {
        if let Some(entry) = lane_primary(lane, &config.models.lanes) {
            match crate::switch::ensure_model(store, herdr, seat, profile, config, entry.model(), now)
                .map_err(|e| format!("model check: {}", e.detail))?
            {
                crate::switch::ModelGate::Ready => {}
                crate::switch::ModelGate::Waiting => return Ok(false),
                crate::switch::ModelGate::Failed(reason) => {
                    // T-60: the lane cannot run on the chain's model — it is
                    // parked in NeedsCheck below, so the captain is rung.
                    crate::quota::alert_captain(store, &seat.project, seat, &reason, now)
                        .map_err(|e| e.detail)?;
                    return Err(reason);
                }
            }
        }
    }

    // 7. the agent is there on the right model: the binding becomes Active and
    //    so does the seat. The model the seat runs on is recorded: the review
    //    must never run the writer's current model (T-35).
    let api_session = store.read(|tx| tx.get_binding(&seat.id, generation))
        .map_err(|e| e.detail)?.and_then(|binding| binding.session_ref);
    let session_ref = api_session.as_deref()
        .or_else(|| started.agent_session.as_ref().map(|session| session.value.as_str()));
    store
        .write(|tx| {
            gear_app::seats::finish_onboard(tx, &seat.id, generation, session_ref, now)?;
            if let Some(entry) = lane_primary(lane, &config.models.lanes) {
                tx.set_meta(&crate::model::model_meta_key(&seat.id), entry.model())?;
            }
            Ok(())
        })
        .map_err(|e| format!("finish_onboard: {}", e.detail))?;
    Ok(true)
}

/// Continue a lane's pending model switch and finish its onboarding when the
/// status bar confirms (T-33, F5): no task is dispatched before that.
fn resume_start(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &SeatRow,
    profile: &gear_herdr::Profile,
    config: &gear_app::config::ProjectConfig,
    now: Ts,
) -> Result<bool, String> {
    let Some(plan) = crate::switch::pending(store, &seat.id).map_err(|e| e.detail)? else {
        return Ok(false);
    };
    let Some((model, labels)) = config
        .models
        .switch
        .iter()
        .find(|(_, labels)| labels.bar == plan.to)
        .map(|(model, labels)| (model.clone(), labels.clone()))
    else {
        crate::switch::clear_pending(store, &seat.id).map_err(|e| e.detail)?;
        let reason = format!(
            "the switch target {:?} has no [models.switch] labels any more",
            plan.to
        );
        // T-60: the lane stops here — the captain is rung.
        crate::quota::alert_captain(store, &seat.project, seat, &reason, now).map_err(|e| e.detail)?;
        return Err(reason);
    };
    match crate::switch::switch_model(store, herdr, &seat.id, profile, None, &labels, &model, now)
        .map_err(|e| format!("model switch: {}", e.detail))?
    {
        crate::switch::SwitchOutcome::Done | crate::switch::SwitchOutcome::Already => {}
        crate::switch::SwitchOutcome::Waiting => return Ok(false),
        crate::switch::SwitchOutcome::Failed => {
            let reason = format!("the model switch to {:?} did not confirm twice", plan.to);
            // T-60: the lane stops here — the captain is rung.
            crate::quota::alert_captain(store, &seat.project, seat, &reason, now)
                .map_err(|e| e.detail)?;
            return Err(reason);
        }
    }
    // The session reference the start reported, when Herdr still shows it.
    let pane = store
        .read(|tx| tx.get_binding(&seat.id, seat.generation))
        .map_err(|e| e.detail)?
        .map(|binding| binding.pane_id);
    let api_session = store.read(|tx| tx.get_binding(&seat.id, seat.generation))
        .map_err(|e| e.detail)?.and_then(|binding| binding.session_ref);
    let session_ref = api_session.or_else(|| pane
        .and_then(|pane| herdr.agent_get(&pane).ok().flatten())
        .and_then(|info| info.agent_session.map(|session| session.value)));
    store
        .write(|tx| {
            gear_app::seats::finish_onboard(tx, &seat.id, seat.generation, session_ref.as_deref(), now)
        })
        .map_err(|e| format!("finish_onboard: {}", e.detail))?;
    Ok(true)
}

/// How often a start refused because the shell is still busy is retried, and
/// how long to wait between tries.
const PANE_BUSY_RETRIES: u32 = 5;
const PANE_BUSY_WAIT_MS: u64 = 1000;

/// The key of a seat's worktree path in the engine's meta (the seat row has
/// no field for it).
pub(crate) fn worktree_path_key(seat: &str) -> String {
    format!("engine.worktree_path.{seat}")
}

/// Park a seat in NeedsCheck with the reason. It is never retried
/// automatically (design/10): a human decides what happens next. The
/// transition is checked like every other seat transition.
fn park(store: &mut Store, seat_id: &str, reason: &str, now: Ts) -> GearResult<()> {
    store.write(|tx| {
        let seat = tx.get_seat(seat_id)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("no seat {seat_id}"))
        })?;
        require_transition(
            SeatState::can_transition(seat.role, seat.state, SeatState::NeedsCheck),
            "seat",
            seat.state.as_str(),
            SeatState::NeedsCheck.as_str(),
        )?;
        tx.set_seat_state(seat_id, SeatState::NeedsCheck, Some(reason), now)
    })
}

/// Park every queued lane of a project that cannot be onboarded.
fn park_queued_lanes(store: &mut Store, project: &str, reason: &str, now: Ts) -> GearResult<()> {
    let queued: Vec<String> = store
        .read(|tx| tx.seats_of_project(project))?
        .into_iter()
        .filter(|seat| seat.role == SeatRole::Lane && seat.state == SeatState::Queued)
        .map(|seat| seat.id)
        .collect();
    for seat_id in queued {
        park(store, &seat_id, reason, now)?;
    }
    Ok(())
}

/// Why a Herdr step did not succeed; the step is named in the reason.
fn step_failure<T>(step: &str, outcome: &Outcome<T>) -> String {
    match outcome {
        Outcome::Refused { code, message } => {
            format!("{step}: refused ({code}): {message}")
        }
        Outcome::Uncertain { reason } => format!("{step}: uncertain: {reason}"),
        Outcome::Ok(_) => format!("{step}: succeeded"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_store::ProjectRow;

    const T0: i64 = 1_800_000_000;

    const CONFIG: &str = r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
"#;

    /// The same project with a lane chain: `minimax-cn/MiniMax-M3.1-Flash`
    /// first, `deepseek/deepseek-flash` as the fallback.
    const CONFIG_WITH_MODELS: &str = r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash"]

[models.switch."minimax-cn/MiniMax-M3.1-Flash"]
picker = "MiniMax-M3.1-Flash"
bar = "MiniMax-M3.1-Flash"
"#;

    /// The same project, but the lane brings its own chain.
    const CONFIG_WITH_LANE_MODELS: &str = r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
models = ["deepseek/deepseek-v4-pro"]

[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash"]

[models.switch."deepseek/deepseek-v4-pro"]
picker = "DeepSeek V4 Pro"
bar = "DeepSeek V4 Pro"

[models.switch."minimax-cn/MiniMax-M3.1-Flash"]
picker = "MiniMax-M3.1-Flash"
bar = "MiniMax-M3.1-Flash"
"#;

    /// An engine that knows the opencode profile.
    fn cfg() -> crate::EngineCfg {
        let mut cfg = crate::EngineCfg { db_path: "/state/gear.db".into(), ..Default::default() };
        cfg.profiles.insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
        cfg
    }

    /// The environment line the engine ran in the pane.
    fn env_line(herdr: &FakeHerdr) -> String {
        herdr
            .calls
            .iter()
            .find_map(|call| match call {
                gear_herdr::fake::Call::PaneRun { command, .. } => Some(command.clone()),
                _ => None,
            })
            .expect("the engine ran the environment line")
    }

    /// The start args the engine started the agent with.
    fn start_args(herdr: &FakeHerdr) -> Vec<String> {
        herdr
            .calls
            .iter()
            .find_map(|call| match call {
                gear_herdr::fake::Call::AgentStart { args, .. } => Some(args.clone()),
                _ => None,
            })
            .expect("the engine started the agent")
    }

    /// A project with one registered lane and the given config text.
    fn world(config_toml: &str) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&ProjectRow {
                    id: "toy".into(),
                    herdr_session: "sess".into(),
                    config_toml: config_toml.into(),
                    config_version: 1,
                    quota_lanes: 1,
                    quota_temps: 0,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: T0,
                })?;
                tx.insert_seat(&SeatRow {
                    id: "toy:heat".into(),
                    project: "toy".into(),
                    role: SeatRole::Lane,
                    name: "heat".into(),
                    profile: "opencode".into(),
                    generation: 0,
                    state: SeatState::Queued,
                    state_since: T0,
                    state_reason: Some("registered".into()),
                    task_id: None,
                })?;
                Ok(())
            })
            .unwrap();
        store
    }

    #[test]
    fn api_start_records_session_before_tui_and_sends_no_model_keys() {
        let mut store = world(CONFIG_WITH_MODELS);
        let mut herdr = FakeHerdr::new();
        herdr.start_screen = Some("┃ Build auto · MiniMax-M3.1-Flash".into());
        let model = serde_json::json!({"providerID":"minimax-cn", "id":"MiniMax-M3.1-Flash"});
        herdr.api_replies.extend([
            Ok(serde_json::json!({"data":[{"providerID":"minimax-cn", "id":"MiniMax-M3.1-Flash", "variants":[]}]})),
            Ok(serde_json::json!({"data":{"id":"ses_api_start", "model":model}})),
            Ok(serde_json::json!({"data":[{"providerID":"minimax-cn", "id":"MiniMax-M3.1-Flash", "variants":[]}]})),
            Ok(serde_json::json!({"data":{"id":"ses_api_start", "model":model}})),
        ]);
        assert_eq!(onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap(), ["toy:heat"]);
        assert_eq!(start_args(&herdr), ["--auto", "--session", "ses_api_start"]);
        let binding = store.read(|tx| tx.get_binding("toy:heat", 1)).unwrap().unwrap();
        assert_eq!(binding.session_ref.as_deref(), Some("ses_api_start"));
        assert!(!herdr.calls.iter().any(|call| matches!(call,
            gear_herdr::fake::Call::SendKeys { .. } | gear_herdr::fake::Call::PaneType { .. })));
        let create = herdr.calls.iter().find_map(|call| match call {
            gear_herdr::fake::Call::OpenCodeApi { args } if args[0] == "session.create" => Some(args),
            _ => None,
        }).unwrap();
        let body: serde_json::Value = serde_json::from_str(&create[2]).unwrap();
        assert_eq!(body["agent"], "build");
        assert_eq!(body["model"], model);
        let path = store.read(|tx| tx.get_meta(&worktree_path_key("toy:heat"))).unwrap().unwrap();
        assert_eq!(body["location"]["directory"], path);
    }

    #[test]
    fn a_lane_without_a_profile_is_parked_in_needs_check() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        // No profile is configured, so the seat never reaches Herdr.
        let active =
            onboard_queued(&mut store, &mut herdr, &crate::EngineCfg::default(), T0 + 1).unwrap();

        assert!(active.is_empty(), "no seat became active");
        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::NeedsCheck);
        let reason = seat.state_reason.unwrap_or_default();
        assert!(reason.contains("opencode"), "{reason}");
        assert!(herdr.calls.is_empty(), "nothing was done in Herdr: {:?}", herdr.calls);
    }

    /// The shell still runs the environment line when the start comes: Herdr
    /// refuses with agent_pane_busy, and the start is tried again, not parked.
    #[test]
    fn a_busy_shell_at_start_is_retried() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        herdr.refuse_next_start = Some("agent_pane_busy".into());

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(active, vec!["toy:heat".to_string()]);
        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active);
    }

    #[test]
    fn a_seat_that_is_not_queued_is_left_alone() {
        let mut store = world(CONFIG);
        store
            .write(|tx| tx.set_seat_state("toy:heat", SeatState::Active, Some("on"), T0 + 1))
            .unwrap();
        let mut herdr = FakeHerdr::new();

        let active =
            onboard_queued(&mut store, &mut herdr, &crate::EngineCfg::default(), T0 + 2).unwrap();

        assert!(active.is_empty());
        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active, "an active seat is never onboarded again");
    }

    #[test]
    fn the_lane_model_is_carried_by_the_environment_line() {
        let mut store = world(CONFIG_WITH_MODELS);
        let mut herdr = FakeHerdr::new();
        // An OpenCode lane must start on the chain's model (T-33, F5).
        herdr.start_screen = Some("Build auto · MiniMax-M3.1-Flash MiniMax · high".into());

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(active, vec!["toy:heat".to_string()], "state {:?}: {:?}", seat.state, seat.state_reason);
        let env = env_line(&herdr);
        assert!(
            env.contains(r#"OPENCODE_CONFIG_CONTENT='{"model":"minimax-cn/MiniMax-M3.1-Flash"}'"#),
            "the environment line must carry the lane's model: {env}"
        );
        assert!(env.starts_with("export GEAR_CTX=toy:heat:1; export GEAR_DB=/state/gear.db"), "{env}");
        // The model is only read by a process that inherits the pane's
        // environment, so the agent gets a private server.
        assert!(!start_args(&herdr).contains(&"--standalone".to_string()), "no --standalone (L-56): {:?}", start_args(&herdr));
    }

    /// T-33 (F5): a lane that starts on the wrong model is switched in place
    /// before it is marked Active; no work is dispatched until the bar says so.
    #[test]
    fn a_lane_that_starts_on_the_wrong_model_switches_before_it_is_active() {
        let mut store = world(CONFIG_WITH_MODELS);
        let mut herdr = FakeHerdr::new();
        // The agent comes up on another model.
        herdr.start_screen = Some("Build auto · Flint-40B Free OpenCode · high".to_string());
        herdr.screen_after = vec![
            ("esc".into(), "  Ask anything…".into()),
            (
                "/model".into(),
                "  Search\n  Recent\n  MiniMax-M3.1-Flash  MiniMax".into(),
            ),
            ("MiniMax-M3.1-Flash".into(), "Build auto · MiniMax-M3.1-Flash MiniMax · high".into()),
        ];

        // Tick 1: the agent starts, the check finds another model, esc is sent.
        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();
        assert!(active.is_empty(), "the seat is not started yet");
        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Starting, "the start stays pending");
        assert!(crate::switch::pending(&mut store, "toy:heat").unwrap().is_some());

        // The next ticks continue the switch; the seat becomes Active only when
        // the bar confirms.
        let mut active = Vec::new();
        for tick in 0..5 {
            active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 2 + tick).unwrap();
            if !active.is_empty() {
                break;
            }
        }
        assert_eq!(active, vec!["toy:heat".to_string()]);
        let sent: Vec<String> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                gear_herdr::fake::Call::PaneType { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(sent.iter().filter(|c| *c == "/model").count(), 1, "{sent:?}");
        assert!(sent.iter().any(|c| c == "MiniMax-M3.1-Flash"), "{sent:?}");
        assert!(herdr.started_agents.len() == 1, "the agent is started once: {:?}", herdr.started_agents);
    }

    /// T-33 (F5): a lane already on the chain's model sends nothing.
    #[test]
    fn a_lane_already_on_its_model_is_not_touched() {
        let mut store = world(CONFIG_WITH_MODELS);
        let mut herdr = FakeHerdr::new();
        herdr.start_screen = Some("Build auto · MiniMax-M3.1-Flash MiniMax · high".to_string());

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(active, vec!["toy:heat".to_string()]);
        let sent: Vec<String> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                gear_herdr::fake::Call::PaneType { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(!sent.iter().any(|c| c == "/model"), "nothing typed into the agent: {sent:?}");
    }

    #[test]
    fn the_lane_own_model_wins_over_the_lane_chain() {
        let mut store = world(CONFIG_WITH_LANE_MODELS);
        let mut herdr = FakeHerdr::new();
        herdr.start_screen = Some("Build auto · DeepSeek V4 Pro DeepSeek · high".into());

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(active, vec!["toy:heat".to_string()]);
        let env = env_line(&herdr);
        assert!(
            env.contains(r#"OPENCODE_CONFIG_CONTENT='{"model":"deepseek/deepseek-v4-pro"}'"#),
            "the lane's own model must win: {env}"
        );
        assert!(!env.contains("MiniMax-M3.1-Flash"), "the lane chain must not be used: {env}");
    }

    #[test]
    fn without_a_model_chain_the_environment_line_stays_as_it_was() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(active, vec!["toy:heat".to_string()]);
        assert_eq!(env_line(&herdr), "export GEAR_CTX=toy:heat:1; export GEAR_DB=/state/gear.db");
        assert_eq!(start_args(&herdr), vec!["--auto".to_string()]);
    }
}
