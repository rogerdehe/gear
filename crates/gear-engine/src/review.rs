//! Starting a reviewer when a lane hands in a result (design/10 temp workers,
//! conventions 17 §8, plan T-5).
//!
//! One temp reviewer per attempt that a lane handed in: the task is
//! `Verifying`, the attempt is `Submitted` with a candidate, and no temp seat
//! is bound to that task yet. The reviewer gets
//!
//! - its own worktree (`review/t<n>`), cut at the candidate,
//! - its own agent (the `[temps]` kind) on a model of its own: the first of
//!   `[models].review` that differs from the writer's,
//! - a review packet with the goal, the accept criteria, the candidate, the
//!   diff range, the rules (`gear --skill review`) and the exact command to
//!   hand the verdict in.
//!
//! The lane is never blocked: the reviewer is a seat of its own.

use gear_app::config::ProjectConfig;
use gear_herdr::{HerdrAct, Outcome, Profile};
use gear_model::{
    AttemptState, ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, SeatRole, SeatState,
    TaskState, Ts,
};
use gear_store::{BindingStatus, NewMessage, NewOp, SeatRow, Store, Tx};

use crate::model::model_start;
use crate::model::review_model;
use crate::EngineCfg;

/// What the review step did in one tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewReport {
    /// The reviewer seats that were started (their seat id).
    pub started: Vec<String>,
    /// The tasks whose review still waits: no free temp slot, or a reviewer
    /// that is not up yet.
    pub waiting: Vec<String>,
}

/// Start the reviewers the project needs.
///
/// Per task whose attempt a lane handed in and that no reviewer is bound to:
/// create the temp seat (bound to that task for life), and — when a temp slot
/// is free — give it a worktree, an agent and the review packet. A task
/// without a free slot waits: the captain is told once, and the lane carries
/// on with its next task.
pub fn start_reviews(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &std::collections::BTreeMap<String, gear_herdr::Profile>,
    project: &str,
    now: Ts,
) -> GearResult<ReviewReport> {
    let mut report = ReviewReport::default();

    let Some(row) = store.read(|tx| tx.get_project(project))? else {
        return Ok(report);
    };
    let config = match gear_app::config::parse(&row.config_toml) {
        Ok(config) => config,
        // A config that does not parse: the project is parked elsewhere and
        // no reviewer is started for it.
        Err(_) => return Ok(report),
    };
    // Without `[temps]` there is no reviewer to start.
    let Some(temps) = config.temps.as_ref() else {
        return Ok(report);
    };
    let Some(profile) = profiles.get(&temps.profile) else {
        return Ok(report);
    };
    // An unverified profile may not take over a real project (design/04).
    if !profile.verified {
        return Ok(report);
    }

    // A reviewer that the old activation/delivery pair could leave without its
    // packet gets it before anything else is started.
    report.started.extend(recover_missing_packets(store, project, &config, now)?);

    for task in tasks_to_review(store, project)? {
        // 1. the reviewer seat: created once and bound to the task for life.
        let seat = match temp_seat_of(store, project, &task.task)? {
            Some(seat) => seat,
            // A seat that finished its last job is warm: the job takes it
            // rather than building a new worktree (17 §8a).
            None => match free_seat(store, project)? {
                Some(seat) => take_seat(store, &seat, &task.task, now)?,
                // No warm seat free: a new one only while the quota has room.
                // Otherwise the job waits for a seat instead of building a
                // cold extra worktree (finding 2 of the T-18 review).
                None => match seats_free_for_new(store, project, i64::from(config.quota.temps))? {
                    true => create_temp_seat(store, project, &task.task, &temps.profile, now)?,
                    false => {
                        report.waiting.push(task.task.clone());
                        tell_captain(store, project, &task, now)?;
                        continue;
                    }
                },
            },
        };

        // 2. a seat that is already up needs nothing; a seat that was parked
        //    (unknown, needs check, missing) is never retried (design/10).
        if !matches!(seat.state, SeatState::Queued | SeatState::Starting | SeatState::Released) {
            continue;
        }

        // 3. a free temp slot? A reviewer that already holds one (starting)
        //    carries on; a queued one waits, and the captain is told once
        //    (design/10: QUEUED happens only when the quota is exhausted; the
        //    quota is the project config's `[quota].temps`).
        if seat.state != SeatState::Released
            && !occupies_quota(seat.state)
            && !has_free_slot(store, project, i64::from(config.quota.temps))?
        {
            report.waiting.push(task.task.clone());
            tell_captain(store, project, &task, now)?;
            continue;
        }

        // 4. the reviewer: worktree, environment, agent, review packet.
        let model = review_model(&config, &task.writer);
        match start_reviewer(
            store,
            herdr,
            cfg,
            &config,
            &seat,
            &task,
            model.as_deref(),
            profile,
            now,
        )? {
            Started::Yes => report.started.push(seat.id.clone()),
            // The pane of a fresh worktree needs a few seconds (the shell is
            // still starting): the next tick tries again, this is not a
            // failure.
            Started::NotYet => report.waiting.push(task.task.clone()),
        }
    }
    Ok(report)
}

/// An attempt a lane handed in that no reviewer is bound to.
struct ReviewTask {
    task: String,
    attempt: i64,
    /// The seat that handed the result in.
    writer: String,
    candidate: String,
}

/// The tasks of the project whose attempt was handed in and that no temp seat
/// is bound to yet.
fn tasks_to_review(store: &mut Store, project: &str) -> GearResult<Vec<ReviewTask>> {
    store.read(|tx| {
        let seats = tx.seats_of_project(project)?;
        let mut wanted = Vec::new();
        for task in tx.tasks_of_project(project)? {
            // The lane handed a result in: the task is Verifying and its
            // attempt is Submitted with a candidate.
            if task.state != TaskState::Verifying {
                continue;
            }
            let Some(attempt) = tx.attempts_of_task(project, &task.id)?.into_iter().last() else {
                continue;
            };
            if attempt.state != AttemptState::Submitted {
                continue;
            }
            // Its verdict is in: the review is done and waits for settlement,
            // not for another reviewer (L-61: with reviewers released on their
            // verdict, this stops a start/release loop).
            if tx.get_review_verdict(project, &task.id, attempt.n)?.is_some() {
                continue;
            }
            let Some(candidate) = attempt.candidate.clone() else {
                continue;
            };
            // A reviewer that is up already: it is not started twice. A
            // reviewer that waits (queued) or is not up yet (starting) is
            // still this task's reviewer: the tick starts it when it can.
            // A released (or retired) reviewer is finished: a rework attempt,
            // or a task whose reviewer was lost, gets a new one.
            if seats.iter().any(|seat| {
                seat.role == SeatRole::Temp
                    && seat.task_id.as_deref() == Some(task.id.as_str())
                    && !matches!(
                        seat.state,
                        SeatState::Queued | SeatState::Starting | SeatState::Released | SeatState::Retired
                    )
            }) {
                continue;
            }
            wanted.push(ReviewTask {
                task: task.id.clone(),
                attempt: attempt.n,
                writer: attempt.seat.clone(),
                candidate,
            });
        }
        Ok(wanted)
    })
}

/// The temp seat bound to `task`, if there is one.
fn temp_seat_of(store: &mut Store, project: &str, task: &str) -> GearResult<Option<SeatRow>> {
    store.read(|tx| {
        Ok(tx
            .seats_of_project(project)?
            .into_iter()
            // A released or retired reviewer is finished; a rework attempt
            // (or a lost review) gets a new seat.
            .find(|seat| {
                seat.role == SeatRole::Temp
                    && seat.task_id.as_deref() == Some(task)
                    && !matches!(seat.state, SeatState::Released | SeatState::Retired)
            }))
    })
}

/// A seat that finished its last job and holds no live one: a warm worktree
/// with its build cache waiting for the next review (17 §8a).
fn free_seat(store: &mut Store, project: &str) -> GearResult<Option<SeatRow>> {
    store.read(|tx| {
        Ok(tx
            .seats_of_project(project)?
            .into_iter()
            // Only a warm seat of the new kind (`review-<k>`): legacy per-task
            // seats (`r-<n>`) are retired and never reused.
            .find(|seat| {
                seat.role == SeatRole::Temp
                    && seat.state == SeatState::Released
                    && seat.name.starts_with("review-")
            }))
    })
}

/// Whether the project may add a seat: fewer seats exist than the quota allows.
fn seats_free_for_new(store: &mut Store, project: &str, quota: i64) -> GearResult<bool> {
    store.read(|tx| {
        let seats = tx
            .seats_of_project(project)?
            .into_iter()
            .filter(|seat| seat.role == SeatRole::Temp && seat.state != SeatState::Retired)
            .count() as i64;
        Ok(seats < quota)
    })
}

/// Bind a free seat to the new job. The seat keeps its worktree and its
/// cache; only the job it serves changes.
fn take_seat(store: &mut Store, seat: &SeatRow, task: &str, now: Ts) -> GearResult<SeatRow> {
    store.write(|tx| tx.set_seat_task(&seat.id, task))?;
    let _ = now;
    Ok(SeatRow { task_id: Some(task.to_string()), ..seat.clone() })
}

/// Create the temp seat bound to `task`: one transaction, so two ticks cannot
/// create two seats for the same task.
///
/// The seat is created `Queued` — a temp worker that waits for a slot
/// (design/10: QUEUED happens only when the quota is exhausted) — and bound
/// to the task for life, so the reviewer of that task is found again.
fn create_temp_seat(
    store: &mut Store,
    project: &str,
    task: &str,
    profile: &str,
    now: Ts,
) -> GearResult<SeatRow> {
    store.write(|tx| {
        let name = next_temp_name(tx, project)?;
        let id = format!("{project}:{name}");
        let seat = SeatRow {
            id: id.clone(),
            project: project.to_string(),
            role: SeatRole::Temp,
            name,
            profile: profile.to_string(),
            generation: 0,
            state: SeatState::Queued,
            state_since: now,
            state_reason: Some("waiting for a review".to_string()),
            task_id: Some(task.to_string()),
        };
        tx.insert_seat(&seat)?;
        Ok(seat)
    })
}

/// The next free seat name: `review-1`, `review-2`, … (a warm review seat,
/// conventions 17 §8a).
fn next_temp_name(tx: &Tx<'_>, project: &str) -> GearResult<String> {
    let seats = tx.seats_of_project(project)?;
    let mut n = 1;
    loop {
        let name = format!("review-{n}");
        if !seats.iter().any(|seat| seat.name == name) {
            return Ok(name);
        }
        n += 1;
        if n > 1000 {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!("project {project} has too many temp seats"),
            ));
        }
    }
}

/// Whether a temp slot is free: the seats that occupy the quota are Starting,
/// Active, Releasing and Unknown (design/10).
fn has_free_slot(store: &mut Store, project: &str, quota: i64) -> GearResult<bool> {
    store.read(|tx| {
        let occupying = tx
            .seats_of_project(project)?
            .into_iter()
            .filter(|seat| seat.role == SeatRole::Temp && occupies_quota(seat.state))
            .count() as i64;
        Ok(occupying < quota)
    })
}

/// The states in which a temp seat occupies the quota (design/10).
fn occupies_quota(state: SeatState) -> bool {
    matches!(
        state,
        SeatState::Starting | SeatState::Active | SeatState::Releasing | SeatState::Unknown
    )
}

/// The outcome of one reviewer start.
enum Started {
    /// The agent is up; the review packet was delivered.
    Yes,
    /// The agent is not up: either a step to retry on the next tick (a pane
    /// that is still starting) or a seat that was parked for a human — its
    /// state says which.
    NotYet,
}

/// Give the reviewer its worktree, its environment, its agent and the review
/// packet.
///
/// The worktree is the one of the branch `review/t<n>`, cut at the candidate
/// the writer handed in, so the reviewer reads the code that was handed in.
/// A fresh worktree's shell needs a few seconds: `agent_pane_busy` right
/// after `worktree_create` is not a failure, the next tick tries again.
fn start_reviewer(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    config: &ProjectConfig,
    seat: &SeatRow,
    task: &ReviewTask,
    model: Option<&str>,
    profile: &Profile,
    now: Ts,
) -> GearResult<Started> {
    let repo = &config.project.repo;
    // The job's branch, and the seat's own anchor (its workspace label is
    // long-lived; the agent of a job is named after its task).
    let branch = review_branch(&task.task);
    let seat_branch = format!("home/{}", seat.name);
    let seat_label = format!("{}-{}", config.project.id, seat.name);
    let label = agent_name(config, &task.task);
    let kind = config
        .temps
        .as_ref()
        .map(|temps| temps.kind.clone())
        .unwrap_or_default();

    // 1. the pane: the seat's, from its binding (an interrupted start or a new
    //    job on a warm seat), else the seat's worktree — opened, or created
    //    once at the integration branch.
    let binding = store.read(|tx| tx.get_binding(&seat.id, seat.generation))?;
    let live = binding.as_ref().filter(|binding| binding.status != BindingStatus::Void);
    let pane = match live.map(|binding| binding.pane_id.clone()) {
        Some(pane) => pane,
        None => {
            // A warm seat is reopened by its path: its checkout is on the last
            // job's review branch, not on the seat's own branch.
            let by_path = match worktree_path(store, &seat.id)? {
                Some(path) => herdr.worktree_open_path(repo, &path, &seat_label),
                None => Outcome::Refused { code: "worktree_not_found".into(), message: String::new() },
            };
            let opened = match by_path {
                Outcome::Refused { code, .. } if code == "worktree_not_found" => {
                    herdr.worktree_open(repo, &seat_branch, &seat_label)
                }
                other => other,
            };
            let worktree = match opened {
                Outcome::Ok(worktree) => worktree,
                Outcome::Refused { code, .. } if code == "worktree_not_found" => {
                    match herdr.worktree_create(
                        repo,
                        &seat_branch,
                        &config.conventions.integration,
                        &seat_label,
                    ) {
                        Outcome::Ok(worktree) => worktree,
                        other => {
                            park(store, &seat.id, &step_failure("worktree_create", &other), now)?;
                            return Ok(Started::NotYet);
                        }
                    }
                }
                other => {
                    park(store, &seat.id, &step_failure("worktree_open", &other), now)?;
                    return Ok(Started::NotYet);
                }
            };
            store.write(|tx| tx.set_meta(&path_key(&seat.id), &worktree.path))?;
            worktree.pane_id
        }
    };

    // 2. a new job is a new generation of the seat: the previous binding is
    //    voided and a pending one is opened on the same pane. An interrupted
    //    start keeps the pending binding it already has.
    let pending = binding.as_ref().map(|binding| binding.status == BindingStatus::Pending).unwrap_or(false);
    let generation = if pending { seat.generation } else { seat.generation + 1 };
    if !pending {
        if let Err(reason) =
            store.write(|tx| gear_app::seats::begin_onboard(tx, &seat.id, &pane, &kind, now))
        {
            park(store, &seat.id, &format!("begin_onboard: {}", reason.detail), now)?;
            return Ok(Started::NotYet);
        }
    }

    // 3. the occupant of the pane. An agent this engine started for this very
    //    job is adopted (an engine restart cuts a start in half); any other
    //    agent is never typed into — the environment and the checkout would go
    //    into its conversation instead of into the shell's (F2 and F5 of the
    //    T-18 review).
    let occupant = match herdr.agent_get(&pane) {
        Ok(info) => info,
        Err(error) => {
            // The pane could not be looked at: never start on a guess.
            park(
                store,
                &seat.id,
                &format!("agent_get: {}", error.0),
                now,
            )?;
            return Ok(Started::NotYet);
        }
    };

    let started = match occupant {
        // The start was accepted before the restart: adopt it. Nothing is
        // typed into it — not the environment, not the checkout.
        Some(info) => {
            if !recorded_start(store, &seat.id, generation, &pane, &kind)? {
                // Another agent is in the pane: do not start, and never take
                // the pane from it. A human decides.
                park(
                    store,
                    &seat.id,
                    "the reviewer's pane is held by another agent",
                    now,
                )?;
                return Ok(Started::NotYet);
            }
            info
        }
        None => {
            // 4. the environment: the call context, and the model when the CLI
            //    takes one. It is set in the confirmed shell — the pane holds
            //    no agent — and before the checkout, so the shell has applied
            //    it long before the reviewer starts.
            let mut env_command = format!(
                "export GEAR_CTX={}:{}:{}; export GEAR_DB={}",
                config.project.id, seat.name, generation, cfg.db_path
            );
            if let Some(model) = model {
                if let Some(start) = model_start(&kind, model) {
                    if !start.env.is_empty() {
                        env_command = format!("{env_command}; {}", start.env);
                    }
                }
            }
            if !env_ready(store, herdr, &seat.id, generation, &pane, &env_command)? {
                // A fresh worktree's shell needs a few seconds: the next tick tries.
                return Ok(Started::NotYet);
            }

            // 5. the seat's checkout takes the job's branch at the approved
            //    candidate: this is also where the previous job's uncommitted
            //    changes go (never the ignored files, so `target/` and the
            //    build cache survive). The reviewer starts only once the
            //    engine has *seen* the checkout land on the candidate (F1).
            let Some(path) = worktree_path(store, &seat.id)? else {
                // The seat's worktree was never recorded: do not check out blind.
                park(store, &seat.id, "the seat's worktree path is unknown", now)?;
                return Ok(Started::NotYet);
            };
            if !checkout_ready(
                store,
                herdr,
                &seat.id,
                generation,
                &pane,
                &path,
                &branch,
                &task.candidate,
            )? {
                // The checkout is pending or failed: no reviewer starts on a
                // tree nobody checked out.
                return Ok(Started::NotYet);
            }

            // 6. the agent's args: the profile's unattended args, the model
            //    when the CLI takes one, and the session of an earlier review
            //    of the same task when the reviewer resumes it (F3).
            let args = reviewer_args(store, config, profile, &kind, model, &task.task)?;

            // The start is recorded before Herdr is asked: an engine restart
            // between the two must find the intent and adopt the agent it
            // asked for, and nothing else.
            record_start(store, &seat.id, generation, &pane, &kind, &label)?;
            // A grok worktree is trusted before its agent starts: grok asks on
            // the first start, while Herdr reports it idle, and the prompt
            // would be lost.
            if let Err(reason) = crate::trust::trust_worktree(&kind, &path, now) {
                // Nothing started, so the request goes: it must not authorize
                // the adoption of an agent that is not ours.
                store.write(|tx| tx.set_meta(&start_key(&seat.id, generation), ""))?;
                park(store, &seat.id, &format!("trust: {reason}"), now)?;
                return Ok(Started::NotYet);
            }
            match herdr.agent_start(&label, &kind, &pane, &args) {
                Outcome::Ok(info) => info,
                Outcome::Refused { code, .. } if code == "agent_pane_busy" => {
                    // The shell of a fresh worktree is still starting: the next
                    // tick tries again (the seat keeps its binding).
                    return Ok(Started::NotYet);
                }
                other => {
                    park(store, &seat.id, &step_failure("agent_start", &other), now)?;
                    return Ok(Started::NotYet);
                }
            }
        }
    };

    // 5. the seat is up and has its packet: one transaction. A crash between
    //    the two would leave a live reviewer that every later tick skips (an
    //    Active temp is not started again) and that never gets a job.
    let session_ref = started.agent_session.as_ref().map(|session| session.value.as_str());
    let packet = review_packet_for(store, config, &task.task, task.attempt, &task.candidate)?;
    store.write(|tx| {
        gear_app::seats::finish_onboard(tx, &seat.id, generation, session_ref, now)?;
        // The task's reviewer session is kept: a re-review of the same task
        // resumes it instead of starting a new conversation (F3).
        if let Some(session) = session_ref.filter(|session| !session.is_empty()) {
            tx.set_meta(&session_key(&config.project.id, &task.task), session)?;
        }
        tx.insert_op(&NewOp {
            project: config.project.id.clone(),
            seat: seat.id.clone(),
            generation,
            kind: OpKind::Deliver,
            task: Some(task.task.clone()),
            attempt: Some(task.attempt),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({ "text": packet }),
            created: now,
        })?;
        Ok(())
    })
    .map_err(|e| GearError::new(e.code, format!("finish_onboard: {}", e.detail)))?;
    Ok(Started::Yes)
}

/// The key of a seat's environment: set once per generation, before the
/// checkout, so the shell has applied it before the reviewer starts.
fn env_key(seat: &str, generation: i64) -> String {
    format!("engine.review_env.{seat}.{generation}")
}

/// Whether the environment of the seat's generation is set. It is set once: the
/// export is submitted before the checkout, so the shell applies it first and
/// the reviewer inherits it.
fn env_ready(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &str,
    generation: i64,
    pane: &str,
    command: &str,
) -> GearResult<bool> {
    let key = env_key(seat, generation);
    if store
        .read(|tx| tx.get_meta(&key))?
        .is_some_and(|done| !done.is_empty())
    {
        return Ok(true);
    }
    if let Outcome::Ok(()) = herdr.pane_run(pane, command) {
        store.write(|tx| tx.set_meta(&key, "1"))?;
        Ok(true)
    } else {
        // The pane refused the text: the next tick tries again.
        Ok(false)
    }
}

/// The key of a seat's checkout intent: the token of the checkout the engine
/// submitted for that generation. A restart must not forget it (F1).
fn checkout_key(seat: &str, generation: i64) -> String {
    format!("engine.review_checkout.{seat}.{generation}")
}

/// The key of the counter that gives every checkout a fresh token.
fn checkout_seq_key() -> String {
    "engine.review_checkout_seq".to_string()
}

/// The key of the recorded start: the agent this engine asked Herdr to start
/// for that generation. Only such an agent may be adopted (F5).
fn start_key(seat: &str, generation: i64) -> String {
    format!("engine.review_start.{seat}.{generation}")
}

/// The key of a task's reviewer session: the session a re-review resumes (F3).
fn session_key(project: &str, task: &str) -> String {
    format!("engine.review_session.{project}.{task}")
}

/// The marker a finished checkout prints: `GEAR_CK_OK_<token>_<sha>_<branch>`.
const OK_MARKER: &str = "GEAR_CK_OK_";
/// The marker a failed checkout prints: `GEAR_CK_FAIL_<token>`.
const FAIL_MARKER: &str = "GEAR_CK_FAIL_";
/// How many lines of the pane the engine reads to see what the checkout did.
const CHECKOUT_TAIL_LINES: u32 = 24;

/// The checkout of a job: the seat's worktree switches to the job's branch at
/// the candidate — the previous job's uncommitted changes go with it (never the
/// ignored files, so `target/` and the build cache survive) — and the chain
/// reports what it did: the commit it landed on and the branch.
///
/// The token is a shell variable, so the echo of the command line (which the
/// pane shows) carries the marker text with the variable unexpanded and can
/// never be read as the report of a finished checkout.
fn checkout_command(path: &str, branch: &str, candidate: &str, token: &str) -> String {
    format!(
        "T={token}; cd {path} && git fetch origin && git checkout -f -B {branch} {candidate} \
         && git clean -fd && echo {ok}${{T}}_$(git rev-parse HEAD)_$(git rev-parse --abbrev-ref HEAD) \
         || echo {fail}${{T}}",
        ok = OK_MARKER,
        fail = FAIL_MARKER,
    )
}

/// What the checkout chain reported.
enum Checkout {
    /// The chain is still running: it has printed nothing yet.
    Pending,
    /// The chain finished: the checkout is at `sha` on `branch`.
    Finished { sha: String, branch: String },
    /// The chain failed: it printed the failure marker.
    Failed,
}

/// Read the checkout's report out of the pane's tail.
///
/// Only a line that *is* the report counts: the pane also shows the echo of
/// the command line, and that echo names no commit.
fn observe_checkout(tail: &str, token: &str) -> Checkout {
    let ok = format!("{OK_MARKER}{token}_");
    let fail = format!("{FAIL_MARKER}{token}");
    let mut finished: Option<(String, String)> = None;
    let mut failed = false;
    for line in tail.lines() {
        let line = line.trim();
        if line == fail {
            failed = true;
            continue;
        }
        let Some(rest) = line.split(&ok).nth(1) else {
            continue;
        };
        let Some((sha, branch)) = rest.split_once('_') else {
            continue;
        };
        if !is_commit_id(sha) {
            continue;
        }
        finished = Some((sha.to_string(), branch.trim().to_string()));
    }
    match finished {
        Some((sha, branch)) => Checkout::Finished { sha, branch },
        None if failed => Checkout::Failed,
        None => Checkout::Pending,
    }
}

/// Whether `text` looks like a git object id: hex, at least seven characters.
fn is_commit_id(text: &str) -> bool {
    text.len() >= 7 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// Whether the commit `sha` is the one `candidate` names (a candidate may be
/// short: seven characters are enough to name a commit).
fn same_commit(sha: &str, candidate: &str) -> bool {
    let sha = sha.trim().to_lowercase();
    let candidate = candidate.trim().to_lowercase();
    !candidate.is_empty() && (sha == candidate || sha.starts_with(&candidate))
}

/// Make sure the seat's worktree is at the job's branch and candidate — and
/// that the engine has *seen* it happen.
///
/// The first call submits the checkout and waits: `pane_run` only submits the
/// text, it neither waits for git nor reports the exit status, so the engine
/// reads the pane and looks for the chain's own report. A pending or a failed
/// chain starts no reviewer (F1 of the T-18 review).
fn checkout_ready(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &str,
    generation: i64,
    pane: &str,
    path: &str,
    branch: &str,
    candidate: &str,
) -> GearResult<bool> {
    let key = checkout_key(seat, generation);
    let recorded = store
        .read(|tx| tx.get_meta(&key))?
        .filter(|token| !token.is_empty());
    let Some(token) = recorded else {
        // The checkout was never submitted: submit it and wait for its report.
        submit_checkout(store, herdr, seat, generation, pane, path, branch, candidate)?;
        return Ok(false);
    };
    // The chain ran on an earlier tick: read what it did.
    let tail = herdr.read_tail(pane, CHECKOUT_TAIL_LINES).unwrap_or_default();
    match observe_checkout(&tail, &token) {
        Checkout::Finished { sha, branch: got } => {
            if same_commit(&sha, candidate) && got == branch {
                Ok(true)
            } else {
                // It landed somewhere else: check out again.
                submit_checkout(store, herdr, seat, generation, pane, path, branch, candidate)?;
                Ok(false)
            }
        }
        Checkout::Failed => {
            // The fetch or the checkout failed: try again with a fresh token,
            // so the old report is not read as the new one's.
            submit_checkout(store, herdr, seat, generation, pane, path, branch, candidate)?;
            Ok(false)
        }
        Checkout::Pending => Ok(false),
    }
}

/// Submit the checkout of a job and remember its token, so that the next tick
/// can read the chain's report.
fn submit_checkout(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &str,
    generation: i64,
    pane: &str,
    path: &str,
    branch: &str,
    candidate: &str,
) -> GearResult<()> {
    let token = next_checkout_token(store)?;
    let command = checkout_command(path, branch, candidate, &token);
    store.write(|tx| tx.set_meta(&checkout_key(seat, generation), &token))?;
    if let Outcome::Ok(()) = herdr.pane_run(pane, &command) {
        Ok(())
    } else {
        // The pane refused the text: the intent is void, the next tick tries
        // again with a fresh token.
        store.write(|tx| tx.set_meta(&checkout_key(seat, generation), ""))?;
        Ok(())
    }
}

/// A fresh token for a checkout: every submission gets its own, so the report
/// of an earlier attempt is never read as this one's.
fn next_checkout_token(store: &mut Store) -> GearResult<String> {
    store.write(|tx| {
        let next = tx
            .get_meta(&checkout_seq_key())?
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        tx.set_meta(&checkout_seq_key(), &next.to_string())?;
        Ok(format!("{next:x}"))
    })
}

/// Whether this engine recorded the start of the agent in `pane`: an adopt is
/// only allowed for a start GEAR itself asked for, never for an agent that
/// happens to be of the same kind (F5 of the T-18 review).
fn recorded_start(
    store: &mut Store,
    seat: &str,
    generation: i64,
    pane: &str,
    kind: &str,
) -> GearResult<bool> {
    let Some(raw) = store.read(|tx| tx.get_meta(&start_key(seat, generation)))? else {
        return Ok(false);
    };
    let Ok(record) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(false);
    };
    Ok(record.get("pane").and_then(|value| value.as_str()) == Some(pane)
        && record.get("kind").and_then(|value| value.as_str()) == Some(kind))
}

/// Record that this engine is about to start `label` in `pane`. The record is
/// written before Herdr is asked, so a restart in between still knows which
/// agent it may adopt.
fn record_start(
    store: &mut Store,
    seat: &str,
    generation: i64,
    pane: &str,
    kind: &str,
    label: &str,
) -> GearResult<()> {
    let record = serde_json::json!({ "pane": pane, "kind": kind, "label": label });
    store.write(|tx| tx.set_meta(&start_key(seat, generation), &record.to_string()))
}

/// The args a reviewer starts with: the profile's unattended args, the model
/// when the CLI takes one, and — when the task was reviewed before — the
/// profile's resume args with that session, so a re-review resumes the job's
/// session instead of starting a new conversation (F3 of the T-18 review).
fn reviewer_args(
    store: &mut Store,
    config: &ProjectConfig,
    profile: &Profile,
    kind: &str,
    model: Option<&str>,
    task: &str,
) -> GearResult<Vec<String>> {
    let mut args = profile.unattended_args.clone();
    if let Some(model) = model {
        if let Some(start) = model_start(kind, model) {
            args.extend(start.args);
        }
    }
    let session = store
        .read(|tx| tx.get_meta(&session_key(&config.project.id, task)))?
        .filter(|session| !session.is_empty());
    if let Some(session) = session {
        if let Some(resume) = &profile.resume_args {
            args.extend(resume.iter().map(|arg| arg.replace("{session}", &session)));
        }
    }
    Ok(args)
}

/// The review packet of an attempt: the contract's goal and accept, the
/// candidate, the diff range, the rules and the verdict command.
fn review_packet_for(
    store: &mut Store,
    config: &ProjectConfig,
    task: &str,
    attempt: i64,
    candidate: &str,
) -> GearResult<String> {
    let (goal, accept) = store.read(|tx| {
        let version = tx
            .get_attempt(&config.project.id, task, attempt)?
            .map(|row| row.adopted_version)
            .unwrap_or(1);
        match tx.get_contract(&config.project.id, task, version)? {
            Some(contract) => Ok((contract.goal, contract.accept)),
            None => Ok((String::new(), String::new())),
        }
    })?;
    Ok(review_packet(
        task,
        attempt,
        &goal,
        &accept,
        candidate,
        &config.conventions.integration,
        &review_branch(task),
    ))
}

/// Reviewers that are up but never got their packet — the state an engine
/// restart could leave behind before the activation and the delivery were one
/// transaction. Each one gets its packet here, once: the absence of any
/// Deliver op for that seat is the guard, so the next tick adds nothing.
fn recover_missing_packets(
    store: &mut Store,
    project: &str,
    config: &ProjectConfig,
    now: Ts,
) -> GearResult<Vec<String>> {
    let mut recovered = Vec::new();
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats {
        if seat.role != SeatRole::Temp || seat.state != SeatState::Active {
            continue;
        }
        let Some(task) = seat.task_id.clone() else {
            continue;
        };
        if store
            .read(|tx| tx.last_op(&seat.id, OpKind::Deliver))?
            .is_some_and(|op| !matches!(op.state, gear_model::OpState::Failed | gear_model::OpState::Void))
        {
            // It has its packet, or it is sending it. A refused send (Failed,
            // e.g. `agent_not_ready` right after the start) is sent again.
            continue;
        }
        let Some(attempt) = store
            .read(|tx| tx.attempts_of_task(project, &task))?
            .into_iter()
            .last()
        else {
            continue;
        };
        let Some(candidate) = attempt.candidate.clone() else {
            continue;
        };
        let packet = review_packet_for(store, config, &task, attempt.n, &candidate)?;
        let generation = seat.generation;
        store.write(|tx| {
            tx.insert_op(&NewOp {
                project: project.to_string(),
                seat: seat.id.clone(),
                generation,
                kind: OpKind::Deliver,
                task: Some(task.clone()),
                attempt: Some(attempt.n),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({ "text": packet }),
                created: now,
            })?;
            Ok(())
        })?;
        recovered.push(seat.id.clone());
    }
    Ok(recovered)
}

/// Where a seat's worktree path is kept (the seat row has no field for it).
fn path_key(seat: &str) -> String {
    format!("engine.review_path.{seat}")
}

/// The seat's worktree path, once its first job created the worktree.
fn worktree_path(store: &mut Store, seat: &str) -> GearResult<Option<String>> {
    store.read(|tx| tx.get_meta(&path_key(seat)))
}

/// The review branch of a task: `review/t<n>` (conventions 17 §3: the
/// reviewers' branch).
fn review_branch(task: &str) -> String {
    format!("review/{}", task.to_lowercase().replace('-', ""))
}

/// The Herdr agent name (and workspace label) of a task's reviewer:
/// `<project>-review-<task-id>` — project prefix plus what the agent is for
/// (kickoff naming rule, 17 §2).
fn agent_name(config: &ProjectConfig, task: &str) -> String {
    format!(
        "{}-review-{}",
        config.project.id,
        task.to_lowercase().replace('-', "")
    )
}

/// The packet the reviewer gets: what to review, where the rules are and the
/// exact command to hand the verdict in.
fn review_packet(
    task: &str,
    attempt: i64,
    goal: &str,
    accept: &str,
    candidate: &str,
    integration: &str,
    branch: &str,
) -> String {
    format!(
        "Review {task} attempt {attempt}. Run `gear` first (it records that you got this).\n\
         \n\
         Goal: {goal}\n\
         Accept: {accept}\n\
         Candidate: {candidate}\n\
         Diff to review: `{integration}..{candidate}`\n\
         Your worktree: branch `{branch}`, checked out at the candidate.\n\
         \n\
         Rules: `gear --skill review` — the mechanical acceptance you run yourself, \
         the findings with their severity, the review template and the verdict rule.\n\
         \n\
         Write the review in your worktree at the path your project uses for reviews, commit it \
         on `{branch}` and push it (the candidate stays untouched).\n\
         \n\
         Hand your verdict in with exactly:\n\
         gear send --kind review --task {task} --attempt {attempt} --verdict <pass|fail> \
         \"<the path to your review file, or the text>\"\n\
         \n\
         GEAR reads the file and keeps the text of your findings, so the writer reads them from \
         GEAR (the rework note and the task packet): your worktree and your branch may be gone \
         afterwards.\n\
         \n\
         A `fail` becomes the writer's rework: write every finding so the writer can act on it \
         without asking."
    )
}

/// Tell the captain that a review waits: one open item per task, so the tick
/// does not fill the inbox with the same news.
fn tell_captain(store: &mut Store, project: &str, task: &ReviewTask, now: Ts) -> GearResult<()> {
    store.write(|tx| {
        let captain = tx.captain_of(project)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("project {project} has no captain"))
        })?;
        // One open item per task: the captain is told once, not every tick.
        let already = tx
            .actionable_for(project, &captain.id, now)?
            .into_iter()
            .any(|message| message.task.as_deref() == Some(task.task.as_str()));
        if already {
            return Ok(());
        }
        let seq = next_thread_seq(tx, project, &captain.id, now, &task.task)?;
        let sender_gen = tx
            .get_seat(&task.writer)?
            .map(|seat| seat.generation)
            .unwrap_or(0);
        let message = NewMessage {
            project: project.to_string(),
            thread: format!("{}:{seq}", task.task),
            task: Some(task.task.clone()),
            attempt: Some(task.attempt),
            sender: task.writer.clone(),
            sender_gen,
            recipient: captain.id.clone(),
            kind: MsgKind::Question,
            urgent: false,
            body: format!(
                "The review of {} (attempt {}) waits: every temp seat is in use, so the \
                 reviewer is queued. The lane is not blocked and carries on with its next task.",
                task.task, task.attempt
            ),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        };
        tx.insert_message(&message)?;
        Ok(())
    })
}

/// The next free thread number for `prefix`: the highest one still open, plus
/// one, skipping threads that exist (the same rule as `gear send`).
fn next_thread_seq(
    tx: &Tx<'_>,
    project: &str,
    recipient: &str,
    now: Ts,
    prefix: &str,
) -> GearResult<u32> {
    let mut seq = tx
        .actionable_for(project, recipient, now)?
        .iter()
        .filter_map(|message| {
            message
                .thread
                .strip_prefix(prefix)?
                .strip_prefix(':')?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0);
    loop {
        seq += 1;
        if seq > 10_000 {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!("too many threads named {prefix}:* in project {project}"),
            ));
        }
        if tx
            .thread_messages(project, &format!("{prefix}:{seq}"))?
            .is_empty()
        {
            return Ok(seq);
        }
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

/// A failed start: the seat's outcome is unknown, a human decides (design/10:
/// "Start timeout / unknown outcome → UNKNOWN (occupies quota); no second one
/// is opened"). A temp seat has no direct way from Queued to Unknown, so it
/// passes through Starting (gear-model's temp transitions).
fn park(store: &mut Store, seat_id: &str, reason: &str, now: Ts) -> GearResult<()> {
    store.write(|tx| {
        let seat = tx.get_seat(seat_id)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("no seat {seat_id}"))
        })?;
        // A queued or freed (warm) seat that fails to start passes through
        // Starting, so its failure is recorded instead of failing the tick.
        let from = if matches!(seat.state, SeatState::Queued | SeatState::Released) {
            gear_model::state::require_transition(
                SeatState::can_transition(seat.role, seat.state, SeatState::Starting),
                "seat",
                seat.state.as_str(),
                SeatState::Starting.as_str(),
            )?;
            tx.set_seat_state(seat_id, SeatState::Starting, Some("starting"), now)?;
            SeatState::Starting
        } else {
            seat.state
        };
        gear_model::state::require_transition(
            SeatState::can_transition(seat.role, from, SeatState::Unknown),
            "seat",
            from.as_str(),
            SeatState::Unknown.as_str(),
        )?;
        tx.set_seat_state(seat_id, SeatState::Unknown, Some(reason), now)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_model::{OpState, TaskState};
    use gear_store::{testing, BindingRow, BindingStatus, ContractRow, ProjectRow, TaskRow};

    const T0: i64 = 1_800_000_000;

    /// A project with one lane, one reviewer kind (codex) and one free temp
    /// slot.
    const CONFIG: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 1

[captain]
pane = "w1:p9"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[temps]
kind = "codex"
profile = "codex"

[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash"]
review = ["gpt-6.1-sol"]
"#;

    /// The same project without a free temp slot.
    const CONFIG_NO_SLOT: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p9"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[temps]
kind = "codex"
profile = "codex"

[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash"]
review = ["gpt-6.1-sol"]
"#;

    /// An engine that knows the codex profile.
    fn cfg() -> EngineCfg {
        let mut cfg = EngineCfg { db_path: "/state/gear.db".into(), ..Default::default() };
        for kind in ["codex", "opencode", "claude"] {
            cfg.profiles
                .insert(kind.to_string(), gear_herdr::builtin(kind).unwrap());
        }
        cfg
    }

    /// A project with one lane whose task T-1 was handed in (the attempt is
    /// Submitted with a candidate, the task is Verifying).
    fn world(config_toml: &str) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&ProjectRow {
                    id: "p".into(),
                    herdr_session: "sess".into(),
                    config_toml: config_toml.into(),
                    config_version: 1,
                    quota_lanes: 1,
                    quota_temps: 1,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: T0,
                })?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p1", "opencode", T0)?;
                testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p9", "claude", T0)?;
                tx.insert_task(&TaskRow {
                    project: "p".into(),
                    id: "T-1".into(),
                    rank: 1,
                    state: TaskState::Ready,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    updated: T0,
                })?;
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    version: 1,
                    goal: "the goal".into(),
                    accept: "the accept criteria".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                })?;
                // The lane hands the result in.
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
                gear_app::tasks::submit_result(
                    tx,
                    "p",
                    "T-1",
                    1,
                    1,
                    "abc1234",
                    "ops/reports/T-1.md",
                    T0,
                )?;
                Ok(())
            })
            .unwrap();
        store
    }

    /// The worktrees the engine created: (branch, base).
    fn worktrees(herdr: &FakeHerdr) -> Vec<(String, String)> {
        herdr.calls
            .iter()
            .filter_map(|call| match call {
                Call::WorktreeCreate { branch, base, .. } => {
                    Some((branch.clone(), base.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// The agents the engine started: (kind, args).
    fn starts(herdr: &FakeHerdr) -> Vec<(String, Vec<String>)> {
        herdr.calls
            .iter()
            .filter_map(|call| match call {
                Call::AgentStart { kind, args, .. } => Some((kind.clone(), args.clone())),
                _ => None,
            })
            .collect()
    }

    /// The shell commands the engine typed into a pane.
    fn pane_runs(herdr: &FakeHerdr, pane: &str) -> Vec<String> {
        herdr.calls
            .iter()
            .filter_map(|call| match call {
                Call::PaneRun { pane: p, command } if p == pane => Some(command.clone()),
                _ => None,
            })
            .collect()
    }

    /// The pane of a seat's binding.
    fn pane_of(store: &mut Store, seat: &str, generation: i64) -> String {
        store
            .read(|tx| tx.get_binding(seat, generation))
            .unwrap()
            .expect("the seat's binding")
            .pane_id
    }

    /// Emulate the shell that ran the checkout: the pane shows the report of a
    /// checkout that landed on `sha` and `branch`. The FakeHerdr has no shell
    /// of its own — the test writes what the shell would print.
    fn confirm_checkout(
        store: &mut Store,
        herdr: &mut FakeHerdr,
        seat: &str,
        generation: i64,
        sha: &str,
        branch: &str,
    ) {
        let token = store
            .read(|tx| tx.get_meta(&checkout_key(seat, generation)))
            .unwrap()
            .expect("the checkout's token");
        let pane = pane_of(store, seat, generation);
        herdr.set_screen(&pane, &format!("{OK_MARKER}{token}_{sha}_{branch}"));
    }

    /// Emulate a checkout that failed: the pane shows the failure marker.
    fn fail_checkout(store: &mut Store, herdr: &mut FakeHerdr, seat: &str, generation: i64) {
        let token = store
            .read(|tx| tx.get_meta(&checkout_key(seat, generation)))
            .unwrap()
            .expect("the checkout's token");
        let pane = pane_of(store, seat, generation);
        herdr.set_screen(&pane, &format!("{FAIL_MARKER}{token}"));
    }

    /// R1 (review): the project's profile table reaches the reviewer start — a
    /// reviewer on a profile the config defines starts with that kind.
    #[test]
    fn a_configured_reviewer_profile_starts_the_reviewer() {
        let text = CONFIG
            .replace(
                "[temps]\nkind = \"codex\"\nprofile = \"codex\"",
                "[temps]\nkind = \"grok\"\nprofile = \"grok\"",
            )
            + "\n[profiles.grok]\nkind = \"grok\"\nunattended_args = [\"--yolo\"]\ntail_lines = 16\nconfirm_rounds = 1\n";
        let mut store = world(&text);
        let mut herdr = FakeHerdr::new();
        let config = gear_app::config::parse(&text).unwrap();
        let profiles = gear_app::config::profile_table(&gear_herdr::builtins(), &config).unwrap();

        // The first tick submits the checkout; the reviewer starts once the
        // shell reports it (T-18's flow).
        start_reviews(&mut store, &mut herdr, &cfg(), &profiles, "p", T0 + 10).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &profiles, "p", T0 + 20).unwrap();

        assert_eq!(report.started, vec!["p:review-1".to_string()], "a reviewer starts on the configured profile");
        let starts: Vec<(String, Vec<String>)> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::AgentStart { kind, args, .. } => Some((kind.clone(), args.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 1, "{starts:?}");
        assert_eq!(starts[0].0, "grok");
        assert!(starts[0].1.contains(&"--yolo".to_string()), "{:?}", starts[0].1);
    }

    /// R1 (review): an override of a built-in profile reaches the start — the
    /// custom unattended args replace the built-in's.
    #[test]
    fn a_builtin_reviewer_override_reaches_the_start() {
        let text = CONFIG.to_string()
            + "\n[profiles.codex]\nunattended_args = [\"--custom-flag\"]\n";
        let mut store = world(&text);
        let mut herdr = FakeHerdr::new();
        let config = gear_app::config::parse(&text).unwrap();
        let profiles = gear_app::config::profile_table(&gear_herdr::builtins(), &config).unwrap();

        start_reviews(&mut store, &mut herdr, &cfg(), &profiles, "p", T0 + 10).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        start_reviews(&mut store, &mut herdr, &cfg(), &profiles, "p", T0 + 20).unwrap();

        let starts: Vec<(String, Vec<String>)> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::AgentStart { kind, args, .. } => Some((kind.clone(), args.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 1, "{starts:?}");
        assert!(starts[0].1.contains(&"--custom-flag".to_string()), "{:?}", starts[0].1);
    }

    #[test]
    fn a_result_in_starts_one_reviewer() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        // The first tick: the seat and its worktree are made and the checkout
        // is submitted. The reviewer waits — the engine has not seen the
        // checkout land on the candidate yet (F1 of the T-18 review).
        let first = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        assert!(first.started.is_empty(), "{first:?}");
        assert_eq!(first.waiting, vec!["T-1".to_string()]);
        assert!(starts(&herdr).is_empty(), "no reviewer before the checkout");
        // one worktree, on the seat's own branch, cut at the integration branch
        assert_eq!(
            worktrees(&herdr),
            vec![("home/review-1".to_string(), "dev".to_string())]
        );

        // The shell reports the checkout: the reviewer starts.
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();

        // one temp seat, bound to the task for life
        assert_eq!(report.started, vec!["p:review-1".to_string()]);
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.role, SeatRole::Temp);
        assert_eq!(seat.task_id.as_deref(), Some("T-1"));
        assert_eq!(seat.state, SeatState::Active);

        // still one worktree, on the review branch, cut at the candidate
        assert_eq!(
            worktrees(&herdr),
            vec![("home/review-1".to_string(), "dev".to_string())]
        );

        // one agent, the reviewer's kind, on the review model
        assert_eq!(
            starts(&herdr),
            vec![(
                "codex".to_string(),
                vec![
                    "--dangerously-bypass-approvals-and-sandbox".to_string(),
                    "-m".to_string(),
                    "gpt-6.1-sol".to_string(),
                    "-c".to_string(),
                    "model_reasoning_effort=high".to_string(),
                ]
            )]
        );

        // one delivery, with the review packet
        let ops = store.read(|tx| tx.ops_in_state("p", OpState::Intended)).unwrap();
        assert_eq!(ops.len(), 1, "{ops:?}");
        let op = &ops[0];
        assert_eq!(op.kind, OpKind::Deliver);
        assert_eq!(op.seat, "p:review-1");
        assert_eq!(op.task.as_deref(), Some("T-1"));
        let text = op.payload.get("text").and_then(|t| t.as_str()).unwrap();
        assert!(text.contains("the goal"), "{text}");
        assert!(text.contains("the accept criteria"), "{text}");
        assert!(text.contains("abc1234"), "{text}");
        assert!(text.contains("dev..abc1234"), "{text}");
        // The rules come from the binary, not from a file of the gear repo
        // (T-30): the packet names `gear --skill review` and no `ops/` path.
        assert!(text.contains("gear --skill review"), "{text}");
        assert!(!text.contains("ops/"), "no gear-repo path in the packet:\n{text}");
        assert!(
            text.contains(
                "gear send --kind review --task T-1 --attempt 1 --verdict <pass|fail> \
                 \"<the path to your review file, or the text>\""
            ),
            "{text}"
        );
        // The findings live in GEAR's store (T-21, shipped): the writer reads
        // the text from the rework note and the packet, not from the branch.
        assert!(text.contains("keeps the text of your findings"), "{text}");
        assert!(!text.contains("git show"), "no branch pointer in the packet:\n{text}");
    }

    #[test]
    fn the_environment_is_set_before_the_checkout_and_only_once() {
        // The export is submitted before the checkout, so the shell has
        // applied it long before the reviewer starts; and it is submitted
        // once per generation, so the pane's screen keeps the checkout's
        // report in sight.
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        // A second tick while the checkout is still running.
        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 30).unwrap();

        let commands = pane_runs(&herdr, "wt1:p1");
        let export = commands
            .iter()
            .position(|command| command.contains("export GEAR_CTX="))
            .expect("the export");
        let checkout = commands
            .iter()
            .position(|command| command.contains("git checkout"))
            .expect("the checkout");
        assert!(export < checkout, "the export comes first: {commands:?}");
        assert_eq!(
            commands.iter().filter(|command| command.contains("export GEAR_CTX=")).count(),
            1,
            "the export is set once: {commands:?}"
        );
    }

    #[test]
    fn a_failed_checkout_starts_no_reviewer() {
        // F1 of the T-18 review: the checkout must be *seen* to land on the
        // candidate. A failed fetch prints the failure marker: no reviewer
        // starts, and the engine tries again with a fresh token.
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        let first_token = store
            .read(|tx| tx.get_meta(&checkout_key("p:review-1", 1)))
            .unwrap()
            .unwrap();
        fail_checkout(&mut store, &mut herdr, "p:review-1", 1);

        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();

        assert!(report.started.is_empty(), "{report:?}");
        assert_eq!(report.waiting, vec!["T-1".to_string()]);
        assert!(starts(&herdr).is_empty(), "no reviewer on a failed checkout");
        // The seat is not parked: the next tick tries again.
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Starting);
        // The checkout was submitted again, with a token of its own.
        let checkouts: Vec<String> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::PaneRun { command, .. } if command.contains("git checkout") => {
                    Some(command.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(checkouts.len(), 2, "{checkouts:?}");
        let second_token = store
            .read(|tx| tx.get_meta(&checkout_key("p:review-1", 1)))
            .unwrap()
            .unwrap();
        assert_ne!(first_token, second_token, "a fresh token for the new attempt");
        assert!(checkouts[0].contains(&format!("T={first_token}")), "{checkouts:?}");
        assert!(checkouts[1].contains(&format!("T={second_token}")), "{checkouts:?}");
    }

    #[test]
    fn a_pending_checkout_starts_no_reviewer() {
        // F1 of the T-18 review: `pane_run` only submits the text — it does
        // not wait for git. A checkout that has not reported yet starts no
        // reviewer, and the reviewer starts as soon as the report is there.
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        // The shell has printed nothing yet: the checkout is still running.
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();
        assert!(report.started.is_empty(), "{report:?}");
        assert!(starts(&herdr).is_empty(), "no reviewer while the checkout runs");

        // The report arrives.
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 30).unwrap();
        assert_eq!(report.started, vec!["p:review-1".to_string()]);
    }

    #[test]
    fn a_checkout_on_another_commit_starts_no_reviewer() {
        // The chain reported a commit that is not the candidate: the reviewer
        // would read the wrong tree, so it does not start and the checkout
        // runs again.
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "9999999", "review/t1");

        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();

        assert!(report.started.is_empty(), "{report:?}");
        assert!(starts(&herdr).is_empty(), "no reviewer on the wrong commit");
    }

    #[test]
    fn the_tick_starts_the_reviewer_and_delivers_the_packet() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        // The lane and the captain are there, so the tick observes them.
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Idle);
        herdr.add_agent("w1:p9", "claude", gear_herdr::AgentStatus::Idle);

        // The first tick submits the checkout; the reviewer waits for its
        // report.
        crate::tick(&mut store, &mut herdr, &cfg(), T0 + 10).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");

        let report = crate::tick(&mut store, &mut herdr, &cfg(), T0 + 20).unwrap();

        assert_eq!(report.reviewers, vec!["p:review-1".to_string()]);
        // The review packet reached the reviewer's pane: one delivery, sent.
        let prompts = herdr.prompts_to("wt1:p1");
        assert_eq!(prompts.len(), 1, "{prompts:?}");
        assert!(prompts[0].starts_with("Review T-1 attempt 1."), "{}", prompts[0]);
        // … and the delivery waits for the verdict, not for a view.
        let op = store
            .read(|tx| tx.get_op("p", "O-1"))
            .unwrap()
            .expect("the review delivery");
        assert_eq!(op.state, OpState::Submitted);
    }

    #[test]
    fn the_second_tick_starts_no_second_reviewer() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 30).unwrap();

        assert!(report.started.is_empty(), "{:?}", report.started);
        assert_eq!(worktrees(&herdr).len(), 1);
        assert_eq!(starts(&herdr).len(), 1);
        // Still one temp seat for the task.
        let seats = store.read(|tx| tx.seats_of_project("p")).unwrap();
        let reviewers: Vec<_> = seats
            .iter()
            .filter(|seat| seat.role == SeatRole::Temp)
            .collect();
        assert_eq!(reviewers.len(), 1);
    }

    #[test]
    fn without_a_free_slot_no_reviewer_starts() {
        let mut store = world(CONFIG_NO_SLOT);
        let mut herdr = FakeHerdr::new();

        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();

        assert!(report.started.is_empty());
        assert_eq!(report.waiting, vec!["T-1".to_string()]);
        assert!(worktrees(&herdr).is_empty());
        assert!(starts(&herdr).is_empty());
        // No seat is built for a job that cannot start: the quota has no room
        // and a cold extra seat is exactly what finding 2 forbids.
        assert!(
            store.read(|tx| tx.get_seat("p:review-1")).unwrap().is_none(),
            "no seat is created while the quota is full"
        );
        // The captain is told once.
        let inbox = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 11)).unwrap();
        assert_eq!(inbox.len(), 1, "the captain is told: {inbox:?}");
        assert!(inbox[0].body.contains("T-1"), "{}", inbox[0].body);
        // The next tick does not tell the captain again.
        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();
        let inbox = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 21)).unwrap();
        assert_eq!(inbox.len(), 1, "one open item per task: {inbox:?}");
    }

    #[test]
    fn a_pane_that_is_not_ready_is_tried_again_on_the_next_tick() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        // A reviewer that got its worktree and its binding, but whose agent is
        // not up yet (the shell of a fresh worktree needs a few seconds).
        let pane = "wt1:p1";
        herdr.add_shell(pane);
        store
            .write(|tx| {
                tx.insert_seat(&SeatRow {
                    id: "p:review-1".into(),
                    project: "p".into(),
                    role: SeatRole::Temp,
                    name: "review-1".into(),
                    profile: "codex".into(),
                    generation: 1,
                    state: SeatState::Starting,
                    state_since: T0,
                    state_reason: Some("onboarding".into()),
                    task_id: Some("T-1".into()),
                })?;
                tx.insert_binding(&BindingRow {
                    seat: "p:review-1".into(),
                    generation: 1,
                    session: "sess".into(),
                    pane_id: pane.into(),
                    kind: "codex".into(),
                    session_ref: None,
                    status: BindingStatus::Pending,
                    handover_id: None,
                    created: T0,
                })?;
                // The first job recorded the seat's worktree path.
                tx.set_meta(&path_key("p:review-1"), "/fake-worktrees/home-review-1")?;
                Ok(())
            })
            .unwrap();

        // The first tick submits the checkout.
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        assert!(report.started.is_empty(), "the checkout is not confirmed yet");
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");

        // The shell is still starting: the agent start is refused.
        herdr.refuse_next_start = Some("agent_pane_busy".into());
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();

        assert!(report.started.is_empty(), "the agent is not up yet");
        assert_eq!(report.waiting, vec!["T-1".to_string()]);
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Starting, "not parked: tried again");
        // The next tick starts the agent: the worktree and the environment are
        // already there.
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 30).unwrap();
        assert_eq!(report.started, vec!["p:review-1".to_string()]);
        // The refused call was the agent's start; the start happens once.
        assert_eq!(herdr.started_agents.len(), 1, "the agent starts once");
        assert_eq!(starts(&herdr).len(), 2, "the refused start and the one that worked");
        assert!(worktrees(&herdr).is_empty(), "no second worktree");
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active);
    }

    #[test]
    fn an_accepted_start_is_adopted_not_repeated() {
        // P1 #1 of the review: an engine restart can cut the start between
        // Herdr accepting it and the seat going Active. The pane then holds a
        // live reviewer with a Pending binding; the tick must adopt it, never
        // start a second one (a second start is refused and would park a live
        // reviewer without its job).
        //
        // F2 of the T-18 review: nothing is typed into that live agent — not
        // the environment, not the checkout. The export is agent input, and it
        // would go into the reviewer's conversation.
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        let pane = "wt1:p1";
        herdr.add_shell(pane);
        // Herdr accepted the start: the codex agent is in the pane.
        herdr.add_agent(pane, "codex", gear_herdr::AgentStatus::Idle);
        store
            .write(|tx| {
                tx.insert_seat(&SeatRow {
                    id: "p:review-1".into(),
                    project: "p".into(),
                    role: SeatRole::Temp,
                    name: "review-1".into(),
                    profile: "codex".into(),
                    generation: 1,
                    state: SeatState::Starting,
                    state_since: T0,
                    state_reason: Some("onboarding".into()),
                    task_id: Some("T-1".into()),
                })?;
                tx.insert_binding(&BindingRow {
                    seat: "p:review-1".into(),
                    generation: 1,
                    session: "sess".into(),
                    pane_id: pane.into(),
                    kind: "codex".into(),
                    session_ref: Some("ses_reviewer".into()),
                    status: BindingStatus::Pending,
                    handover_id: None,
                    created: T0,
                })?;
                // The engine recorded the start it asked Herdr for.
                tx.set_meta(
                    &start_key("p:review-1", 1),
                    &serde_json::json!({ "pane": pane, "kind": "codex", "label": "p-review-t1" })
                        .to_string(),
                )?;
                Ok(())
            })
            .unwrap();

        let report = crate::tick(&mut store, &mut herdr, &cfg(), T0 + 10).unwrap();

        assert_eq!(report.reviewers, vec!["p:review-1".to_string()]);
        // No second start: the agent that was already there is adopted.
        assert!(starts(&herdr).is_empty(), "no second agent_start: {:?}", starts(&herdr));
        assert!(worktrees(&herdr).is_empty(), "the worktree was already made");
        // Nothing was typed into the live agent (F2).
        assert!(
            pane_runs(&herdr, pane).is_empty(),
            "the live agent gets no shell text: {:?}",
            pane_runs(&herdr, pane)
        );
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active);
        // … and it got its packet (the tick executes the delivery).
        let prompts = herdr.prompts_to(pane);
        assert_eq!(prompts.len(), 1, "{prompts:?}");
        assert!(prompts[0].starts_with("Review T-1 attempt 1."), "{}", prompts[0]);
        let in_flight = store.read(|tx| tx.slot_ops("p:review-1")).unwrap();
        assert_eq!(in_flight.len(), 1, "{in_flight:?}");
    }

    #[test]
    fn an_agent_the_engine_never_started_is_not_adopted() {
        // F5 of the T-18 review: an agent of the right kind that this engine
        // never started is not adopted — it may be another job's reviewer that
        // is still leaving. The pane is not typed into either.
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        let pane = "wt1:p1";
        herdr.add_shell(pane);
        // An agent of the reviewer's kind sits in the pane — but no start of
        // this engine is on record.
        herdr.add_agent(pane, "codex", gear_herdr::AgentStatus::Idle);
        store
            .write(|tx| {
                tx.insert_seat(&SeatRow {
                    id: "p:review-1".into(),
                    project: "p".into(),
                    role: SeatRole::Temp,
                    name: "review-1".into(),
                    profile: "codex".into(),
                    generation: 1,
                    state: SeatState::Starting,
                    state_since: T0,
                    state_reason: Some("onboarding".into()),
                    task_id: Some("T-1".into()),
                })?;
                tx.insert_binding(&BindingRow {
                    seat: "p:review-1".into(),
                    generation: 1,
                    session: "sess".into(),
                    pane_id: pane.into(),
                    kind: "codex".into(),
                    session_ref: None,
                    status: BindingStatus::Pending,
                    handover_id: None,
                    created: T0,
                })?;
                tx.set_meta(&path_key("p:review-1"), "/fake-worktrees/home-review-1")?;
                Ok(())
            })
            .unwrap();

        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();

        assert!(report.started.is_empty(), "{report:?}");
        assert!(starts(&herdr).is_empty(), "no reviewer started on a foreign agent");
        assert!(
            pane_runs(&herdr, pane).is_empty(),
            "the foreign agent gets no shell text: {:?}",
            pane_runs(&herdr, pane)
        );
        // The seat is parked for a human: the pane is not taken from the agent.
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Unknown);
    }

    #[test]
    fn two_jobs_on_one_seat_reuse_the_warm_worktree() {
        // T-18: a seat keeps its worktree and its build cache between jobs.
        // The second job takes the released seat, uses the same worktree (no
        // second worktree_open/create) and its agent is named after its task.
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        // Job 1: T-1's result comes in.
        let first = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        assert!(first.started.is_empty(), "the checkout waits for its report");
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        let first = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 15).unwrap();
        assert_eq!(first.started, vec!["p:review-1".to_string()]);
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.task_id.as_deref(), Some("T-1"));
        let worktrees_after_first = worktrees(&herdr);
        assert_eq!(
            worktrees_after_first,
            vec![("home/review-1".to_string(), "dev".to_string())]
        );
        // The job's agent is named after its task.
        let names: Vec<String> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::AgentStart { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["p-review-t1".to_string()]);

        // The job ends (T-17 releases it) and a second task is handed in.
        store
            .write(|tx| tx.set_seat_state("p:review-1", SeatState::Released, Some("review done"), T0 + 20))
            .unwrap();
        // The release closed the job's pane and ended its binding (L-64); the
        // worktree and cache stay.
        let _ = herdr.pane_close("wt1:p1");
        store
            .write(|tx| {
                let generation = tx.get_seat("p:review-1")?.unwrap().generation;
                tx.set_binding_status("p:review-1", generation, BindingStatus::Void)
            })
            .unwrap();
        store
            .write(|tx| {
                // The first review passed: its task is settled.
                let mut first = tx.get_task("p", "T-1")?.unwrap();
                first.state = TaskState::Done;
                tx.update_task(&first)?;
                tx.insert_task(&TaskRow {
                    project: "p".into(),
                    id: "T-2".into(),
                    rank: 2,
                    state: TaskState::Ready,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    updated: T0,
                })?;
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: "T-2".into(),
                    version: 1,
                    goal: "the second goal".into(),
                    accept: "the second accept".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-2", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-2", 1, T0)?;
                gear_app::tasks::submit_result(tx, "p", "T-2", 1, 1, "def5678", "ops/reports/T-2.md", T0)?;
                Ok(())
            })
            .unwrap();

        let second = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 30).unwrap();
        assert!(second.started.is_empty(), "the checkout waits for its report");
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 2, "def5678", "review/t2");
        let second = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 35).unwrap();

        assert_eq!(second.started, vec!["p:review-1".to_string()], "the warm seat took the job");
        // No second worktree: the seat's own is reopened by its path (its
        // checkout is on the last job's review branch, not on the seat's).
        assert_eq!(worktrees(&herdr), worktrees_after_first, "one worktree in total");
        assert!(
            herdr.calls.iter().any(|call| matches!(call, Call::WorktreeOpenPath { path, .. } if path.ends_with("home-review-1"))),
            "reopened by path: {:?}",
            herdr.calls
        );
        // The second job's agent is named after the second task.
        let names: Vec<String> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::AgentStart { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["p-review-t1".to_string(), "p-review-t2".to_string()]);
        // The checkout took the second job's branch at its candidate.
        let checkouts: Vec<String> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::PaneRun { command, .. } if command.contains("git checkout") => {
                    Some(command.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(checkouts.len(), 2, "{checkouts:?}");
        assert!(checkouts[1].contains("git checkout -f -B review/t2 def5678"), "{}", checkouts[1]);
        assert!(checkouts[1].contains("git clean -fd"), "the last job's leftovers go");
        assert!(!checkouts[1].contains("clean -fdx"), "the build cache stays");
        // A different task gets a new session (F3): the second reviewer starts
        // a fresh conversation.
        let second_args = &starts(&herdr)[1].1;
        assert!(
            !second_args.iter().any(|arg| arg == "resume"),
            "a new task is a new session: {second_args:?}"
        );
    }

    #[test]
    fn a_rereview_resumes_the_jobs_session() {
        // F3 of the T-18 review: a re-review of the same task resumes that
        // job's session — the reviewer keeps its conversation instead of
        // starting from nothing.
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        // Attempt 1 of T-1 is reviewed.
        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 15).unwrap();
        let first_args = starts(&herdr)[0].1.clone();
        assert!(!first_args.iter().any(|arg| arg == "resume"), "{first_args:?}");
        // Herdr reported the session of the reviewer.
        let session = store
            .read(|tx| tx.get_meta(&session_key("p", "T-1")))
            .unwrap()
            .expect("the reviewer's session");
        assert!(!session.is_empty(), "the session is kept for the re-review");

        // The review fails: the reviewer is released and its agent exits.
        store
            .write(|tx| {
                let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
                attempt.state = AttemptState::Rejected;
                attempt.updated = T0 + 20;
                tx.update_attempt(&attempt)?;
                let mut task = tx.get_task("p", "T-1")?.unwrap();
                task.state = TaskState::Ready;
                task.updated = T0 + 20;
                tx.update_task(&task)?;
                tx.set_seat_state("p:review-1", SeatState::Released, Some("review done"), T0 + 20)
            })
            .unwrap();
        herdr.exit_agent("wt1:p1");

        // Attempt 2 of the same task is handed in.
        store
            .write(|tx| {
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + 25)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 2, T0 + 25)?;
                gear_app::tasks::submit_result(tx, "p", "T-1", 2, 1, "abc1234", "ops/reports/T-1.md", T0 + 25)
            })
            .unwrap();

        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 30).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 2, "abc1234", "review/t1");
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 35).unwrap();

        assert_eq!(report.started, vec!["p:review-1".to_string()]);
        let second_args = &starts(&herdr)[1].1;
        assert!(
            second_args.iter().any(|arg| arg == &session),
            "the re-review resumes the session {session}: {second_args:?}"
        );
        assert!(second_args.iter().any(|arg| arg == "resume"), "{second_args:?}");
    }

    #[test]
    fn a_live_reviewer_without_its_packet_gets_it_on_the_next_tick() {
        // P1 #2 of the review: before activation and the delivery were one
        // transaction, a restart could leave an Active reviewer with no
        // packet — and an Active temp is never started again, so it would sit
        // in the slot forever. The next tick gives it the packet once.
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        let pane = "wt1:p1";
        herdr.add_agent(pane, "codex", gear_herdr::AgentStatus::Idle);
        store
            .write(|tx| {
                tx.insert_seat(&SeatRow {
                    id: "p:review-1".into(),
                    project: "p".into(),
                    role: SeatRole::Temp,
                    name: "review-1".into(),
                    profile: "codex".into(),
                    generation: 1,
                    state: SeatState::Active,
                    state_since: T0,
                    state_reason: Some("onboarded".into()),
                    task_id: Some("T-1".into()),
                })?;
                tx.insert_binding(&BindingRow {
                    seat: "p:review-1".into(),
                    generation: 1,
                    session: "sess".into(),
                    pane_id: pane.into(),
                    kind: "codex".into(),
                    session_ref: Some("ses_reviewer".into()),
                    status: BindingStatus::Active,
                    handover_id: None,
                    created: T0,
                })?;
                Ok(())
            })
            .unwrap();

        let report = crate::tick(&mut store, &mut herdr, &cfg(), T0 + 10).unwrap();
        assert_eq!(report.reviewers, vec!["p:review-1".to_string()]);
        // One packet reached the live reviewer, and it was not started again.
        assert_eq!(herdr.prompts_to(pane).len(), 1, "{:?}", herdr.prompts_to(pane));
        assert!(starts(&herdr).is_empty());

        // The next tick adds nothing: the packet is the guard.
        let report = crate::tick(&mut store, &mut herdr, &cfg(), T0 + 20).unwrap();
        assert!(report.reviewers.is_empty(), "{:?}", report.reviewers);
        assert_eq!(herdr.prompts_to(pane).len(), 1);
    }
}

/// How many "continue" nudges a reviewer gets before the captain is told.
const REVIEW_NUDGE_BUDGET: i64 = 3;

/// Reviewers that stopped before handing in their verdict are told to go on
/// (L-53). The signal is Herdr's own state — the agent is idle while its job
/// (a verdict for the attempt it reviews) is not done — whatever stopped it: a
/// server error ("model at capacity"), a dropped connection, a pause. Screen
/// text only rules out the cases a nudge cannot fix (quota, auth). After
/// [`REVIEW_NUDGE_BUDGET`] nudges the seat is returned as flagged, which opens
/// the captain's stall item.
///
/// Returns (seats nudged, seats flagged).
pub fn nudge_stopped_reviewers(
    store: &mut Store,
    project: &str,
    observed: &[crate::observe::SeatObs],
    cooldown_s: i64,
    now: Ts,
) -> GearResult<(Vec<String>, Vec<String>)> {
    let mut nudged = Vec::new();
    let mut flagged = Vec::new();
    for obs in observed {
        let seat = &obs.seat;
        if seat.role != SeatRole::Temp || seat.state != SeatState::Active || !obs.idle {
            continue;
        }
        if obs.flags.iter().any(|f| {
            matches!(f, gear_herdr::profile::ScreenFlag::Quota | gear_herdr::profile::ScreenFlag::Auth)
        }) {
            continue; // a nudge cannot fix a quota or a login
        }
        let Some(task) = seat.task_id.clone() else { continue };
        let outcome = store.write(|tx| {
            // The attempt under review and whether its verdict is in.
            let Some(attempt) = tx.attempts_of_task(project, &task)?.into_iter().last() else {
                return Ok(None);
            };
            if attempt.state != gear_model::AttemptState::Submitted
                || tx.get_review_verdict(project, &task, attempt.n)?.is_some()
            {
                return Ok(None);
            }
            // Never two deliveries in flight; wait out the cooldown.
            if !tx.slot_ops(&seat.id)?.is_empty() {
                return Ok(None);
            }
            let Some(last) = tx.last_op(&seat.id, OpKind::Deliver)? else {
                return Ok(None); // the packet itself is not sent yet
            };
            if now - last.created < cooldown_s {
                return Ok(None);
            }
            let used = last.payload.get("nudge").and_then(|n| n.as_i64()).unwrap_or(0);
            if used >= REVIEW_NUDGE_BUDGET {
                return Ok(Some(false));
            }
            tx.insert_op(&NewOp {
                project: project.to_string(),
                seat: seat.id.clone(),
                generation: seat.generation,
                kind: OpKind::Deliver,
                task: Some(task.clone()),
                attempt: Some(attempt.n),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({
                    "text": format!(
                        "You stopped before handing in your verdict on {task} (a server error such as \"model at capacity\", or a pause). Continue the review where you stopped, then hand in the verdict with the `gear send --kind review` command from your packet."
                    ),
                    "nudge": used + 1,
                }),
                created: now,
            })?;
            Ok(Some(true))
        })?;
        match outcome {
            Some(true) => nudged.push(seat.id.clone()),
            Some(false) => flagged.push(seat.id.clone()),
            None => {}
        }
    }
    Ok((nudged, flagged))
}
