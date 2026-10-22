//! `gear admin …` (design/01 registration, /13 ops commands). User only:
//! `Env.admin` must be true and `Env.ctx` None (an agent never runs these),
//! else `Forbidden`.

use crate::config;
use crate::Env;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, Ts};
use gear_store::{ProjectRow, SeatRow, Store};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InitResult {
    pub project: String,
    pub config_version: i64,
    /// Seat ids registered by this init (captain first, then lanes in config order).
    pub seats: Vec<String>,
}

/// `gear admin init <config>` — first registration of a project (slice 3, first
/// cut; re-init with changes comes later):
/// 1. caller check (see module doc);
/// 2. `config::parse` (errors as is) and `config::validate` — any issue →
///    `BadFormat` whose detail lists every `field: problem`;
/// 3. project already in the store → `BadFormat` "already registered";
/// 4. one transaction: `insert_project` (config text verbatim, config_version 1,
///    quotas from `[quota]`, plan_version 0) and one seat per role —
///    `<id>:captain` (Captain, profile = `captain.profile`) and `<id>:<lane>`
///    for each lane (Lane, its profile) — all generation 0, state Queued,
///    `state_reason` "registered". Onboarding (worktree, pane, agent) is the
///    engine's job afterwards, not init's.
/// Checks that need Herdr or git (session, repo, branch) are not part of this cut.
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

    store.write(|tx| {
        // 3. a project is registered once.
        if tx.get_project(&project_id)?.is_some() {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!("project {project_id} is already registered"),
            ));
        }

        // 4. the project row and one seat per role; onboarding is the
        // engine's job afterwards.
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
            })?;
            seats.push(seat_id);
            Ok(())
        };

        register("captain", SeatRole::Captain, &config.captain.profile)?;
        for lane in &config.lanes {
            register(&lane.name, SeatRole::Lane, &lane.profile)?;
        }

        Ok(InitResult { project: project_id, config_version: 1, seats })
    })
}
