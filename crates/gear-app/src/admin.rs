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
    /// T-373: one display line per seat whose worktree the **old**
    /// `conventions.seat_branch` made and the new config no longer uses —
    /// empty when the `seat_branch` did not change. Nothing is removed: the
    /// line names the path and the command that takes it away.
    #[serde(default)]
    pub stale_worktrees: Vec<String>,
}

/// The seat that may register a project: the user's own shell (no ctx), or
/// the project's captain (T-86).
const CAPTAIN: &str = "captain";

/// The meta key that remembers which file the applied config came from
/// (T-321): `config.path.<project>`. The store keeps the applied text
/// (`projects.config_toml`) and its version, not the path; the home page needs
/// the path to tell a config file that drifted from the config the project
/// runs on. No schema change: the path is a meta row.
pub fn config_path_key(project: &str) -> String {
    format!("config.path.{project}")
}

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
    user: Option<&str>,
    now: Ts,
) -> GearResult<InitResult> {
    init_with_home(store, env, config_text, None, user, now)
}

/// `gear admin init` with an explicit home for the skill check (T-83): the CLI
/// passes the user's `HOME`; a test passes a scratch home with the skills
/// installed. `None` means the process's `HOME`.
pub fn init_with_home(
    store: &mut Store,
    env: &Env,
    config_text: &str,
    home: Option<&std::path::Path>,
    user: Option<&str>,
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

    // 2. everything that needs no store: the config itself, the profiles and
    //    the GEAR skills (`check`, T-228). The other projects of the branch
    //    check (T-256) are this store's, so `register`/`re_register` read them
    //    inside the transaction, before any write.
    let config = check(config_text, home)?;

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

    store.write(|tx| {
        match tx.get_project(&project_id)? {
            // The project is registered: the config changes.
            Some(project) => re_register(tx, &project, &config, config_text, user, now),
            // The project is new: it is registered.
            None => register(tx, &config, config_text, now),
        }
    })
}

/// The checks `init` makes before it touches the store (T-228): the config's
/// shape and every issue at once, each seat's profile, and the GEAR skill of
/// every kind the project can start. A caller that only wants to validate its
/// `gear.toml` runs this: it opens no store, reaches no Herdr and starts
/// nothing. The caller-context checks (`init`'s step 2b) need the store and
/// stay there, and the cross-project branch check (T-256) is [`branch_clash`],
/// which the caller feeds its own projects.
pub fn check(
    config_text: &str,
    home: Option<&std::path::Path>,
) -> GearResult<config::ProjectConfig> {
    // Parse and validate the config; every issue is reported at once.
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

    Ok(config)
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
    user: Option<&str>,
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
    init_with_home(store, env, config_text, home, user, now)
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

/// The branch check of a registration (T-256), against every other project of
/// the store.
fn check_other_projects(tx: &Tx<'_>, config: &config::ProjectConfig) -> GearResult<()> {
    match branch_clash(config, &other_projects(tx)?) {
        Some(problem) => Err(GearError::new(ErrorCode::BadFormat, problem)),
        None => Ok(()),
    }
}

/// Every project of the store as `(id, config_toml)` (T-256).
fn other_projects(tx: &Tx<'_>) -> GearResult<Vec<(String, String)>> {
    let mut others = Vec::new();
    for id in tx.project_ids()? {
        if let Some(row) = tx.get_project(&id)? {
            others.push((row.id, row.config_toml));
        }
    }
    Ok(others)
}

/// The problem of a config that would run more lanes than `[quota] lanes`
/// allows (T-304), or `None` when it fits.
///
/// The quota counts the lanes that can run: a lane seat that is Retired does
/// not take a slot, and a config lane that has no seat yet does (the engine
/// will onboard it). `retired` are the lane names this project's seats hold as
/// `SeatState::Retired`; a first registration has none.
pub fn lane_quota_problem(
    config: &config::ProjectConfig,
    retired: &[String],
) -> Option<String> {
    let running = config
        .lanes
        .iter()
        .filter(|lane| !retired.iter().any(|name| *name == lane.name))
        .count();
    if running as u32 > config.quota.lanes {
        return Some(format!(
            "quota.lanes: {running} lanes would run but the quota allows {} (a retired lane does not count)",
            config.quota.lanes
        ));
    }
    None
}

/// Every lane name this project's seats hold as `Retired` (T-304).
fn retired_lane_names(tx: &Tx<'_>, project: &str) -> GearResult<Vec<String>> {
    let mut names = Vec::new();
    for seat in tx.seats_of_project(project)? {
        if seat.role == SeatRole::Lane && seat.state == SeatState::Retired {
            names.push(seat.name);
        }
    }
    Ok(names)
}

/// The problem of two projects sharing branches, voyage folders or GitHub
/// issues (T-256, T-302), or `None` when they cannot collide.
///
/// `others` are the store's projects as `(id, config_toml)`; one that does not
/// parse is skipped, since it says nothing about what it cuts. Four checks, in
/// this order:
///
/// - **GitHub issues** (T-302): whatever the local paths, two repos of the two
///   configs that read and write the same `owner/name` (case-insensitively)
///   take the same issues unless each project selects them with non-empty,
///   disjoint `[github.issues] labels` — hence [`github_clash`], first.
/// - **shared repository**: a project of `others` that works in none of this
///   config's repositories (canonicalized, the text when it does not exist) is
///   no partner of this config at all.
/// - **branches** (T-256): on a shared repository this config's task, review
///   and seat branch templates must name `{project}`, so two fleets in one
///   Herdr session keep `hull/t12` and `review/t12` to themselves.
/// - **voyage folder** (T-302): on a shared repository the two projects must
///   not expand `conventions.voyage_dir` to the same folder, or their plans,
///   reports and reviews overwrite each other. The default
///   `gear/voyages/{milestone}` names no project.
pub fn branch_clash(
    config: &config::ProjectConfig,
    others: &[(String, String)],
) -> Option<String> {
    let ours = repo_paths(config);
    let our_repos = config.repos();
    for (other_id, other_text) in others {
        if *other_id == config.project.id {
            continue;
        }
        let Ok(other) = config::parse(other_text) else {
            continue;
        };
        // (a) The GitHub check comes first: two projects need not share a
        // local path to take the same `owner/name` issues.
        if let Some(problem) = github_clash(other_id, &our_repos, &other) {
            return Some(problem);
        }
        let theirs = repo_paths(&other);
        let Some(path) = ours.iter().find(|path| theirs.contains(path)) else {
            continue;
        };
        let conventions = &config.conventions;
        let templated = [
            &conventions.task_branch,
            &conventions.review_branch,
            &conventions.seat_branch,
        ]
        .iter()
        .all(|template| template.contains("{project}"));
        if !templated {
            return Some(format!(
                "project {other_id} also works in {path}: put {{project}} in conventions.task_branch, review_branch and seat_branch so the two projects' branches cannot collide"
            ));
        }
        let our_voyages = gear_model::layout::voyage_dir(
            &conventions.voyage_dir,
            &config.project.id,
            "{milestone}",
        );
        let their_voyages = gear_model::layout::voyage_dir(
            &other.conventions.voyage_dir,
            &other.project.id,
            "{milestone}",
        );
        if our_voyages == their_voyages {
            return Some(format!(
                "project {other_id} also works in {path} and keeps its voyages in the same folder {our_voyages}: put {{project}} in conventions.voyage_dir so the two projects' plans, reports and reviews cannot overwrite each other"
            ));
        }
    }
    None
}

/// The GitHub issue clash of this config with `other` (T-302): a repo of each
/// that reads and writes the same `owner/name` (ASCII case-insensitively) takes
/// the same issues unless both select them with labels that do not overlap.
///
/// Two projects on one GitHub repository may share it only with non-empty,
/// disjoint `[github.issues] labels`: an empty list takes every open issue, and
/// one shared label puts an issue in both projects' queues.
fn github_clash(
    other_id: &str,
    our_repos: &[config::Repo],
    other: &config::ProjectConfig,
) -> Option<String> {
    let their_repos = other.repos();
    for repo in our_repos {
        let Some(ours) = &repo.github else {
            continue;
        };
        for their_repo in &their_repos {
            let Some(theirs) = &their_repo.github else {
                continue;
            };
            if !ours.repo.eq_ignore_ascii_case(&theirs.repo) {
                continue;
            }
            if ours.issues_labels.is_empty() || theirs.issues_labels.is_empty() {
                return Some(format!(
                    "project {other_id} also takes issues from GitHub {}: each project needs its own non-empty [github.issues] labels",
                    theirs.repo
                ));
            }
            let mut shared: Vec<&str> = Vec::new();
            for label in &ours.issues_labels {
                if theirs.issues_labels.contains(label) && !shared.iter().any(|seen| *seen == label) {
                    shared.push(label);
                }
            }
            if !shared.is_empty() {
                return Some(format!(
                    "project {other_id} also takes issues labelled {} from GitHub {}: give each project labels of its own",
                    shared.join(", "),
                    theirs.repo
                ));
            }
        }
    }
    None
}

/// A config's repo paths, each canonicalized to compare two configs that name
/// one repository differently; a path that does not exist stays its text.
fn repo_paths(config: &config::ProjectConfig) -> Vec<String> {
    config
        .repos()
        .into_iter()
        .map(|repo| match std::fs::canonicalize(&repo.path) {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(_) => repo.path,
        })
        .collect()
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
    check_other_projects(tx, config)?;
    // The lane quota counts the lanes that would run (T-304); a first
    // registration has no retired lane seat yet.
    if let Some(problem) = lane_quota_problem(config, &[]) {
        return Err(GearError::new(ErrorCode::BadFormat, problem));
    }

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
        // A first registration changes no `seat_branch`: there is no old one.
        stale_worktrees: Vec::new(),
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
/// - a lower `[quota] temps` retires the temp seats it no longer allows, the
///   newest ones and every Active or Starting seat staying (T-237);
/// - a lane that is not registered yet becomes a Queued seat (generation 0) —
///   the engine onboards it;
/// - a registered lane keeps its seat: its generation, state and bindings stay,
///   only its profile follows the config;
/// - a **retired** lane stays retired, even when it is in the new config
///   (T-247): a re-apply starts no agent; a Ready task that names the lane is
///   what brings it back, in `plan::apply_queue`;
/// - a changed `conventions.seat_branch` says which seat worktrees GEAR no
///   longer uses (T-373): the result carries one line per seat that has a
///   worktree path in meta, naming the seat, the path and both branch
///   expansions. Nothing is removed;
/// - a **departed voyage's staffing** (T-392): while the voyage is under way
///   (a voyage with a departure and a task that is not terminal), a change to
///   `quota`, `lanes`, `temps` or `models` is refused with `StaffLocked` and
///   the changed fields when the caller gives no `user` words, and nothing is
///   written; with the user's words the change applies as today and the
///   voyage gets one starred `staff` decision.
///
/// The plan, the tasks and the attempts are untouched: a config change is not
/// a plan change.
///
/// The meta key of a seat's worktree path: `engine.worktree_path.<seat id>`.
/// `gear_engine::onboard::worktree_path_key` builds the same key — the engine
/// sits above this crate, so the shape is named here as well; the two must
/// stay the same string.
const WORKTREE_PATH_KEY: &str = "engine.worktree_path.";

/// The seat worktrees the stored `conventions.seat_branch` made that the new
/// config no longer uses (T-373): one line per seat of the project that has a
/// worktree path in meta, empty when the `seat_branch` did not change.
///
/// The line is what the CLI prints; it takes nothing away — the seat keeps its
/// worktree until its own agent runs in the new one, and the user removes the
/// old tree then.
fn stale_seat_worktrees(
    tx: &Tx<'_>,
    project: &ProjectRow,
    config: &config::ProjectConfig,
) -> GearResult<Vec<String>> {
    // A stored config that does not parse says nothing about the old branch.
    let Ok(stored) = config::parse(&project.config_toml) else {
        return Ok(Vec::new());
    };
    let old_template = &stored.conventions.seat_branch;
    let new_template = &config.conventions.seat_branch;
    if old_template == new_template {
        return Ok(Vec::new());
    }
    let mut lines = Vec::new();
    for seat in tx.seats_of_project(&project.id)? {
        let Some(path) = tx.get_meta(&format!("{WORKTREE_PATH_KEY}{}", seat.id))? else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        let old_branch = gear_model::branch::expand_seat(old_template, &project.id, &seat.name);
        let new_branch = gear_model::branch::expand_seat(new_template, &project.id, &seat.name);
        lines.push(format!(
            "seat {}: its worktree {path} was made for branch {old_branch}; new agents of this seat get a worktree on {new_branch}; remove the old one with `herdr worktree remove` once the seat runs in the new one",
            seat.name
        ));
    }
    Ok(lines)
}
fn re_register(
    tx: &Tx<'_>,
    project: &ProjectRow,
    config: &config::ProjectConfig,
    config_text: &str,
    user: Option<&str>,
    now: Ts,
) -> GearResult<InitResult> {
    let project_id = project.id.clone();
    check_other_projects(tx, config)?;
    // The lane quota counts the lanes that would run (T-304): the retired lane
    // seats keep their seat and do not take a slot.
    if let Some(problem) = lane_quota_problem(config, &retired_lane_names(tx, &project_id)?) {
        return Err(GearError::new(ErrorCode::BadFormat, problem));
    }

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

    // T-392: the staffing of a departed voyage is agreed at its kickoff, like
    // the goal (T-331). A change to it needs the user's own words, and the
    // refusal writes nothing. Words that are blank once trimmed are no words
    // (rework F1): `--user ""` never stands for an agreement.
    let user = user.map(str::trim).filter(|words| !words.is_empty());
    let (staff_changes, staff_changed) = match config::parse(&project.config_toml) {
        Ok(stored) => (
            staffing_changes(&stored, config),
            staffing_changed(&stored, config),
        ),
        Err(_) => (Vec::new(), false),
    };
    let locked_voyage = if !staff_changed {
        None
    } else {
        staffing_voyage(tx, &project_id)?
    };
    if let (Some(voyage), None) = (&locked_voyage, user) {
        return Err(GearError::new(
            ErrorCode::StaffLocked,
            staffing_refusal(voyage, &staff_changes),
        ));
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

    // A lower `[quota] temps` retires the reviewer seats it no longer allows
    // (T-237): the extra temp seats of an older, larger quota each hold their
    // own worktree, and the engine rotates through all of them.
    retire_excess_temps(tx, &project_id, config.quota.temps, now)?;

    // The decision log (design §7.4): a config change is GEAR's own entry.
    {
        let voyage = crate::plan::current_voyage(tx, &project_id)?;
        let what = format!("config v{version}");
        let text = format!("config|{what}");
        tx.insert_decision_if_new(&gear_store::NewDecision {
            project: project_id.to_string(),
            key: crate::decision::key_of(&text),
            voyage,
            at: crate::decision::apply_time(now),
            kind: "config".to_string(),
            what,
            why: None,
            alt: None,
            changes: None,
            // GEAR's own entries are never starred (§7.2).
            star: false,
            source: "gear".to_string(),
            line: None,
            plan_version: None,
        })?;
    }

    // The user's own words for a staffing change of a departed voyage are a
    // starred decision (T-392): the voyage's goal is locked the same way.
    if let (Some(voyage), Some(words)) = (&locked_voyage, user) {
        let what = staff_changes.join("; ");
        let why = format!("user: «{words}»");
        let text = format!("staff|{what}|{what}|{why}");
        tx.insert_decision_if_new(&gear_store::NewDecision {
            project: project_id.to_string(),
            key: crate::decision::key_of(&text),
            voyage: voyage.clone(),
            at: crate::decision::apply_time(now),
            kind: "staff".to_string(),
            what,
            why: Some(why),
            alt: None,
            changes: None,
            star: true,
            source: "gear".to_string(),
            line: None,
            plan_version: None,
        })?;
    }

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
                // A retired lane stays retired through any re-apply (T-247):
                // a config change is not a reason to start an agent again. A
                // Ready task that names the lane brings it back (see
                // `plan::apply_queue`).
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
        stale_worktrees: stale_seat_worktrees(tx, project, config)?,
    })
}

/// The voyage whose staffing is locked (T-392): the milestone of the latest
/// departed voyage, when the project still has a task that is not terminal,
/// else `None`.
///
/// The staffing of a voyage is agreed at its kickoff, like its goal (T-331):
/// while it is under way, a change to it is the user's decision, not a config
/// edit. A project whose tasks are all done, failed or cancelled has no
/// voyage under way, and its next voyage may start with other staff.
pub fn staffing_voyage(tx: &Tx<'_>, project: &str) -> GearResult<Option<String>> {
    let under_way = tx
        .tasks_of_project(project)?
        .iter()
        .any(|task| !task.state.is_terminal());
    if !under_way {
        return Ok(None);
    }
    let milestone = crate::plan::current_voyage(tx, project)?;
    Ok((!milestone.is_empty()).then_some(milestone))
}

/// Whether the staffing differs between the stored config and the new one
/// (T-392): `quota`, `lanes`, `temps` and `models` compared as whole sections,
/// so anything in them — a reordered `[[lanes]]` included — counts.
///
/// [`staffing_changes`] explains a difference field by field; it never decides
/// whether there is one.
pub fn staffing_changed(old: &config::ProjectConfig, new: &config::ProjectConfig) -> bool {
    old.quota != new.quota
        || old.lanes != new.lanes
        || old.temps != new.temps
        || old.models != new.models
}

/// The staff fields that differ between the stored config and the new one
/// (T-392), each as `section.key: old → new`, over `quota`, `lanes`, `temps`
/// and `models`.
pub fn staffing_changes(old: &config::ProjectConfig, new: &config::ProjectConfig) -> Vec<String> {
    let mut changes: Vec<String> = Vec::new();

    // [quota]
    let (a, b) = (&old.quota, &new.quota);
    if a.lanes != b.lanes {
        changes.push(changed_field("quota.lanes", &a.lanes.to_string(), &b.lanes.to_string()));
    }
    if a.temps != b.temps {
        changes.push(changed_field("quota.temps", &a.temps.to_string(), &b.temps.to_string()));
    }
    if a.includes_captain != b.includes_captain {
        changes.push(changed_field(
            "quota.includes_captain",
            &a.includes_captain.to_string(),
            &b.includes_captain.to_string(),
        ));
    }
    if a.sources != b.sources {
        changes.push(changed_field(
            "quota.sources",
            &quota_sources_text(&a.sources),
            &quota_sources_text(&b.sources),
        ));
    }

    // [[lanes]] — a lane both configs name is compared field by field; a lane
    // only one of them names is one change, with its staff as the value.
    let lanes_before = changes.len();
    let mut names: Vec<&str> = old.lanes.iter().map(|lane| lane.name.as_str()).collect();
    for lane in &new.lanes {
        if !names.contains(&lane.name.as_str()) {
            names.push(&lane.name);
        }
    }
    for name in names {
        let before = old.lanes.iter().find(|lane| lane.name == name);
        let after = new.lanes.iter().find(|lane| lane.name == name);
        let key = format!("lanes.{name}");
        match (before, after) {
            (Some(a), Some(b)) => {
                if a.duty != b.duty {
                    changes.push(changed_field(&format!("{key}.duty"), &a.duty, &b.duty));
                }
                if a.owns != b.owns {
                    changes.push(changed_field(
                        &format!("{key}.owns"),
                        &a.owns.join(", "),
                        &b.owns.join(", "),
                    ));
                }
                if a.not != b.not {
                    changes.push(changed_field(
                        &format!("{key}.not"),
                        &a.not.join(", "),
                        &b.not.join(", "),
                    ));
                }
                if a.kind != b.kind {
                    changes.push(changed_field(&format!("{key}.kind"), &a.kind, &b.kind));
                }
                if a.profile != b.profile {
                    changes.push(changed_field(&format!("{key}.profile"), &a.profile, &b.profile));
                }
                if a.models != b.models {
                    changes.push(changed_field(
                        &format!("{key}.models"),
                        &chain_text(&a.models),
                        &chain_text(&b.models),
                    ));
                }
                if a.repo != b.repo {
                    changes.push(changed_field(
                        &format!("{key}.repo"),
                        a.repo.as_deref().unwrap_or("none"),
                        b.repo.as_deref().unwrap_or("none"),
                    ));
                }
            }
            (Some(a), None) => changes.push(changed_field(&key, &lane_text(a), "none")),
            (None, Some(b)) => changes.push(changed_field(&key, "none", &lane_text(b))),
            (None, None) => {}
        }
    }
    // The sections differ but no lane field does: the lanes are in another
    // order. The order is the staffing too (it is the order the board shows
    // and the lane list prints), so it is named as the one change.
    if old.lanes != new.lanes && changes.len() == lanes_before {
        changes.push(changed_field(
            "lanes",
            &lane_order_text(&old.lanes),
            &lane_order_text(&new.lanes),
        ));
    }

    // [temps]
    match (&old.temps, &new.temps) {
        (None, None) => {}
        (Some(a), Some(b)) => {
            if a.kind != b.kind {
                changes.push(changed_field("temps.kind", &a.kind, &b.kind));
            }
            if a.profile != b.profile {
                changes.push(changed_field("temps.profile", &a.profile, &b.profile));
            }
        }
        (None, Some(b)) => changes.push(changed_field("temps", "none", &temps_text(b))),
        (Some(a), None) => changes.push(changed_field("temps", &temps_text(a), "none")),
    }

    // [models]
    let (a, b) = (&old.models, &new.models);
    for (key, before, after) in [
        ("models.lanes", &a.lanes, &b.lanes),
        ("models.review", &a.review, &b.review),
        ("models.captain", &a.captain, &b.captain),
    ] {
        if before != after {
            changes.push(changed_field(key, &chain_text(before), &chain_text(after)));
        }
    }
    if a.on_quota.lanes != b.on_quota.lanes {
        changes.push(changed_field(
            "models.on_quota.lanes",
            quota_policy_text(a.on_quota.lanes),
            quota_policy_text(b.on_quota.lanes),
        ));
    }
    if a.on_quota.review != b.on_quota.review {
        changes.push(changed_field(
            "models.on_quota.review",
            quota_policy_text(a.on_quota.review),
            quota_policy_text(b.on_quota.review),
        ));
    }
    if a.on_wait != b.on_wait {
        changes.push(changed_field(
            "models.on_wait",
            wait_policy_text(a.on_wait),
            wait_policy_text(b.on_wait),
        ));
    }
    if a.quota_reset_h != b.quota_reset_h {
        changes.push(changed_field(
            "models.quota_reset_h",
            &a.quota_reset_h.to_string(),
            &b.quota_reset_h.to_string(),
        ));
    }
    let ids: std::collections::BTreeSet<&String> = a.switch.keys().chain(b.switch.keys()).collect();
    for id in ids {
        let before = a.switch.get(id);
        let after = b.switch.get(id);
        if before != after {
            changes.push(changed_field(
                &format!("models.switch.{id}"),
                &switch_text(before),
                &switch_text(after),
            ));
        }
    }

    changes
}

/// The refusal a staffing change of a departed voyage gets without the user's
/// own words (T-392): the voyage, every changed field, and how to apply it.
pub fn staffing_refusal(voyage: &str, changes: &[String]) -> String {
    format!(
        "the staffing of voyage {voyage} was agreed at its kickoff: {} — ask the user first, then run init again with --user \"<the user's words>\"",
        changes.join("; ")
    )
}

/// One changed field as `<key>: <old> → <new>`.
fn changed_field(key: &str, old: &str, new: &str) -> String {
    format!("{key}: {old} → {new}")
}

/// The lanes in config order, as one value: the order is staffing too.
fn lane_order_text(lanes: &[config::LaneSection]) -> String {
    if lanes.is_empty() {
        return "none".to_string();
    }
    lanes.iter().map(|lane| lane.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// A lane as one value, for a lane that only one of the two configs names.
fn lane_text(lane: &config::LaneSection) -> String {
    let mut text = format!(
        "duty {:?}, kind {}, profile {}, owns [{}]",
        lane.duty,
        lane.kind,
        lane.profile,
        lane.owns.join(", ")
    );
    if !lane.not.is_empty() {
        text.push_str(&format!(", not [{}]", lane.not.join(", ")));
    }
    if !lane.models.is_empty() {
        text.push_str(&format!(", models [{}]", chain_text(&lane.models)));
    }
    if let Some(repo) = &lane.repo {
        text.push_str(&format!(", repo {repo}"));
    }
    text
}

/// A model chain as one value: the entries in order, `none` when empty.
fn chain_text(chain: &[config::ModelEntry]) -> String {
    if chain.is_empty() {
        return "none".to_string();
    }
    chain
        .iter()
        .map(entry_text)
        .collect::<Vec<_>>()
        .join(", ")
}

/// One model entry as one value, for a changed-field message (T-392 rework
/// F2): the model and its CLI, then the arguments it starts with and its
/// context window when the entry names them — so a reasoning effort that
/// changed reads `medium → high`, not the same entry twice.
fn entry_text(entry: &config::ModelEntry) -> String {
    let mut text = entry.describe();
    let args = entry.args();
    if !args.is_empty() {
        text.push_str(&format!(" args [{}]", args.join(" ")));
    }
    if let Some(k) = entry.context_window_k() {
        text.push_str(&format!(" context_window_k {k}"));
    }
    text
}

/// The quota sources as one value.
fn quota_sources_text(sources: &[config::QuotaSource]) -> String {
    if sources.is_empty() {
        return "none".to_string();
    }
    sources
        .iter()
        .map(|source| {
            let key = match (source.key_file.is_empty(), source.key_path.is_empty()) {
                (true, _) | (_, true) => String::new(),
                (false, false) => format!(" key {}#{}", source.key_file, source.key_path),
            };
            format!("{} via {}{key}", source.provider, source.reader)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The `[temps]` section as one value.
fn temps_text(temps: &config::TempsSection) -> String {
    format!("{} (profile {})", temps.kind, temps.profile)
}

/// A `[models] on_quota` policy as the config spells it.
fn quota_policy_text(policy: config::QuotaPolicy) -> &'static str {
    match policy {
        config::QuotaPolicy::Fallback => "fallback",
        config::QuotaPolicy::Wait => "wait",
    }
}

/// A `[models] on_wait` policy as the config spells it.
fn wait_policy_text(policy: config::WaitPolicy) -> &'static str {
    match policy {
        config::WaitPolicy::Keep => "keep",
        config::WaitPolicy::Release => "release",
    }
}

/// One `[models.switch]` entry as one value.
fn switch_text(entry: Option<&config::ModelSwitch>) -> String {
    match entry {
        None => "none".to_string(),
        Some(entry) => match &entry.variant {
            Some(variant) => format!("picker {:?}, bar {:?}, variant {variant}", entry.picker, entry.bar),
            None => format!("picker {:?}, bar {:?}", entry.picker, entry.bar),
        },
    }
}

/// Retire the temp seats a lower `[quota] temps` no longer allows (T-237).
///
/// The quota is the number of reviewer seats the project keeps. A seat that is
/// Active or Starting holds a working reviewer: it is never retired and it
/// takes a slot. The idle seats — Released or Standby — fill the slots that are
/// left, newest `state_since` first (their worktrees are the warm ones), and
/// every idle seat past them goes: a Released one (no agent left) becomes
/// Retired, a Standby one (its agent and pane are still there) becomes
/// Releasing, so the engine's own release path closes the pane and the next
/// config apply retires it.
fn retire_excess_temps(tx: &Tx<'_>, project: &str, quota: u32, now: Ts) -> GearResult<()> {
    let seats: Vec<SeatRow> = tx
        .seats_of_project(project)?
        .into_iter()
        .filter(|seat| seat.role == SeatRole::Temp && seat.state != SeatState::Retired)
        .collect();
    if seats.len() as i64 <= i64::from(quota) {
        return Ok(());
    }

    // The working reviewers hold their slots; the idle ones take the rest.
    let working = seats
        .iter()
        .filter(|seat| matches!(seat.state, SeatState::Active | SeatState::Starting))
        .count();
    let slots = usize::try_from(quota).unwrap_or(usize::MAX);
    let room = slots.saturating_sub(working);
    let mut idle: Vec<&SeatRow> = seats
        .iter()
        .filter(|seat| matches!(seat.state, SeatState::Released | SeatState::Standby))
        .collect();
    // The newest seats fill the slots; the id breaks a tie so the choice is
    // the same on every run.
    idle.sort_by(|a, b| b.state_since.cmp(&a.state_since).then_with(|| a.id.cmp(&b.id)));

    let reason = format!("temps quota lowered to {quota}");
    for seat in idle.into_iter().skip(room) {
        // A Standby reviewer still has its agent: Releasing is the transition
        // that lets the engine close its pane (Temp has no Standby -> Retired).
        let state = if seat.state == SeatState::Standby {
            SeatState::Releasing
        } else {
            SeatState::Retired
        };
        tx.set_seat_state(&seat.id, state, Some(&reason), now)?;
    }
    Ok(())
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
