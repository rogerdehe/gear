//! Onboarding a seat (design/03, /10): what the engine does to make a fresh
//! agent session the occupant of a seat. This module only *plans* the steps
//! (pure); executing them through Herdr is a later task.

use gear_herdr::{HerdrAct, Outcome, Profile};
use gear_model::state::require_transition;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, Ts};
use gear_store::{SeatRow, Store};

use crate::model::{lane_primary, model_start};

/// T-107: GEAR no longer ships a pre-push hook, and no allow-list is checked.
/// A worktree that still sets GEAR's own `core.hooksPath` (installed by an
/// older engine) gets it unset here, so an existing lane or reviewer loses the
/// hook the moment it is onboarded again. Only GEAR's own path is touched: a
/// repository's own hooks stay as they are.
pub(crate) fn forget_gear_hooks(worktree: &std::path::Path) -> Result<(), String> {
    let set = match git_config(worktree, &["--worktree", "--get", "core.hooksPath"]) {
        Ok(path) => path.trim().to_string(),
        // No worktree override at all (or no worktree config extension).
        Err(_) => return Ok(()),
    };
    if set.is_empty() {
        return Ok(());
    }
    // Only the exact directory the old installer made — `<this worktree's git
    // dir>/gear-hooks` — is ours. A repository that happens to point its hooks
    // at another directory by that name keeps it: the removal is recursive and
    // must never reach a directory GEAR did not create (F2 of the review).
    let git_dir = match git(worktree, &["rev-parse", "--absolute-git-dir"]) {
        Ok(dir) => dir.trim().to_string(),
        Err(_) => return Ok(()),
    };
    if git_dir.is_empty() {
        return Ok(());
    }
    let ours = std::path::Path::new(&git_dir).join("gear-hooks");
    let set_path = std::path::Path::new(&set);
    let same = match (
        std::fs::canonicalize(set_path),
        std::fs::canonicalize(&ours),
    ) {
        (Ok(set), Ok(ours)) => set == ours,
        _ => set_path == ours,
    };
    if !same {
        return Ok(()); // not the hook GEAR installed: leave it alone
    }
    git_config(worktree, &["--worktree", "--unset", "core.hooksPath"])?;
    // The directory holds nothing but GEAR's script.
    let _ = std::fs::remove_dir_all(&ours);
    Ok(())
}

/// The git common dir of `dir`, absolute: the repository a checkout belongs
/// to. A path git cannot read (not a repository, gone) is `None`.
fn common_dir(dir: &str) -> Option<String> {
    let dir = std::path::Path::new(dir);
    let out = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

/// Where a seat's temp files go (T-413): `<scratch_dir>/<project>/<seat>` when
/// the project names a `[conventions].scratch_dir` (`~` is the home folder),
/// else the seat's own worktree holds them under `.gear/tmp`. `None` = the seat
/// has neither, so there is nothing to point it at.
pub(crate) fn scratch_dir(
    conventions: &gear_app::config::Conventions,
    project: &str,
    seat: &str,
    worktree: Option<&str>,
) -> Option<String> {
    match conventions
        .scratch_dir
        .as_deref()
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
    {
        Some(dir) => Some(
            expand_home(dir)
                .join(project)
                .join(seat)
                .to_string_lossy()
                .into_owned(),
        ),
        None => worktree
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(|path| {
                std::path::Path::new(path)
                    .join(".gear")
                    .join("tmp")
                    .to_string_lossy()
                    .into_owned()
            }),
    }
}

/// A path that starts with `~` is in the home folder of the user.
fn expand_home(path: &str) -> std::path::PathBuf {
    let path = path.trim();
    let Some(home) = std::env::var_os("HOME") else {
        return std::path::PathBuf::from(path);
    };
    match path.strip_prefix('~') {
        Some(rest) => {
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            let mut home = std::path::PathBuf::from(home);
            if !rest.is_empty() {
                home.push(rest);
            }
            home
        }
        None => std::path::PathBuf::from(path),
    }
}

/// A path as one shell word: single-quoted, with an embedded `'` escaped, so a
/// scratch directory with a space or an apostrophe still reaches `mkdir` and
/// `TMPDIR` whole (F1 of the T-413 review).
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', r"'\''"))
}

/// The prefix of every seat's environment line (T-420): the seat's own scratch
/// directory, made and exported as `TMPDIR` as one shell command, so every
/// temp file of the seat — pi, codex, claude, cargo and the OS temp API all
/// read `TMPDIR` — lands there and never in the system temp directory. The
/// worktree default is ignored by the repository it belongs to, so it never
/// shows up as an uncommitted path. `None` = the seat has neither a project
/// scratch dir nor a worktree, so there is nothing to point it at.
pub(crate) fn tmpdir_prefix(
    conventions: &gear_app::config::Conventions,
    project: &str,
    seat: &str,
    worktree: Option<&str>,
) -> Option<String> {
    let scratch = scratch_dir(conventions, project, seat, worktree)?;
    if conventions
        .scratch_dir
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        if let Some(worktree) = worktree {
            exclude_gear_dir(worktree);
        }
    }
    let scratch = shell_quote(&scratch);
    Some(format!("mkdir -p {scratch}; export TMPDIR={scratch}; "))
}

/// The worktree default scratch (T-413) lives in `<worktree>/.gear/tmp`, so the
/// repository ignores `.gear/`: the line is appended once to the common dir's
/// `info/exclude`, which every worktree of the repository shares. A path that
/// is not a git repository, or cannot be written, is left as it is: the ignore
/// only keeps `git status` quiet, it never decides a merge.
fn exclude_gear_dir(worktree: &str) {
    const LINE: &str = ".gear/";
    let Some(common) = common_dir(worktree) else {
        return;
    };
    let info = std::path::Path::new(&common).join("info");
    if std::fs::create_dir_all(&info).is_err() {
        return;
    }
    let exclude = info.join("exclude");
    let mut text = std::fs::read_to_string(&exclude).unwrap_or_default();
    if text.lines().any(|line| line.trim() == LINE) {
        return;
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(LINE);
    text.push('\n');
    let _ = std::fs::write(&exclude, text);
}

/// The other repository the worktree at `path` belongs to, when it is not the
/// one at `repo` (T-253). Herdr places a worktree by the repo folder's name and
/// the branch, so two repos with the same folder name and the same seat branch
/// shared one directory (`.herdr-worktrees/api/home-merge` belonged to one
/// `api` repo while another project's `api` repo needed the same path). The git
/// common dir is the repository's identity: the worktree's must be the repo's
/// own. A path git cannot read (a fake Herdr, a path that is gone) is not
/// evidence of another repository, so it is let through.
pub(crate) fn foreign_worktree(repo: &str, path: &str) -> Option<String> {
    let own = common_dir(repo)?;
    let theirs = common_dir(path)?;
    (own != theirs).then_some(theirs)
}

/// Check the worktree `path` Herdr returned for `seat` against the repository
/// it must belong to (T-253). A worktree of another repository is never used:
/// the captain gets one item on thread `worktree:<seat>` and the caller is
/// told not to proceed.
pub(crate) fn check_worktree(
    store: &mut Store,
    project: &str,
    seat: &str,
    repo: &str,
    path: &str,
    now: Ts,
) -> Result<(), String> {
    let Some(other) = foreign_worktree(repo, path) else {
        return Ok(());
    };
    let reason = format!(
        "the worktree {path} Herdr gave {seat} belongs to another repository ({other}); rename one repo folder or set conventions.seat_branch with {{project}}"
    );
    open_worktree_item(store, project, seat, &reason, now)?;
    Err(reason)
}

/// One captain item per seat on thread `worktree:<seat>`: the same body is
/// never repeated, an open item already says it (T-253).
fn open_worktree_item(
    store: &mut Store,
    project: &str,
    seat: &str,
    body: &str,
    now: Ts,
) -> Result<(), String> {
    let thread = format!("worktree:{seat}");
    store
        .write(|tx| {
            let Some(captain) = tx.captain_of(project)? else {
                return Ok(());
            };
            let already = tx
                .thread_messages(project, &thread)?
                .into_iter()
                .any(|message| {
                    matches!(
                        message.state,
                        gear_model::MsgState::Open | gear_model::MsgState::Waiting
                    ) && message.body == body
                });
            if already {
                return Ok(());
            }
            tx.insert_message(&gear_store::NewMessage {
                meta: None,
                project: project.to_string(),
                thread,
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: captain.id,
                kind: gear_model::MsgKind::System,
                urgent: false,
                body: body.to_string(),
                reply_to: None,
                needs_ack: false,
                state: gear_model::MsgState::Open,
                created: now,
            })?;
            Ok(())
        })
        .map_err(|e| e.detail)
}

/// Run `git <args>` in `dir`; the trimmed stdout, or the stderr as the error.
fn git(dir: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_config(dir: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config"])
        .args(args)
        .output()
        .map_err(|e| format!("git config {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Where a lane's long-lived worktree comes from (17 §3–§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeStep {
    pub repo: String,
    /// The project's `[conventions].seat_branch` with `{project}` and `{seat}`
    /// filled in (T-253): `<project>/<lane>` by default.
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
    /// One shell line: `export GEAR_CTX=<project>:<seat>:<gen>; export GEAR_DB=<db>`,
    /// then [`EDITOR_ENV`].
    pub env_command: String,
    /// `<project>-<seat name>` (17 §2), at most 32 chars.
    pub agent_name: String,
    pub kind: String,
    /// The profile's `unattended_args`, then `resume_args` with `{session}`
    /// replaced when `resume` is given.
    pub args: Vec<String>,
}

/// The editor variables every agent GEAR starts carries (T-468): a git
/// command that opens an editor would wait for an answer no one can give
/// (2026-10-31: review-11's `git rebase --continue` sat in vim for 59
/// minutes).
pub const EDITOR_ENV: &str =
    "export GIT_EDITOR=true; export GIT_SEQUENCE_EDITOR=true; export EDITOR=true";

/// Plan onboarding of `seat` as generation `generation` (the next one):
/// - Lane seats get a `WorktreeStep` (branch
///   `seat_branch` = the project's `[conventions].seat_branch` template,
///   `<project>/<name>` by default, base `integration`, label
///   `<project>-<name>`); other roles `None`;
/// - `env_command` exactly `export GEAR_CTX=<project>:<name>:<generation>; export GEAR_DB=<db_path>; <EDITOR_ENV>`;
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
    seat_branch: &str,
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
        return Err(format!("agent name {agent_name} is longer than 32 chars"));
    }
    // The agent is started unattended; a resume adds the profile's resume args
    // with the session reference in place of `{session}`.
    let mut args = profile.unattended_args.clone();
    if let Some(session) = resume {
        let resume_args = profile
            .resume_args
            .as_ref()
            .ok_or_else(|| format!("profile {kind} cannot resume a session"))?;
        args.extend(
            resume_args
                .iter()
                .map(|arg| arg.replace("{session}", session)),
        );
    }
    // Only a lane holds a long-lived worktree; the captain and the temps get
    // their pane elsewhere (17 §3-§4).
    let worktree = match seat.role {
        SeatRole::Lane => Some(WorktreeStep {
            repo: repo.to_string(),
            branch: gear_model::branch::expand_seat(seat_branch, &seat.project, &seat.name),
            base: integration.to_string(),
            label: agent_name.clone(),
        }),
        _ => None,
    };
    Ok(OnboardPlan {
        worktree,
        env_command: format!(
            "export GEAR_CTX={}:{}:{}; export GEAR_DB={}; {EDITOR_ENV}",
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
///    the read transaction) for the lane's repo (`repo_of_lane`: the repo the
///    lane works in, with its own integration branch), the lane's `kind`; the
///    profile is `cfg.profiles[seat.profile]` (missing → seat NeedsCheck,
///    reason says so);
/// 2. `plan_onboard(seat, kind, profile, generation = seat.generation + 1, repo,
///    integration, cfg.db_path, None)` (Err → NeedsCheck with the reason);
/// 3. Herdr `worktree_create` → the root pane;
/// 4. `gear_app::seats::begin_onboard(seat, pane, kind)` in a write transaction;
/// 5. Herdr `pane_run(pane, env_command)`, then `agent_start(agent_name, kind, pane, args)`;
/// 6. agent started → `gear_app::seats::finish_onboard(seat, gen, session_ref)`.
/// A start refused with `agent_pane_busy` (the shell still runs the environment
/// line) keeps the seat in Starting with the start pending — same binding, pane
/// and generation — and a later tick tries it once more, for at most
/// `cfg.pane_busy_ticks` ticks (T-95); nothing before the start runs again, and
/// the tick is never held for a shell. Any other Herdr step not `Ok` → the seat
/// goes NeedsCheck with a reason naming the step (never retried
/// automatically). Returns the seat ids that became Active.
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
        let Some(config_text) = config_text else {
            continue;
        };

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

        // A voyage that has not departed yet starts no lane this tick
        // (T-448): at its departure time the engine onboards them by itself.
        if gear_app::config::departs_at(&config, now).is_some() {
            continue;
        }

        for seat in seats {
            // Only lanes are onboarded by the engine in this cut; the captain
            // and the temps get their pane elsewhere.
            if seat.role != SeatRole::Lane {
                continue;
            }
            if crate::restart::pending(store, &seat.id)?.is_some() {
                continue;
            }
            // A lane whose start is waiting on its model (T-33, F5) or on its
            // shell (T-95) is resumed: both plans are durable, so an engine
            // restart continues them instead of starting the seat over.
            let resuming = seat.state == SeatState::Starting
                && (crate::switch::pending(store, &seat.id)?.is_some()
                    || crate::start::pending(store, &seat.id)?.is_some());
            let rebound = seat.state == SeatState::Missing
                && store.read(|tx| crate::rebind::ready_to_onboard(tx, &seat.id))?;
            if seat.state == SeatState::Queued
                && store.read(|tx| {
                    Ok(tx
                        .get_meta(&format!("lane-rebind-applied:{}", seat.id))?
                        .is_some_and(|s| !s.is_empty())
                        && !crate::rebind::ready_to_onboard(tx, &seat.id)?)
                })?
            {
                continue;
            }
            if seat.state != SeatState::Queued && !resuming && !rebound {
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
                // A failed step parks the seat; it is never retried blindly,
                // and a start left pending for it goes too.
                Err(reason) => {
                    crate::start::clear_pending(store, &seat.id)?;
                    park(store, &seat.id, &reason, now)?
                }
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
        profiles
            .get(&seat.profile)
            .map(|profile| (seat.profile.as_str(), profile))
    } else {
        crate::restart::profile_for_kind(profiles, &kind)
    };
    let (profile_name, profile) =
        selected.ok_or_else(|| format!("no profile drives the CLI {kind:?}"))?;

    // A start that is already under way is continued, never redone: the model
    // switch it waits on (T-33, F5), or the shell a busy refusal waits for
    // (T-95). Both plans are in the store, so an engine restart continues them
    // instead of starting the seat over.
    if seat.state == SeatState::Starting {
        if crate::switch::pending(store, &seat.id)
            .map_err(|e| e.detail)?
            .is_some()
        {
            return resume_start(store, herdr, seat, profile, config, now);
        }
        if crate::start::pending(store, &seat.id)
            .map_err(|e| e.detail)?
            .is_some()
        {
            return resume_busy_start(store, herdr, cfg, seat, config, profile, entry, now);
        }
        // Starting for another reason (a handover, a restart): not this tick's.
        return Ok(false);
    }

    // 2. the plan: worktree, environment line, agent name and start args.
    //    The lane's worktree is in the lane's own repo, cut from that repo's
    //    integration branch (T-53): a lane bound to a child repo works there.
    let repo = config.repo_of_lane(&seat.name);
    let mut plan = plan_onboard(
        seat,
        &kind,
        profile,
        seat.generation + 1,
        &repo.path,
        &repo.integration,
        &config.conventions.seat_branch,
        &cfg.db_path,
        None,
    )?;
    let rules = crate::rules::arguments(
        store,
        &seat.project,
        seat.role,
        &seat.name,
        &cfg.db_path,
        profile,
        false,
    )
    .map_err(|e| e.detail)?;
    plan.args.splice(
        profile.unattended_args.len()..profile.unattended_args.len(),
        rules,
    );
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

    // 3. the worktree, with the root pane the agent starts in. A lane's home
    //    worktree is long-lived (17 §3): it is **reused** before anything is
    //    created —
    //    a. by its own recorded path (the lane worked in it before: a restart,
    //       or a lane retired and registered again — its checkout is usually on
    //       the task branch it last worked, so the home-branch lookup below
    //       does not find it, and a create would refuse "already exists";
    //       T-125);
    //    b. by its home branch (a lane whose checkout is still there);
    //    c. only a lane that has neither gets a new worktree.
    let recorded = store
        .read(|tx| {
            Ok(tx
                .get_meta(&worktree_repo_path_key(&seat.id, &worktree.repo))?
                .filter(|path| !path.is_empty())
                .or(tx.get_meta(&worktree_path_key(&seat.id))?))
        })
        .map_err(|e| format!("worktree path: {}", e.detail))?
        .filter(|path| !path.is_empty());
    let by_path = match recorded.as_deref() {
        Some(path) => herdr.worktree_open_path(&worktree.repo, path, &worktree.label),
        None => Outcome::Refused {
            code: "worktree_not_found".into(),
            message: String::new(),
        },
    };
    let opened = match by_path {
        // A stale record (the worktree was removed): fall back to the branch.
        Outcome::Refused { code, .. } if code == "worktree_not_found" => {
            herdr.worktree_open(&worktree.repo, &worktree.branch, &worktree.label)
        }
        other => other,
    };
    let created = match opened {
        Outcome::Ok(worktree) => worktree,
        Outcome::Refused { code, .. } if code == "worktree_not_found" => match herdr
            .worktree_create(
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
    // T-253: the worktree must belong to the lane's repo. Herdr places
    // worktrees by the repo folder's name and the branch, so a same-named repo
    // came back with another repository's checkout. It is not used, and the
    // captain hears about it.
    if let Err(reason) = check_worktree(
        store,
        &seat.project,
        &seat.name,
        &worktree.repo,
        &created.path,
        now,
    ) {
        return Err(reason);
    }
    let pane = created.pane_id;
    // T-107: an older engine left its pre-push hook here; the worktree loses
    // it now that GEAR has no allow-list.
    crate::onboard::forget_gear_hooks(std::path::Path::new(&created.path))?;
    // The worktree's path is kept: a fallback to another CLI may have to trust
    // it (grok asks before its first start in a directory, T-32).
    store
        .write(|tx| {
            tx.set_meta(&worktree_path_key(&seat.id), &created.path)?;
            tx.set_meta(
                &worktree_repo_path_key(&seat.id, &worktree.repo),
                &created.path,
            )
        })
        .map_err(|e| format!("worktree path: {}", e.detail))?;

    // 3a. the seat's scratch directory (T-413, T-420): the project's
    //    `[conventions].scratch_dir` when it names one, else the lane's own
    //    worktree. The line comes first, so every temp file of the seat lands
    //    there and never in the system temp directory.
    if let Some(prefix) = tmpdir_prefix(
        &config.conventions,
        &seat.project,
        &seat.name,
        Some(&created.path),
    ) {
        plan.env_command = format!("{prefix}{}", plan.env_command);
    }

    // 3b. the pane must be a shell. A seat retried while its agent still runs
    //    there (the agent never exited, M3 fixture re-run) would get the
    //    environment line typed into the agent's prompt — the agent then runs
    //    it as an instruction — and the start refused as agent_name_taken. The
    //    agent is asked to exit first (its profile's exit keys or command, once
    //    per tick) and the start waits for the shell; after three ticks the
    //    seat stops with a reason.
    if let Some(occupant) = herdr.agent_get(&pane).ok().flatten() {
        let key = format!("engine.onboard_exit.{}", seat.id);
        let tries = store
            .read(|tx| tx.get_meta(&key))
            .map_err(|e| e.detail)?
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        if tries >= 3 {
            store
                .write(|tx| tx.set_meta(&key, ""))
                .map_err(|e| e.detail)?;
            return Err(format!(
                "the {} agent in pane {pane} did not exit after three tries; a seat starts only in a shell",
                occupant.agent.as_deref().unwrap_or("unknown")
            ));
        }
        let exit_profile = occupant
            .agent
            .as_deref()
            .and_then(|kind| crate::restart::profile_for_kind(profiles, kind))
            .map(|(_, profile)| profile)
            .unwrap_or(profile);
        if let Some(command) = exit_profile.exit_command.as_deref() {
            // Design §10.1: the command goes to the agent's name, never to the
            // pane id — an agent that exited leaves a shell behind, and the
            // command would be typed into it.
            // T-285: an agent Herdr left unnamed is named first, in this same
            // try — a prompt to an unnamed agent is refused, so a leftover
            // would never be asked and the seat would park after three tries.
            let name = crate::names::agent_name(&seat.project, &seat.name);
            let target = match crate::names::live_name(herdr, &pane) {
                Some(live) => Some(live),
                None => match crate::names::ensure(herdr, &pane, &name) {
                    crate::names::Ensure::Renamed | crate::names::Ensure::Named => Some(name),
                    crate::names::Ensure::Taken { .. }
                    | crate::names::Ensure::NoAgent
                    | crate::names::Ensure::Failed(_) => None,
                },
            };
            if let Some(name) = target {
                let _ = herdr.prompt(&name, command);
            }
        } else if !exit_profile.exit_keys.is_empty() {
            let _ = herdr.send_keys(&pane, &exit_profile.exit_keys);
        }
        store
            .write(|tx| tx.set_meta(&key, &(tries + 1).to_string()))
            .map_err(|e| e.detail)?;
        return Ok(false);
    }
    store
        .write(|tx| tx.set_meta(&format!("engine.onboard_exit.{}", seat.id), ""))
        .map_err(|e| e.detail)?;

    // 4. the seat moves to Starting with a pending binding for the pane. The
    //    seat's profile follows the CLI that runs its model (T-35). A start
    //    left pending by an earlier generation goes with it: it belonged to
    //    another pane.
    let generation = store
        .write(|tx| {
            tx.set_meta(&crate::start::pending_key(&seat.id), "")?;
            let generation = gear_app::seats::begin_onboard(tx, &seat.id, &pane, &kind, now)?;
            tx.set_seat_profile(&seat.id, profile_name)?;
            Ok(generation)
        })
        .map_err(|e| format!("begin_onboard: {}", e.detail))?;

    if kind == "opencode" {
        if let Some(entry) = entry {
            plan.args.extend(crate::opencode::start_args(
                store,
                herdr,
                &seat.id,
                generation,
                &created.path,
                config,
                entry.model(),
                cfg.model_list_wait_s,
                cfg.model_list_poll_ms,
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
    // The shell may still be running the environment line when the start comes
    // (a slow shell init — nvm, a plugin manager): Herdr then refuses with
    // agent_pane_busy. That is "not yet", not a failure (T-95): the start is
    // tried once per tick on this binding, and only the bound parks the seat.
    // The tick is never held for the shell — the lanes queued behind it go on.
    let start_args = gear_herdr::render_args(&plan.args, &created.path);
    let pending = crate::start::PendingStart {
        pane: pane.clone(),
        generation,
        agent_name: plan.agent_name.clone(),
        kind: plan.kind.clone(),
        args: start_args.clone(),
        tries: 0,
    };
    match herdr.agent_start(&plan.agent_name, &plan.kind, &pane, &start_args) {
        Outcome::Ok(started) => finish_start(
            store,
            herdr,
            seat,
            config,
            profile,
            entry,
            &kind,
            generation,
            Some(&started),
            now,
        ),
        Outcome::Refused { code, .. } if code == "agent_pane_busy" => {
            wait_for_shell(store, cfg, &seat.id, pending)
        }
        other => Err(start_failure(herdr, &pane, &kind, &other)),
    }
}

/// Try a pending start once more (T-95): the same binding, pane and generation,
/// with the arguments rendered for the same worktree — nothing before the start
/// is run again. A binding that moved means a human decides, not another pane.
fn resume_busy_start(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &crate::EngineCfg,
    seat: &SeatRow,
    config: &gear_app::config::ProjectConfig,
    profile: &gear_herdr::Profile,
    entry: Option<&gear_app::config::ModelEntry>,
    now: Ts,
) -> Result<bool, String> {
    let Some(pending) = crate::start::pending(store, &seat.id).map_err(|e| e.detail)? else {
        return Ok(false);
    };
    let binding = store
        .read(|tx| tx.get_binding(&seat.id, pending.generation))
        .map_err(|e| e.detail)?;
    let Some(binding) = binding else {
        return Err(format!(
            "agent_start: the start was pending on {} (generation {}), but that binding is gone",
            pending.pane, pending.generation
        ));
    };
    if binding.pane_id != pending.pane || pending.generation != seat.generation {
        return Err(format!(
            "agent_start: the start was pending on pane {} of generation {}, but the seat is on pane {} of generation {}",
            pending.pane, pending.generation, binding.pane_id, seat.generation
        ));
    }
    let mut args = pending.args.clone();
    let rules = crate::rules::arguments(
        store,
        &seat.project,
        seat.role,
        &seat.name,
        &cfg.db_path,
        profile,
        false,
    )
    .map_err(|e| e.detail)?;
    crate::rules::refresh_args(&mut args, profile, rules);
    match herdr.agent_start(&pending.agent_name, &pending.kind, &pending.pane, &args) {
        Outcome::Ok(started) => finish_start(
            store,
            herdr,
            seat,
            config,
            profile,
            entry,
            &pending.kind,
            pending.generation,
            Some(&started),
            now,
        ),
        Outcome::Refused { code, .. } if code == "agent_pane_busy" => {
            wait_for_shell(store, cfg, &seat.id, pending)
        }
        other => Err(start_failure(herdr, &pending.pane, &pending.kind, &other)),
    }
}

/// Record one busy refusal (T-95): within the bound the start stays pending and
/// the tick goes on to the next seat; at the bound the seat parks with the
/// reason, because a shell that is still busy then is not coming back on its
/// own.
fn wait_for_shell(
    store: &mut Store,
    cfg: &crate::EngineCfg,
    seat_id: &str,
    pending: crate::start::PendingStart,
) -> Result<bool, String> {
    let tries = pending.tries + 1;
    let bound = cfg.pane_busy_ticks.max(1);
    if tries >= bound {
        return Err(format!(
            "agent_start: refused (agent_pane_busy): the shell was still busy after {tries} ticks (the environment line has not finished)"
        ));
    }
    crate::start::set_pending(
        store,
        seat_id,
        &crate::start::PendingStart { tries, ..pending },
    )
    .map_err(|e| e.detail)?;
    Ok(false)
}

/// The agent is up — now, or after the shell said "not yet" (T-95). The model
/// gate (T-33, F5) decides before the seat is marked started; the seat becomes
/// Active and the pending start is forgotten in one transaction.
#[allow(clippy::too_many_arguments)]
fn finish_start(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &SeatRow,
    config: &gear_app::config::ProjectConfig,
    profile: &gear_herdr::Profile,
    entry: Option<&gear_app::config::ModelEntry>,
    kind: &str,
    generation: i64,
    started: Option<&gear_herdr::AgentInfo>,
    now: Ts,
) -> Result<bool, String> {
    // 6. an OpenCode agent runs on the chain's model before the seat is marked
    //    started (T-33, F5): the shared service ignores the start environment,
    //    so the status bar decides here. Waiting keeps the start pending.
    if kind == "opencode" {
        if let Some(entry) = entry {
            match crate::switch::ensure_model(
                store,
                herdr,
                seat,
                profile,
                config,
                entry.model(),
                now,
            )
            .map_err(|e| format!("model check: {}", e.detail))?
            {
                crate::switch::ModelGate::Ready => {}
                crate::switch::ModelGate::Waiting => {
                    // The agent is up; the switch is the seat's next step and
                    // needs no start of its own.
                    crate::start::clear_pending(store, &seat.id).map_err(|e| e.detail)?;
                    return Ok(false);
                }
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
    let api_session = store
        .read(|tx| tx.get_binding(&seat.id, generation))
        .map_err(|e| e.detail)?
        .and_then(|binding| binding.session_ref);
    let session_ref = api_session.as_deref().or_else(|| {
        started
            .and_then(|info| info.agent_session.as_ref())
            .map(|session| session.value.as_str())
    });
    store
        .write(|tx| {
            gear_app::seats::finish_onboard(tx, &seat.id, generation, session_ref, now)?;
            tx.set_meta(
                &format!("engine.session_fresh.{}", seat.id),
                &generation.to_string(),
            )?;
            tx.set_meta(&crate::start::pending_key(&seat.id), "")?;
            if let Some(entry) = entry {
                tx.set_meta(&crate::model::model_meta_key(&seat.id), entry.model())?;
            }
            // T-447: onboarding starts the lane on its chain head, so the
            // fingerprint is recorded here too; reconciliation then sees no
            // config change in this or a later tick.
            if let Some(lane) = config.lanes.iter().find(|lane| lane.name == seat.name) {
                let desired = crate::restart::desired_config(config, &seat.name, &lane.kind);
                tx.set_meta(
                    &format!("engine.restart_config.{}", seat.id),
                    &desired.to_string(),
                )?;
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
    let Some((model, labels)) =
        crate::switch::plan_target(store, herdr, config, &seat.id, &plan.to)
            .map_err(|e| e.detail)?
    else {
        crate::switch::clear_pending(store, &seat.id).map_err(|e| e.detail)?;
        let reason = format!(
            "the switch target {:?} has no [models.switch] labels any more",
            plan.to
        );
        // T-60: the lane stops here — the captain is rung.
        crate::quota::alert_captain(store, &seat.project, seat, &reason, now)
            .map_err(|e| e.detail)?;
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
    let api_session = store
        .read(|tx| tx.get_binding(&seat.id, seat.generation))
        .map_err(|e| e.detail)?
        .and_then(|binding| binding.session_ref);
    let session_ref = api_session.or_else(|| {
        pane.and_then(|pane| herdr.agent_get(&pane).ok().flatten())
            .and_then(|info| info.agent_session.map(|session| session.value))
    });
    store
        .write(|tx| {
            gear_app::seats::finish_onboard(
                tx,
                &seat.id,
                seat.generation,
                session_ref.as_deref(),
                now,
            )?;
            tx.set_meta(
                &format!("engine.session_fresh.{}", seat.id),
                &seat.generation.to_string(),
            )
        })
        .map_err(|e| format!("finish_onboard: {}", e.detail))?;
    Ok(true)
}

/// The key of a seat's worktree path in the engine's meta (the seat row has
/// no field for it).
pub(crate) fn worktree_path_key(seat: &str) -> String {
    format!("engine.worktree_path.{seat}")
}

pub(crate) fn worktree_repo_path_key(seat: &str, repo: &str) -> String {
    format!("engine.worktree_path_repo.{seat}:{repo}")
}

/// Park a seat in NeedsCheck with the reason. It is never retried
/// automatically (design/10): a human decides what happens next. The
/// transition is checked like every other seat transition.
fn park(store: &mut Store, seat_id: &str, reason: &str, now: Ts) -> GearResult<()> {
    store.write(|tx| {
        let seat = tx
            .get_seat(seat_id)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {seat_id}")))?;
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

/// Why a Herdr start did not succeed (T-286): the pane's last lines decide
/// whether the CLI itself is missing. A shell that has no `codex` says so on
/// its screen, and that is not a reason to walk the model chain: the seat is
/// flagged for the install instead.
fn start_failure<T>(
    herdr: &mut dyn HerdrAct,
    pane: &str,
    kind: &str,
    outcome: &Outcome<T>,
) -> String {
    let tail = herdr.read_tail(pane, 16).unwrap_or_default();
    match crate::start::cli_missing(&tail, kind) {
        Some(line) => crate::start::cli_missing_text(kind, &line),
        None => step_failure("agent_start", outcome),
    }
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

    /// The same project with a second lane: one shell may be slow while the
    /// other lane onboards (T-95).
    const CONFIG_TWO_LANES: &str = r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 2
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/heat/"]
kind = "opencode"
profile = "opencode"

[[lanes]]
name = "warm"
duty = "warming"
owns = ["src/warm/"]
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

    /// The same project, but with no `[models.switch]` labels at all (T-94):
    /// the labels must come from OpenCode's own catalogue.
    const CONFIG_WITH_MODELS_NO_SWITCH: &str = r#"
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
lanes = ["minimax-cn/MiniMax-M3.1-Flash"]
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

    /// The same project with a child repo: the lane works in it (T-53).
    const CONFIG_WITH_REPOS: &str = r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[[repos]]
name = "web"
path = "web"
integration = "web-dev"

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
repo = "web"
"#;

    /// An engine that knows the opencode profile.
    fn cfg() -> crate::EngineCfg {
        let mut cfg = crate::EngineCfg {
            db_path: crate::testtmp::temp("gear-engine-tests")
                .join("gear.db")
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        };
        cfg.profiles
            .insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
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
        assert_eq!(
            onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap(),
            ["toy:heat"]
        );
        assert_eq!(start_args(&herdr), ["--auto", "--session", "ses_api_start"]);
        let binding = store
            .read(|tx| tx.get_binding("toy:heat", 1))
            .unwrap()
            .unwrap();
        assert_eq!(binding.session_ref.as_deref(), Some("ses_api_start"));
        assert!(!herdr.calls.iter().any(|call| matches!(
            call,
            gear_herdr::fake::Call::SendKeys { .. } | gear_herdr::fake::Call::PaneType { .. }
        )));
        let create = herdr
            .calls
            .iter()
            .find_map(|call| match call {
                gear_herdr::fake::Call::OpenCodeApi { args } if args[0] == "session.create" => {
                    Some(args)
                }
                _ => None,
            })
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&create[2]).unwrap();
        assert_eq!(body["agent"], "build");
        assert_eq!(body["model"], model);
        let path = store
            .read(|tx| tx.get_meta(&worktree_path_key("toy:heat")))
            .unwrap()
            .unwrap();
        assert_eq!(body["location"]["directory"], path);
    }

    /// T-94 (F3 of the review): a lane whose model has no `[models.switch]`
    /// labels starts on it. The labels come from OpenCode's own lists
    /// (`model.list` for the name, `provider.list` for the provider's display
    /// name), the API switch puts the session on the model, and the seat
    /// becomes Active. Nothing is typed as a model key.
    #[test]
    fn a_lane_starts_on_a_model_with_no_switch_labels() {
        let mut store = world(CONFIG_WITH_MODELS_NO_SWITCH);
        let mut herdr = FakeHerdr::new();
        herdr.start_screen = Some("┃ Build auto · MiniMax-M3.1-Flash MiniMax (minimax.cn)".into());
        let model = serde_json::json!({"providerID":"minimax-cn", "id":"MiniMax-M3.1-Flash"});
        let listed = serde_json::json!({"data":[{"providerID":"minimax-cn", "id":"MiniMax-M3.1-Flash", "name":"MiniMax-M3.1-Flash", "variants":[]}]});
        herdr.api_replies.extend([
            Ok(listed.clone()),
            Ok(serde_json::json!({"data":{"id":"ses_t94", "model":model}})),
            Ok(listed.clone()),
            Ok(serde_json::json!({"data":[{"id":"minimax-cn", "name":"MiniMax (minimax.cn)"}]})),
            Ok(serde_json::json!({"data":{"id":"ses_t94", "model":model}})),
            Ok(listed),
            Ok(serde_json::json!({"data":{"id":"ses_t94", "model":model}})),
        ]);
        assert_eq!(
            onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap(),
            ["toy:heat"]
        );
        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active, "{:?}", seat.state_reason);
        assert!(
            !herdr.calls.iter().any(|call| matches!(
                call,
                gear_herdr::fake::Call::SendKeys { .. } | gear_herdr::fake::Call::PaneType { .. }
            )),
            "no terminal model keys: {:?}",
            herdr.calls
        );
    }

    /// M3 fixture re-run: a lane retried while its agent still runs in its
    /// pane gets no environment line typed into that agent — the agent is asked
    /// to exit, and the start runs in the shell on a later tick.
    #[test]
    fn a_retried_lane_whose_agent_still_runs_is_exited_before_the_start() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        herdr.add_worktree("toy/heat", "/wt/heat", "w5:p1");
        herdr.add_agent("w5:p1", "opencode", gear_herdr::AgentStatus::Idle);

        assert!(onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1)
            .unwrap()
            .is_empty());
        assert!(
            !herdr
                .calls
                .iter()
                .any(|call| matches!(call, gear_herdr::fake::Call::PaneRun { .. })),
            "nothing typed into the agent: {:?}",
            herdr.calls
        );
        assert!(
            herdr.calls.iter().any(|call| matches!(call,
            gear_herdr::fake::Call::SendKeys { pane, .. } if pane == "w5:p1")),
            "{:?}",
            herdr.calls
        );

        herdr.exit_agent("w5:p1");
        onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 2).unwrap();
        assert!(
            herdr.calls.iter().any(|call| matches!(call,
            gear_herdr::fake::Call::PaneRun { pane, .. } if pane == "w5:p1")),
            "{:?}",
            herdr.calls
        );
    }

    /// T-125: a lane registered again reuses its home worktree. Its checkout
    /// is on the task branch it last worked, so `worktree open --branch
    /// toy/heat` finds nothing and a create would collide with the path that
    /// already exists; the recorded path is opened instead.
    #[test]
    fn a_lane_registered_again_reuses_its_home_worktree() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        // The worktree the lane worked in before, its checkout on a task
        // branch (Herdr's registry names it by that branch, not the home one).
        herdr.add_worktree("heat/t7", "/wt/heat", "w5:p1");
        herdr.add_shell("w5:p1");
        // The engine remembers the path it made for this lane.
        store
            .write(|tx| tx.set_meta(&worktree_path_key("toy:heat"), "/wt/heat"))
            .unwrap();

        assert_eq!(
            onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap(),
            ["toy:heat"]
        );
        assert!(
            herdr.calls.iter().any(|call| matches!(call,
                gear_herdr::fake::Call::WorktreeOpenPath { path, .. } if path == "/wt/heat")),
            "the lane's worktree is opened by its path: {:?}",
            herdr.calls
        );
        assert!(
            !herdr
                .calls
                .iter()
                .any(|call| matches!(call, gear_herdr::fake::Call::WorktreeCreate { .. })),
            "nothing is created for a lane that has its worktree: {:?}",
            herdr.calls
        );
        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active, "{:?}", seat.state_reason);
        // The reused worktree's pane is the lane's.
        let binding = store
            .read(|tx| tx.get_binding("toy:heat", 1))
            .unwrap()
            .unwrap();
        assert_eq!(binding.pane_id, "w5:p1");
    }

    /// T-125: a lane with no worktree at all still gets one created.
    #[test]
    fn a_lane_without_a_worktree_gets_one_created() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        assert_eq!(
            onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap(),
            ["toy:heat"]
        );
        assert!(
            herdr
                .calls
                .iter()
                .any(|call| matches!(call, gear_herdr::fake::Call::WorktreeCreate { .. })),
            "a lane that never had one gets a worktree: {:?}",
            herdr.calls
        );
        assert!(
            !herdr
                .calls
                .iter()
                .any(|call| matches!(call, gear_herdr::fake::Call::WorktreeOpenPath { .. })),
            "there is no path to open: {:?}",
            herdr.calls
        );
        assert_eq!(
            store
                .read(|tx| tx.get_seat("toy:heat"))
                .unwrap()
                .unwrap()
                .state,
            SeatState::Active
        );
    }

    /// T-125: a recorded path whose worktree is gone (removed by hand) is not
    /// a failure — the home branch, then a create, are tried in order.
    #[test]
    fn a_stale_recorded_path_falls_back_to_a_new_worktree() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        store
            .write(|tx| tx.set_meta(&worktree_path_key("toy:heat"), "/wt/gone"))
            .unwrap();

        assert_eq!(
            onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap(),
            ["toy:heat"]
        );
        let calls: Vec<String> = herdr.calls.iter().map(|call| format!("{call:?}")).collect();
        let created = calls
            .iter()
            .position(|call| call.starts_with("WorktreeCreate"));
        assert!(created.is_some(), "{calls:?}");
        assert_eq!(
            store
                .read(|tx| tx.get_seat("toy:heat"))
                .unwrap()
                .unwrap()
                .state,
            SeatState::Active
        );
    }

    /// T-67 (trial G01): an empty `model.list` for a fresh directory means
    /// "not loaded yet". The lane waits for it and starts — no other model,
    /// no other provider.
    #[test]
    fn an_empty_model_list_is_waited_for() {
        let mut store = world(CONFIG_WITH_MODELS);
        let mut herdr = FakeHerdr::new();
        herdr.start_screen = Some("┃ Build auto · MiniMax-M3.1-Flash".into());
        let model = serde_json::json!({"providerID":"minimax-cn", "id":"MiniMax-M3.1-Flash"});
        herdr.api_replies.extend([
            // The directory's catalogue is not loaded yet …
            Ok(serde_json::json!({"data":[]})),
            // … then it is.
            Ok(serde_json::json!({"data":[{"providerID":"minimax-cn", "id":"MiniMax-M3.1-Flash", "variants":[]}]})),
            Ok(serde_json::json!({"data":{"id":"ses_waited", "model":model}})),
            // The model gate after the start.
            Ok(serde_json::json!({"data":[{"providerID":"minimax-cn", "id":"MiniMax-M3.1-Flash", "variants":[]}]})),
            Ok(serde_json::json!({"data":{"id":"ses_waited", "model":model}})),
        ]);
        let mut cfg = cfg();
        cfg.model_list_wait_s = 5;
        // T-127 F5: no test sleeps on the wall clock. The poll is 0, so the
        // wait between two asks is no wait at all — the fake's next queued
        // answer is what moves the wait forward.
        cfg.model_list_poll_ms = 0;

        assert_eq!(
            onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1).unwrap(),
            ["toy:heat"]
        );

        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active, "not NeedsCheck");
        let binding = store
            .read(|tx| tx.get_binding("toy:heat", 1))
            .unwrap()
            .unwrap();
        assert_eq!(binding.session_ref.as_deref(), Some("ses_waited"));
        // The same directory is asked again — never another one.
        let looks: Vec<Vec<String>> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                gear_herdr::fake::Call::OpenCodeApi { args } if args[0] == "model.list" => {
                    Some(args.clone())
                }
                _ => None,
            })
            .collect();
        assert!(looks.len() >= 2, "{looks:?}");
        assert_eq!(looks[0], looks[1], "the same directory is asked again");
        // Never a model switch typed into the TUI: the API decided.
        assert!(!herdr.calls.iter().any(|call| matches!(
            call,
            gear_herdr::fake::Call::SendKeys { .. } | gear_herdr::fake::Call::PaneType { .. }
        )));
    }

    /// The list is there and does not hold the model: a config fault, named.
    #[test]
    fn a_model_absent_from_a_loaded_list_parks_the_lane() {
        let mut store = world(CONFIG_WITH_MODELS);
        let mut herdr = FakeHerdr::new();
        herdr.api_replies.extend([Ok(serde_json::json!({
            "data":[{"providerID":"minimax-cn", "id":"MiniMax-M3.0-old", "variants":[]}]
        }))]);

        assert!(onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1)
            .unwrap()
            .is_empty());

        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::NeedsCheck);
        let reason = seat.state_reason.unwrap_or_default();
        assert!(reason.contains("invalid OpenCode model"), "{reason}");
        assert!(reason.contains("absent from model.list"), "{reason}");
        assert!(!herdr
            .calls
            .iter()
            .any(|call| matches!(call, gear_herdr::fake::Call::AgentStart { .. })));
    }

    /// Still empty when the wait runs out: NeedsCheck says so, with the
    /// directory — the model is neither valid nor invalid.
    #[test]
    fn a_model_list_that_stays_empty_parks_the_lane() {
        let mut store = world(CONFIG_WITH_MODELS);
        let mut herdr = FakeHerdr::new();
        herdr
            .api_replies
            .extend([Ok(serde_json::json!({"data":[]}))]);
        let mut cfg = cfg();
        cfg.model_list_wait_s = 0;

        assert!(onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1)
            .unwrap()
            .is_empty());

        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::NeedsCheck);
        let reason = seat.state_reason.unwrap_or_default();
        assert!(reason.contains("model list is not loaded"), "{reason}");
        let path = store
            .read(|tx| tx.get_meta(&worktree_path_key("toy:heat")))
            .unwrap()
            .unwrap();
        assert!(reason.contains(&path), "the directory is named: {reason}");
        assert!(!herdr
            .calls
            .iter()
            .any(|call| matches!(call, gear_herdr::fake::Call::AgentStart { .. })));
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
        assert!(
            herdr.calls.is_empty(),
            "nothing was done in Herdr: {:?}",
            herdr.calls
        );
    }

    /// The shell still runs the environment line when the start comes: Herdr
    /// refuses with agent_pane_busy, the seat keeps its start in Starting, and
    /// the next tick's attempt works — the environment line ran once (T-95).
    #[test]
    fn a_busy_shell_at_start_is_retried() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        herdr.refuse_starts = Some(("agent_pane_busy".into(), 1));

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();
        assert!(active.is_empty(), "the seat waits, it does not start");
        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(
            seat.state,
            SeatState::Starting,
            "a pending start, not NeedsCheck"
        );
        let pending = crate::start::pending(&mut store, "toy:heat")
            .unwrap()
            .expect("a pending start");
        assert_eq!((pending.tries, pending.generation), (1, 1));

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 2).unwrap();
        assert_eq!(active, vec!["toy:heat".to_string()]);
        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active, "not NeedsCheck");
        assert_eq!(crate::start::pending(&mut store, "toy:heat").unwrap(), None);

        // One environment line, two starts — the retry is the start only.
        assert_eq!(
            herdr
                .calls
                .iter()
                .filter(|call| matches!(call, gear_herdr::fake::Call::PaneRun { .. }))
                .count(),
            1,
            "the environment line is never re-run"
        );
        assert_eq!(
            herdr
                .calls
                .iter()
                .filter(|call| matches!(call, gear_herdr::fake::Call::AgentStart { .. }))
                .count(),
            2,
            "one attempt per tick"
        );
        let starts: Vec<String> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                gear_herdr::fake::Call::AgentStart {
                    name, pane, args, ..
                } => Some(format!("{name}@{pane} {}", args.join(" "))),
                _ => None,
            })
            .collect();
        assert_eq!(starts[0], starts[1], "the same binding, pane and arguments");
        assert_eq!(
            herdr
                .calls
                .iter()
                .filter(|call| matches!(call, gear_herdr::fake::Call::WorktreeCreate { .. }))
                .count(),
            1,
            "the worktree is created once"
        );
    }

    /// A start refused because the CLI is not installed says so on the pane
    /// (T-286): the lane parks with the install, not with a generic reason —
    /// and the model chain is never walked for it.
    #[test]
    fn a_lane_whose_cli_is_missing_parks_with_the_install() {
        let config = CONFIG
            .replace("kind = \"opencode\"", "kind = \"codex\"")
            .replace("profile = \"opencode\"", "profile = \"codex\"");
        let mut store = world(&config);
        store
            .write(|tx| tx.set_seat_profile("toy:heat", "codex"))
            .unwrap();
        let mut herdr = FakeHerdr::new();
        let mut cfg = cfg();
        cfg.profiles
            .insert("codex".into(), gear_herdr::builtin("codex").unwrap());
        herdr.refuse_starts = Some(("agent_start_timeout".into(), 1));
        // The shell of the fresh worktree could not find the CLI.
        herdr.screen_after = vec![("GEAR_CTX".into(), "zsh: command not found: codex".into())];

        let active = onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1).unwrap();

        assert!(active.is_empty(), "the lane does not start");
        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::NeedsCheck);
        let reason = seat.state_reason.unwrap_or_default();
        assert!(
            reason.contains("the codex CLI is not installed or not on PATH"),
            "{reason}"
        );
        assert_eq!(crate::start::pending(&mut store, "toy:heat").unwrap(), None);
        assert!(herdr.started_agents.is_empty(), "no agent was started");
    }

    /// A refused start whose screen names no missing CLI keeps today's reason
    /// (T-286).
    #[test]
    fn a_refused_start_without_a_missing_cli_keeps_its_reason() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        herdr.refuse_starts = Some(("agent_start_timeout".into(), 1));
        herdr.screen_after = vec![("GEAR_CTX".into(), "error: provider timeout".into())];

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert!(active.is_empty());
        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::NeedsCheck);
        let reason = seat.state_reason.unwrap_or_default();
        assert!(
            reason.contains("agent_start: refused (agent_start_timeout)"),
            "{reason}"
        );
        assert!(!reason.contains("CLI is not installed"), "{reason}");
    }

    /// Busy for three ticks, then free: the seat is Starting while the shell
    /// finishes and Active on the fourth — one attempt per tick, never a park
    /// (the live multi-api case: three lanes on a slow shell).
    #[test]
    fn a_shell_busy_for_three_ticks_then_free_starts_the_lane() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        herdr.refuse_starts = Some(("agent_pane_busy".into(), 3));

        let mut seen = Vec::new();
        for tick in 0..4 {
            let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1 + tick).unwrap();
            let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
            seen.push((seat.state, active.len()));
            // The pending start survives each tick with the same binding.
            if tick < 3 {
                let pending = crate::start::pending(&mut store, "toy:heat")
                    .unwrap()
                    .unwrap();
                let binding = store
                    .read(|tx| tx.get_binding("toy:heat", 1))
                    .unwrap()
                    .unwrap();
                assert_eq!(pending.tries, tick + 1);
                assert_eq!(
                    (pending.pane.as_str(), pending.generation),
                    (binding.pane_id.as_str(), 1)
                );
            }
        }

        assert_eq!(
            seen,
            vec![
                (SeatState::Starting, 0),
                (SeatState::Starting, 0),
                (SeatState::Starting, 0),
                (SeatState::Active, 1),
            ]
        );
        assert_eq!(herdr.started_agents, vec!["toy-heat".to_string()]);
    }

    /// A shell that stays busy past the bound is not coming back on its own:
    /// the seat parks in NeedsCheck with the reason, and nothing stays pending.
    #[test]
    fn a_shell_busy_past_the_bound_parks_the_seat() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        herdr.refuse_starts = Some(("agent_pane_busy".into(), 1000));
        let mut cfg = cfg();
        cfg.pane_busy_ticks = 3;

        for tick in 0..2 {
            assert!(onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1 + tick)
                .unwrap()
                .is_empty());
            let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
            assert_eq!(seat.state, SeatState::Starting, "tick {}", tick + 1);
        }
        assert!(onboard_queued(&mut store, &mut herdr, &cfg, T0 + 3)
            .unwrap()
            .is_empty());

        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::NeedsCheck);
        let reason = seat.state_reason.unwrap_or_default();
        assert!(reason.contains("agent_pane_busy"), "{reason}");
        assert!(reason.contains("still busy after 3 ticks"), "{reason}");
        assert_eq!(crate::start::pending(&mut store, "toy:heat").unwrap(), None);
        assert!(herdr.started_agents.is_empty(), "no agent was started");
    }

    /// A queued lane behind a busy one onboards in the same tick: the tick is
    /// never held for a shell (T-95).
    #[test]
    fn a_second_queued_lane_onboards_while_the_first_waits() {
        let mut store = world(CONFIG_TWO_LANES);
        store
            .write(|tx| {
                tx.insert_seat(&SeatRow {
                    id: "toy:warm".into(),
                    project: "toy".into(),
                    role: SeatRole::Lane,
                    name: "warm".into(),
                    profile: "opencode".into(),
                    generation: 0,
                    state: SeatState::Queued,
                    state_since: T0,
                    state_reason: Some("registered".into()),
                    task_id: None,
                })
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        // The first start of the tick is refused: heat, first in id order.
        herdr.refuse_starts = Some(("agent_pane_busy".into(), 1));

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(
            active,
            vec!["toy:warm".to_string()],
            "the second lane went on"
        );
        let heat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(heat.state, SeatState::Starting);
        let warm = store.read(|tx| tx.get_seat("toy:warm")).unwrap().unwrap();
        assert_eq!(warm.state, SeatState::Active);
        assert_eq!(herdr.started_agents, vec!["toy-warm".to_string()]);

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 2).unwrap();
        assert_eq!(
            active,
            vec!["toy:heat".to_string()],
            "the waiting lane follows"
        );
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
        assert_eq!(
            seat.state,
            SeatState::Active,
            "an active seat is never onboarded again"
        );
    }

    #[test]
    fn the_lane_model_is_carried_by_the_environment_line() {
        let mut store = world(CONFIG_WITH_MODELS);
        let mut herdr = FakeHerdr::new();
        // An OpenCode lane must start on the chain's model (T-33, F5).
        herdr.start_screen = Some("Build auto · MiniMax-M3.1-Flash MiniMax · high".into());

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(
            active,
            vec!["toy:heat".to_string()],
            "state {:?}: {:?}",
            seat.state,
            seat.state_reason
        );
        let env = env_line(&herdr);
        assert!(
            env.contains(r#"OPENCODE_CONFIG_CONTENT='{"model":"minimax-cn/MiniMax-M3.1-Flash"}'"#),
            "the environment line must carry the lane's model: {env}"
        );
        assert!(env.starts_with(&format!("mkdir -p '/fake-worktrees/toy-heat/.gear/tmp'; export TMPDIR='/fake-worktrees/toy-heat/.gear/tmp'; export GEAR_CTX=toy:heat:1; export GEAR_DB={}", cfg().db_path)), "{env}");
        // The model is only read by a process that inherits the pane's
        // environment, so the agent gets a private server.
        assert!(
            !start_args(&herdr).contains(&"--standalone".to_string()),
            "no --standalone (L-56): {:?}",
            start_args(&herdr)
        );
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
            (
                "MiniMax-M3.1-Flash".into(),
                "Build auto · MiniMax-M3.1-Flash MiniMax · high".into(),
            ),
        ];

        // Tick 1: the agent starts, the check finds another model, esc is sent.
        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();
        assert!(active.is_empty(), "the seat is not started yet");
        let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Starting, "the start stays pending");
        assert!(crate::switch::pending(&mut store, "toy:heat")
            .unwrap()
            .is_some());

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
        assert_eq!(
            sent.iter().filter(|c| *c == "/model").count(),
            1,
            "{sent:?}"
        );
        assert!(sent.iter().any(|c| c == "MiniMax-M3.1-Flash"), "{sent:?}");
        assert!(
            herdr.started_agents.len() == 1,
            "the agent is started once: {:?}",
            herdr.started_agents
        );
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
        assert!(
            !sent.iter().any(|c| c == "/model"),
            "nothing typed into the agent: {sent:?}"
        );
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
        assert!(
            !env.contains("MiniMax-M3.1-Flash"),
            "the lane chain must not be used: {env}"
        );
    }

    #[test]
    fn opencode_start_keeps_its_original_args_without_rule_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        let mut config = cfg();
        config.db_path = dir.path().join("gear.db").to_string_lossy().into();
        assert_eq!(
            onboard_queued(&mut store, &mut herdr, &config, T0 + 1).unwrap(),
            ["toy:heat"]
        );
        assert_eq!(start_args(&herdr), ["--auto"]);
        // T-469: a lane's rules file is its own, under `rules/lanes`.
        assert!(dir.path().join("rules/lanes/heat.md").is_file());
    }

    #[test]
    fn without_a_model_chain_the_environment_line_stays_as_it_was() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(active, vec!["toy:heat".to_string()]);
        assert_eq!(env_line(&herdr), format!("mkdir -p '/fake-worktrees/toy-heat/.gear/tmp'; export TMPDIR='/fake-worktrees/toy-heat/.gear/tmp'; export GEAR_CTX=toy:heat:1; export GEAR_DB={}; {}", cfg().db_path, EDITOR_ENV));
        assert_eq!(start_args(&herdr), vec!["--auto".to_string()]);
    }

    /// T-53: a lane bound to a child repo is onboarded in that repo: its
    /// worktree is cut there, from that repo's own integration branch.
    #[test]
    fn a_lane_bound_to_a_child_repo_is_onboarded_in_that_repo() {
        let mut store = world(CONFIG_WITH_REPOS);
        let mut herdr = FakeHerdr::new();

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(active, vec!["toy:heat".to_string()]);
        let (repo, branch, base, label) = herdr
            .calls
            .iter()
            .find_map(|call| match call {
                gear_herdr::fake::Call::WorktreeCreate {
                    repo,
                    branch,
                    base,
                    label,
                } => Some((repo.clone(), branch.clone(), base.clone(), label.clone())),
                _ => None,
            })
            .expect("the engine created the lane's worktree");
        assert_eq!(repo, "/repo/web", "the worktree is in the lane's repo");
        assert_eq!(branch, "toy/heat");
        assert_eq!(base, "web-dev", "the repo's own integration branch");
        assert_eq!(label, "toy-heat");
    }

    /// T-107: GEAR used to install its own pre-push check in every worktree.
    /// A worktree that still sets it (`core.hooksPath` in its worktree config)
    /// loses the hook when the lane is onboarded again, while a repository's
    /// own hooks stay as they are.
    #[test]
    fn an_old_gear_hooks_path_is_unset_on_onboarding() {
        let sh = |dir: &std::path::Path, command: &str| {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{command}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        // A real repo with a linked worktree for the lane.
        let root = crate::testtmp::temp("gear-onboard-hooks");
        sh(&root, "git init -q -b dev work");
        let work = root.join("work");
        std::fs::write(work.join("README.md"), "dev\n").unwrap();
        sh(
            &work,
            "git add -A && git -c user.email=a@b -c user.name=a commit -qm dev",
        );
        sh(&work, "git worktree add -q -b toy/heat ../heat dev");
        let worktree = root.join("heat");
        // The hook an older engine installed: its own path, worktree-only.
        sh(&worktree, "git config extensions.worktreeConfig true");
        let hooks = format!(
            "{}/gear-hooks",
            sh(&worktree, "git rev-parse --absolute-git-dir")
        );
        sh(
            &worktree,
            &format!("mkdir -p {hooks} && printf '#!/bin/sh\\nexit 1\\n' > {hooks}/pre-push && chmod +x {hooks}/pre-push"),
        );
        sh(
            &worktree,
            &format!("git config --worktree core.hooksPath {hooks}"),
        );
        assert_eq!(sh(&worktree, "git config --worktree core.hooksPath"), hooks);

        // The engine opens that very worktree for the lane.
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        herdr.add_shell("w1:p2");
        herdr.add_worktree("toy/heat", &worktree.to_string_lossy(), "w1:p2");

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(active, vec!["toy:heat".to_string()]);
        let set = std::process::Command::new("git")
            .args([
                "-C",
                &worktree.to_string_lossy(),
                "config",
                "--worktree",
                "--get",
                "core.hooksPath",
            ])
            .output()
            .unwrap();
        assert!(
            !set.status.success() || set.stdout.is_empty(),
            "the hook is gone: {}",
            String::from_utf8_lossy(&set.stdout)
        );
        assert!(
            !std::path::Path::new(&hooks).exists(),
            "the hook directory is gone"
        );

        // F2 of the review: a repository that points its hooks at *another*
        // directory named `gear-hooks` keeps it — the removal is recursive and
        // must only reach the one the old installer made.
        let own = format!("{}/my-tools/gear-hooks", root.display());
        sh(
            &worktree,
            &format!("mkdir -p {own} && printf '#!/bin/sh\\nexit 0\\n' > {own}/pre-push"),
        );
        sh(
            &worktree,
            &format!("git config --worktree core.hooksPath {own}"),
        );
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        herdr.add_shell("w1:p2");
        herdr.add_worktree("toy/heat", &worktree.to_string_lossy(), "w1:p2");
        onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();
        assert_eq!(
            sh(&worktree, "git config --worktree core.hooksPath"),
            own,
            "not ours"
        );
        assert!(
            std::path::Path::new(&own).join("pre-push").exists(),
            "its hooks stay"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// T-413: the lane's temp files go in its own scratch directory and never
    /// in the system temp directory: `TMPDIR` is `<worktree>/.gear/tmp`, which
    /// the repository is told to ignore — once.
    #[test]
    fn a_lane_writes_its_temp_files_in_its_worktree_scratch_dir() {
        let sh = |dir: &std::path::Path, command: &str| {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{command}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        // A real repo with a linked worktree for the lane (the exclude file is
        // the repository's, shared by every worktree of it).
        let root = crate::testtmp::temp("gear-onboard-scratch");
        sh(&root, "git init -q -b dev work");
        let work = root.join("work");
        std::fs::write(work.join("README.md"), "dev\n").unwrap();
        sh(
            &work,
            "git add -A && git -c user.email=a@b -c user.name=a commit -qm dev",
        );
        sh(&work, "git worktree add -q -b toy/heat ../heat dev");
        let worktree = root.join("heat");

        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        herdr.add_shell("w1:p2");
        herdr.add_worktree("toy/heat", &worktree.to_string_lossy(), "w1:p2");

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(active, vec!["toy:heat".to_string()]);
        let scratch = worktree.join(".gear").join("tmp");
        assert!(
            env_line(&herdr).starts_with(&format!(
                "mkdir -p '{}'; export TMPDIR='{}'; export GEAR_CTX=toy:heat:1; export GEAR_DB={}",
                scratch.display(),
                scratch.display(),
                cfg().db_path
            )),
            "{}",
            env_line(&herdr)
        );

        // The repository ignores `.gear/`, the line added once only.
        let exclude = work.join(".git").join("info").join("exclude");
        let lines = |path: &std::path::Path| {
            std::fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .filter(|line| line.trim() == ".gear/")
                .count()
        };
        assert_eq!(lines(&exclude), 1, "the repository ignores .gear/");
        exclude_gear_dir(&worktree.to_string_lossy());
        assert_eq!(lines(&exclude), 1, "the line is added once");
    }

    /// T-413: a project may name the parent (`[conventions].scratch_dir`):
    /// each seat gets `<scratch_dir>/<project>/<seat>`.
    #[test]
    fn a_project_scratch_dir_holds_each_seat_own_directory() {
        let scratch = crate::testtmp::temp("gear-onboard-scratch-dir").join("scratch");
        let config_toml = format!(
            "{CONFIG}\n[conventions]\nscratch_dir = \"{}\"\n",
            scratch.display()
        );
        let mut store = world(&config_toml);
        let mut herdr = FakeHerdr::new();

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(active, vec!["toy:heat".to_string()]);
        let own = scratch.join("toy").join("heat");
        assert!(
            env_line(&herdr).starts_with(&format!(
                "mkdir -p '{}'; export TMPDIR='{}'; export GEAR_CTX=toy:heat:1; export GEAR_DB={}",
                own.display(),
                own.display(),
                cfg().db_path
            )),
            "{}",
            env_line(&herdr)
        );
    }

    /// T-413 (F1 of its review): a `scratch_dir` is a configurable path, so a
    /// space or an apostrophe in it is normal use: the line quotes the path,
    /// `sh` makes the very directory and `TMPDIR` names it whole.
    #[test]
    fn a_scratch_dir_with_a_space_and_an_apostrophe_is_run_as_it_is() {
        let parent = crate::testtmp::temp("gear-onboard-scratch-quoted").join("scratch parent's");
        let config_toml = format!(
            "{CONFIG}\n[conventions]\nscratch_dir = \"{}\"\n",
            parent.display()
        );
        let mut store = world(&config_toml);
        let mut herdr = FakeHerdr::new();

        let active = onboard_queued(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

        assert_eq!(active, vec!["toy:heat".to_string()]);
        let own = parent.join("toy").join("heat");
        let line = env_line(&herdr);
        assert!(line.contains('\''), "the path is quoted: {line}");
        // The seat's shell runs the line: the directory is made and `TMPDIR`
        // holds the path whole, not the part before the space.
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{line}; printf '%s' \"$TMPDIR\""))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{line}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            own.to_string_lossy(),
            "{line}"
        );
        assert!(own.is_dir(), "{line}");
    }
}
