//! Onboarding a seat (design/03, /10): what the engine does to make a fresh
//! agent session the occupant of a seat. This module only *plans* the steps
//! (pure); executing them through Herdr is a later task.

use gear_app::config::LaneSection;
use gear_herdr::{HerdrAct, Outcome, Profile};
use gear_model::state::require_transition;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, Ts};
use gear_store::{SeatRow, Store};

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

/// The primary model of a lane (17 §0): the first entry of the lane's own
/// chain, else the first entry of the lane chain (`[models].lanes`). `None`
/// when no chain is configured — then GEAR does not manage the model.
fn lane_primary<'a>(lane: &'a LaneSection, lane_chain: &'a [String]) -> Option<&'a str> {
    lane.models
        .first()
        .or_else(|| lane_chain.first())
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
}

/// How a CLI takes the model it starts on.
struct ModelStart {
    /// The shell fragment that carries the model in the environment line.
    env: String,
    /// Start args the model needs.
    args: Vec<String>,
}

/// The model of a lane, as the CLI gets it.
///
/// OpenCode has no `--model` flag: it takes the model from its config, and
/// the only config source a *client* can carry is the environment variable
/// `OPENCODE_CONFIG_CONTENT` (a JSON object). Verified 2026-10-22 on OpenCode
/// v2.1.2 — see `skills/gear/references/agents/opencode.md`.
///
/// The variable is read by the process that loads the config. Against the
/// shared background service that is the service, whose environment was fixed
/// when it started, so the model would be ignored; a private server
/// (`--standalone`) reads the environment of the pane it is started in, which
/// is where GEAR exports the model. A CLI without a verified way gets
/// nothing: its model is not managed by GEAR.
fn model_start(kind: &str, model: &str) -> Option<ModelStart> {
    match kind {
        "opencode" => Some(ModelStart {
            env: format!("export OPENCODE_CONFIG_CONTENT='{{\"model\":\"{model}\"}}'"),
            args: vec!["--standalone".to_string()],
        }),
        _ => None,
    }
}

/// Onboard every **Lane** seat in state Queued, in every project (slice-3
/// walking skeleton; captain and temps are not onboarded by the engine yet).
///
/// The config text and the seat roster of a project are read in **one** read
/// transaction: a config change (T-6) commits between ticks, never between the
/// two reads, so every lane in the roster is in the config of the same
/// snapshot. A lane seat whose lane is not in the config is skipped this tick —
/// it is never parked for a config that may be about to arrive (R-63).
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

        for seat in seats {
            // Only lanes are onboarded by the engine in this cut; the captain
            // and the temps get their pane elsewhere.
            if seat.role != SeatRole::Lane || seat.state != SeatState::Queued {
                continue;
            }
            // A lane the snapshot's config does not name is not parked: the
            // next tick reads a newer config (T-6, R-63).
            if !config.lanes.iter().any(|lane| lane.name == seat.name) {
                continue;
            }
            match onboard_seat(store, herdr, cfg, &seat, &config, now) {
                // The seat is Active now.
                Ok(()) => active.push(seat.id.clone()),
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
    now: Ts,
) -> Result<(), String> {
    // 1. the kind of the lane comes from the project config; the profile is
    //    the one the seat was registered with.
    let lane = config
        .lanes
        .iter()
        .find(|lane| lane.name == seat.name)
        .ok_or_else(|| format!("lane {} is not in the project config", seat.name))?;
    let profile = cfg
        .profiles
        .get(&seat.profile)
        .ok_or_else(|| format!("profile {} is not configured", seat.profile))?;

    // 2. the plan: worktree, environment line, agent name and start args.
    let mut plan = plan_onboard(
        seat,
        &lane.kind,
        profile,
        seat.generation + 1,
        &config.project.repo,
        &config.conventions.integration,
        &cfg.db_path,
        None,
    )?;
    // The lane's model goes into the environment line, so the agent GEAR
    // starts runs on the model GEAR chose (17 §0): the lane's own chain
    // first, else the lane chain.
    if let Some(model) = lane_primary(lane, &config.models.lanes) {
        if let Some(start) = model_start(&lane.kind, model) {
            plan.env_command = format!("{}; {}", plan.env_command, start.env);
            plan.args.extend(start.args);
        }
    }
    let worktree = plan
        .worktree
        .clone()
        .ok_or_else(|| "a lane gets a worktree, but the plan has none".to_string())?;

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

    // 4. the seat moves to Starting with a pending binding for the pane.
    let generation = store
        .write(|tx| gear_app::seats::begin_onboard(tx, &seat.id, &pane, &lane.kind, now))
        .map_err(|e| format!("begin_onboard: {}", e.detail))?;

    // 5. the environment in the pane, then the agent itself.
    match herdr.pane_run(&pane, &plan.env_command) {
        Outcome::Ok(()) => {}
        other => return Err(step_failure("pane_run", &other)),
    }
    let started = match herdr.agent_start(&plan.agent_name, &plan.kind, &pane, &plan.args) {
        Outcome::Ok(info) => info,
        other => return Err(step_failure("agent_start", &other)),
    };

    // 6. the agent is there: the binding becomes Active and so does the seat.
    let session_ref = started.agent_session.as_ref().map(|session| session.value.as_str());
    store
        .write(|tx| gear_app::seats::finish_onboard(tx, &seat.id, generation, session_ref, now))
        .map_err(|e| format!("finish_onboard: {}", e.detail))?;
    Ok(())
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

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(active, vec!["toy:heat".to_string()]);
        let env = env_line(&herdr);
        assert!(
            env.contains(r#"OPENCODE_CONFIG_CONTENT='{"model":"minimax-cn/MiniMax-M3.1-Flash"}'"#),
            "the environment line must carry the lane's model: {env}"
        );
        assert!(env.starts_with("export GEAR_CTX=toy:heat:1; export GEAR_DB=/state/gear.db"), "{env}");
        // The model is only read by a process that inherits the pane's
        // environment, so the agent gets a private server.
        assert!(start_args(&herdr).contains(&"--standalone".to_string()), "{:?}", start_args(&herdr));
    }

    #[test]
    fn the_lane_own_model_wins_over_the_lane_chain() {
        let mut store = world(CONFIG_WITH_LANE_MODELS);
        let mut herdr = FakeHerdr::new();

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
