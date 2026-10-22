//! Onboarding a seat (design/03, /10): what the engine does to make a fresh
//! agent session the occupant of a seat. This module only *plans* the steps
//! (pure); executing them through Herdr is a later task.

use gear_herdr::{HerdrAct, Outcome, Profile};
use gear_model::{GearResult, SeatRole, SeatState, Ts};
use gear_store::Store;
use gear_store::SeatRow;

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
    let _ = (seat, kind, profile, generation, repo, integration, db_path, resume);
    todo!("T-58")
}

/// Onboard every **Lane** seat in state Queued, in every project (slice-3
/// walking skeleton; captain and temps are not onboarded by the engine yet).
/// Per seat, in this order:
/// 1. read the project's config (`config_toml`, `gear_app::config::parse`) for
///    `repo`, `conventions.integration` and the lane's `kind`; the profile is
///    `cfg.profiles[seat.profile]` (missing → seat NeedsCheck, reason says so);
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
        // The project's config: the repo, the integration branch and the kind
        // of every lane. A registered project always parses; if it does not,
        // its lanes are parked instead of retried.
        let row = match store.read(|tx| tx.get_project(&project))? {
            Some(row) => row,
            None => continue,
        };
        let config = match gear_app::config::parse(&row.config_toml) {
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

        for seat in store.read(|tx| tx.seats_of_project(&project))? {
            // Only lanes are onboarded by the engine in this cut; the captain
            // and the temps get their pane elsewhere.
            if seat.role != SeatRole::Lane || seat.state != SeatState::Queued {
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
    let plan = plan_onboard(
        seat,
        &lane.kind,
        profile,
        seat.generation + 1,
        &config.project.repo,
        &config.conventions.integration,
        &cfg.db_path,
        None,
    )?;
    let worktree = plan
        .worktree
        .clone()
        .ok_or_else(|| "a lane gets a worktree, but the plan has none".to_string())?;

    // 3. the worktree, with the root pane the agent starts in.
    let created = match herdr.worktree_create(
        &worktree.repo,
        &worktree.branch,
        &worktree.base,
        &worktree.label,
    ) {
        Outcome::Ok(worktree) => worktree,
        other => return Err(step_failure("worktree_create", &other)),
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
/// automatically (design/10): a human decides what happens next.
fn park(store: &mut Store, seat_id: &str, reason: &str, now: Ts) -> GearResult<()> {
    store.write(|tx| tx.set_seat_state(seat_id, SeatState::NeedsCheck, Some(reason), now))
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
}
