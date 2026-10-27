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

use gear_app::config::{ModelEntry, ProjectConfig, Repo};
use gear_herdr::{HerdrAct, Outcome, Profile};
use gear_model::state::require_transition;
use gear_model::{
    AttemptState, ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, SeatRole, SeatState,
    TaskState, Ts,
};
use gear_store::{BindingStatus, NewMessage, NewOp, SeatRow, Store, Tx};

use crate::model::model_start;
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

    // A reviewer that the old activation/delivery pair could leave without its
    // packet gets it before anything else is started.
    report.started.extend(recover_missing_packets(store, project, &config, now)?);

    for task in tasks_to_review(store, project)? {
        // T-69 / B02-04: an attempt whose checkout already failed three times
        // is not tried again. A recovered seat (the park's slot freed) must
        // not restart the loop with a fresh generation and a fresh budget; a
        // new attempt of the task has a fresh budget and its own key.
        if checkout_stuck(store, project, &task.task, task.attempt)? {
            continue;
        }
        // 1. the model the reviewer runs on: the first entry of the review
        //    chain that differs from the writer's **current** model (T-35).
        //    Without a review chain the model is not managed.
        let writer = writer_current_model(store, &config, &task.writer)?;
        let entry = crate::model::review_entry(&config, writer.as_deref());
        if entry.is_none() && !config.models.review.is_empty() {
            report.waiting.push(task.task.clone());
            tell_captain(store, project, &task, "no review model differs from the writer's current model", now)?;
            continue;
        }
        // The CLI that runs it: the entry names it, else the temps kind.
        let kind = entry.and_then(ModelEntry::cli).unwrap_or(temps.kind.as_str()).to_string();
        let selected = if kind == temps.kind {
            profiles.get(&temps.profile).map(|profile| (temps.profile.as_str(), profile))
        } else {
            crate::restart::profile_for_kind(profiles, &kind)
        };
        let Some((profile_name, profile)) = selected else {
            continue;
        };
        // An unverified profile may not take over a real project (design/04).
        if !profile.verified {
            continue;
        }
        // 2. the reviewer seat: created once and bound to the task for life.
        let seat = match temp_seat_of(store, project, &task.task)? {
            Some(seat) => seat,
            // A seat that finished its last job is warm: the job takes it
            // rather than building a new worktree (17 §8a).
            None => match free_seat(store, project, &task.task, &kind)? {
                Some(seat) => take_seat(store, &seat, &task.task, profile_name, now)?,
                // No warm seat free: a new one only while the quota has room.
                // Otherwise the job waits for a seat instead of building a
                // cold extra worktree (finding 2 of the T-18 review).
                None => match seats_free_for_new(store, project, i64::from(config.quota.temps))? {
                    true => create_temp_seat(store, project, &task.task, profile_name, now)?,
                    false => {
                        report.waiting.push(task.task.clone());
                        tell_captain(store, project, &task, "every temp seat is in use", now)?;
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
        //    carries on; **every** other seat — a warm or recovered one
        //    included — needs a slot before it starts, and a queued one waits,
        //    and the captain is told once (design/10: QUEUED happens only when
        //    the quota is exhausted; the quota is the project config's
        //    `[quota].temps`). A `Released` seat used to skip this check, so
        //    several seats recovered by `release_dead_temps` started at once
        //    and passed the quota (F1 of the T-116 review).
        if !occupies_quota(seat.state)
            && !has_free_slot(store, herdr, project, i64::from(config.quota.temps))?
        {
            report.waiting.push(task.task.clone());
            tell_captain(store, project, &task, "every temp seat is in use", now)?;
            continue;
        }

        // The same task can choose another CLI after the writer falls back.
        store.write(|tx| tx.set_seat_profile(&seat.id, profile_name))?;
        // 4. the reviewer: worktree, environment, agent, review packet.
        match start_reviewer(
            store,
            herdr,
            cfg,
            &config,
            &seat,
            &task,
            entry,
            &kind,
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

/// The reviewers whose verdict is in go to `Standby` (design §3.6).
///
/// A reviewer that handed in its verdict keeps its agent and its pane: the
/// seat is idle, not gone, and it can take the next review. What ends is its
/// **job binding** (`seats.task_id = NULL`): the seat waits for work, it does
/// not work on a task any more.
///
/// A reviewer that handed in no verdict is not put on standby: a reviewer
/// whose job is gone (the task was cancelled, a newer attempt was handed in)
/// is released by `release.rs`.
///
/// The transition `Active → Standby` is not in the model's table yet (§3.6 is
/// new): the captain's own `- release <reviewer>` states its transition the
/// same way (`gear-app/src/plan.rs`).
pub fn standby_after_verdict(store: &mut Store, project: &str, now: Ts) -> GearResult<Vec<String>> {
    let mut standby = Vec::new();
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats {
        // Only a reviewer that is up: a queued or starting one has no agent
        // yet, and one on its way out is already leaving.
        if seat.role != SeatRole::Temp || seat.state != SeatState::Active {
            continue;
        }
        let Some(task) = seat.task_id.clone() else { continue };
        if !handed_a_verdict(store, project, &seat, &task)? {
            continue;
        }
        store.write(|tx| {
            // Active → Standby (design §3.6): the model knows the transition.
            require_transition(
                SeatState::can_transition(SeatRole::Temp, SeatState::Active, SeatState::Standby),
                "seat",
                SeatState::Active.as_str(),
                SeatState::Standby.as_str(),
            )?;
            tx.set_seat_state(&seat.id, SeatState::Standby, Some("review done"), now)?;
            // The job binding ends: the seat waits for the next review.
            tx.clear_seat_task(&seat.id)?;
            Ok(())
        })?;
        standby.push(seat.id.clone());
    }
    Ok(standby)
}

/// Whether this reviewer handed in the verdict of the job it works on.
///
/// The job is the attempt its own review packet named — the seat's last
/// `Deliver` op — and the verdict must be that attempt's: an earlier verdict
/// of the same task does not end the review that runs now. A rework is a new
/// attempt of the same task, and it is judged by its own verdict (F1 of the
/// T-143 review).
///
/// A seat with no packet (a legacy seat, a seat of an older engine) has no
/// job to name: the latest attempt of the task is its job then.
fn handed_a_verdict(store: &mut Store, project: &str, seat: &SeatRow, task: &str) -> GearResult<bool> {
    store.read(|tx| {
        let attempt = match tx.last_op(&seat.id, OpKind::Deliver)? {
            // The packet of the job: it must be about this seat's task.
            Some(op) => {
                if op.task.as_deref() != Some(task) {
                    return Ok(false);
                }
                op.attempt
            }
            // A seat with no packet: the latest attempt of the task is its job.
            None => tx
                .attempts_of_task(project, task)?
                .into_iter()
                .last()
                .map(|attempt| attempt.n),
        };
        let Some(attempt) = attempt else { return Ok(false) };
        Ok(tx
            .get_review_verdict(project, task, attempt)?
            .is_some_and(|verdict| verdict.by_seat == seat.id))
    })
}

/// An attempt a lane handed in that no reviewer is bound to.
struct ReviewTask {
    task: String,
    attempt: i64,
    /// The seat that handed the result in.
    writer: String,
    /// The lane that wrote the work: the task's repo is that lane's repo
    /// (T-53).
    lane: String,
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
                lane: task.lane.clone().unwrap_or_default(),
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
fn free_seat(store: &mut Store, project: &str, task: &str, kind: &str) -> GearResult<Option<SeatRow>> {
    store.read(|tx| {
        let seats = tx.seats_of_project(project)?;
        let available = |seat: &&SeatRow| {
            seat.role == SeatRole::Temp
                && matches!(seat.state, SeatState::Standby | SeatState::Released)
                && seat.name.starts_with("review-")
        };
        // V1: prefer the seat of this task's earlier review on the same CLI.
        for round in tx.rounds_of_task(project, task)?.into_iter().rev() {
            if round.role == "review" && round.cli == kind {
                if let Some(seat) = seats.iter().filter(available).find(|seat| seat.id == round.seat) {
                    return Ok(Some(seat.clone()));
                }
            }
        }
        // V2: a live standby seat before a released seat.
        Ok(seats.iter().filter(available).find(|seat| seat.state == SeatState::Standby)
            .or_else(|| seats.iter().filter(available).find(|seat| seat.state == SeatState::Released))
            .cloned())
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
/// cache; only the job it serves changes. The seat's profile follows the CLI
/// that runs the review's model (T-35).
fn take_seat(
    store: &mut Store,
    seat: &SeatRow,
    task: &str,
    profile: &str,
    now: Ts,
) -> GearResult<SeatRow> {
    store.write(|tx| {
        tx.set_seat_task(&seat.id, task)?;
        tx.set_seat_profile(&seat.id, profile)?;
        Ok(())
    })?;
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

/// Whether a temp slot is free: the seats that occupy the quota are the ones
/// with a live agent or starting — `Active`, `Standby`, `Starting` and
/// `Releasing` (design §3.6, the captain's decision of M-2603). A `Released`
/// seat holds no slot, and a parked (`Unknown`) one holds its slot only while
/// Herdr shows an agent in its pane (B02-04: a parked reviewer is a reviewer
/// a human has to look at).
fn has_free_slot(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    quota: i64,
) -> GearResult<bool> {
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    let mut occupying = 0;
    for seat in seats {
        if seat.role != SeatRole::Temp {
            continue;
        }
        if occupies_quota(seat.state) {
            occupying += 1;
            continue;
        }
        if seat.state == SeatState::Unknown && shows_an_agent(store, herdr, &seat)? {
            occupying += 1;
        }
    }
    Ok(occupying < quota)
}

/// Whether Herdr shows an agent in the seat's pane: a parked (`Unknown`)
/// seat holds a temp slot only while its agent is there (design §3.6).
///
/// A pane that cannot be read is not an absence: the seat keeps its slot
/// (design/08).
fn shows_an_agent(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &SeatRow,
) -> GearResult<bool> {
    let Some(binding) = store.read(|tx| tx.get_binding(&seat.id, seat.generation))? else {
        return Ok(false);
    };
    match herdr.agent_get(&binding.pane_id) {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(_) => Ok(true),
    }
}

/// The states in which a temp seat occupies the quota (design §3.6): a live
/// agent (`Active`, `Standby`), one that is starting, and one on its way out
/// (`Releasing` — its agent is still leaving).
fn occupies_quota(state: SeatState) -> bool {
    matches!(
        state,
        SeatState::Active | SeatState::Standby | SeatState::Starting | SeatState::Releasing
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
    entry: Option<&ModelEntry>,
    kind: &str,
    profile: &Profile,
    now: Ts,
) -> GearResult<Started> {
    // The repo the work is in: the repo of the lane that wrote it (T-53).
    // The reviewer's worktree, its branch and the diff the packet names all
    // belong to that repo.
    let repo = config.repo_of_lane(&task.lane);
    // The job's branch, and the seat's own anchor (its workspace label is
    // long-lived; the agent of a job is named after its task).
    let branch = review_branch(&task.task);
    let seat_branch = format!("home/{}", seat.name);
    let seat_label = worktree_label(config, &seat.name, &repo);
    let label = agent_name(config, &task.task);
    let model = entry.map(ModelEntry::model);

    // 1. the pane: the seat's, from its binding (an interrupted start or a new
    //    job on a warm seat), else the seat's worktree — opened, or created
    //    once at the integration branch.
    let binding = store.read(|tx| tx.get_binding(&seat.id, seat.generation))?;
    let live = binding.as_ref().filter(|binding| binding.status != BindingStatus::Void);
    let pane = match live.map(|binding| binding.pane_id.clone()) {
        Some(pane) => pane,
        None => {
            // A warm seat is reopened by its path: its checkout is on the last
            // job's review branch, not on the seat's own branch. A seat keeps
            // one checkout per repo (T-53): a seat whose last worktree is in
            // another repo gets one in this repo — the checkout of another
            // repo holds none of this repo's branches — and a seat that comes
            // back to a repo it used before finds its own checkout there.
            let by_path = match worktree_in(store, &seat.id, &repo, &config.root_repo().path)? {
                Some(path) => herdr.worktree_open_path(&repo.path, &path, &seat_label),
                None => Outcome::Refused { code: "worktree_not_found".into(), message: String::new() },
            };
            let opened = match by_path {
                Outcome::Refused { code, .. } if code == "worktree_not_found" => {
                    herdr.worktree_open(&repo.path, &seat_branch, &seat_label)
                }
                other => other,
            };
            let worktree = match opened {
                Outcome::Ok(worktree) => worktree,
                Outcome::Refused { code, .. } if code == "worktree_not_found" => {
                    match herdr.worktree_create(
                        &repo.path,
                        &seat_branch,
                        &repo.integration,
                        &seat_label,
                    ) {
                        Outcome::Ok(worktree) => worktree,
                        other => {
                            park(store, &seat.id, &step_failure("worktree_create", &other), now)?;
                            return Ok(Started::NotYet);
                        }
                    }
                }
                // The answer could not be read: a detached checkout (its HEAD
                // is kept, the branch let go of — T-69) answers without a
                // branch, and the worktree may well be open. The seat waits:
                // it is not parked, so it holds no temp slot, and the next
                // tick opens the worktree again.
                Outcome::Uncertain { .. } => return Ok(Started::NotYet),
                other => {
                    park(store, &seat.id, &step_failure("worktree_open", &other), now)?;
                    return Ok(Started::NotYet);
                }
            };
            // T-107: a reviewer's worktree left with GEAR's old pre-push hook
            // loses it before the agent starts.
            if let Err(reason) = crate::onboard::forget_gear_hooks(std::path::Path::new(&worktree.path)) {
                park(store, &seat.id, &format!("old hook: {reason}"), now)?;
                return Ok(Started::NotYet);
            }
            store.write(|tx| {
                // Archive the previous checkout before replacing the current
                // path. Older seats have only that path; a missing repo means root.
                if let Some(previous) = tx.get_meta(&path_key(&seat.id))? {
                    let previous_repo = tx.get_meta(&repo_key(&seat.id))?
                        .unwrap_or_else(|| config.root_repo().path);
                    tx.set_meta(&repo_path_key(&seat.id, &previous_repo), &previous)?;
                }
                tx.set_meta(&repo_path_key(&seat.id, &repo.path), &worktree.path)?;
                // Checkout submission still reads the current-path key.
                tx.set_meta(&path_key(&seat.id), &worktree.path)?;
                tx.set_meta(&repo_key(&seat.id), &repo.path)
            })?;
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
            store.write(|tx| gear_app::seats::begin_onboard(tx, &seat.id, &pane, kind, now))
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
            if !recorded_start(store, &seat.id, generation, &pane, kind)? {
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
            if let Some(start) = model.and_then(|model| model_start(kind, model)) {
                if !start.env.is_empty() {
                    env_command = format!("{env_command}; {}", start.env);
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
            // R1 of the T-69 review: the one worktree the engine may detach —
            // a known reviewer seat with no live agent whose recorded checkout
            // is this branch. Every other holder is left to fail the checkout.
            let detach = holder_to_detach(
                store,
                herdr,
                &config.project.id,
                &repo.path,
                &seat.id,
                &branch,
            )?;
            match checkout_ready(
                store,
                herdr,
                &seat.id,
                generation,
                &pane,
                &path,
                &branch,
                &task.candidate,
                detach.as_deref(),
                &repo.path,
            )? {
                Checked::Ready => {}
                // The checkout is pending: no reviewer starts on a tree nobody
                // checked out.
                Checked::Waiting => return Ok(Started::NotYet),
                Checked::Stuck(reason) => {
                    // It keeps failing: the seat is parked for a human and the
                    // captain gets one item, not a fight every tick (T-69).
                    // The attempt is marked, so a seat recovered from the park
                    // (B02-04) is not handed the same failing checkout again:
                    // the new generation's budget would restart the loop (F2
                    // of the T-116 review). A new attempt has a fresh budget.
                    park(store, &seat.id, &reason, now)?;
                    mark_checkout_stuck(store, &config.project.id, &task.task, task.attempt, &reason)?;
                    tell_captain(store, &config.project.id, task, &reason, now)?;
                    return Ok(Started::NotYet);
                }
            }

            // 6. the agent's args: the profile's unattended args, the model
            //    when the CLI takes one, the arguments of the chain entry and
            //    the session of an earlier review of the same task when the
            //    reviewer resumes it (F3).
            let args = reviewer_args(store, config, profile, kind, entry, &task.task)?;

            // The start is recorded before Herdr is asked: an engine restart
            // between the two must find the intent and adopt the agent it
            // asked for, and nothing else.
            record_start(store, &seat.id, generation, &pane, kind, &label)?;
            // A grok worktree is trusted before its agent starts: grok asks on
            // the first start, while Herdr reports it idle, and the prompt
            // would be lost.
            if let Err(reason) = crate::trust::trust_worktree(kind, &path, now) {
                // Nothing started, so the request goes: it must not authorize
                // the adoption of an agent that is not ours.
                store.write(|tx| tx.set_meta(&start_key(&seat.id, generation), ""))?;
                park(store, &seat.id, &format!("trust: {reason}"), now)?;
                return Ok(Started::NotYet);
            }
            // T-76: the reviewer runs in its own worktree; the args name it.
            let args = gear_herdr::render_args(&args, &path);
            match herdr.agent_start(&label, kind, &pane, &args) {
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

    // The chain's model before the reviewer gets any work (T-33, F5): OpenCode
    // ignores the start environment through its shared service, so the status
    // bar decides. Waiting keeps the start pending; the packet waits with it.
    if let Some(model) = model.filter(|_| kind == "opencode") {
        match crate::switch::ensure_model(store, herdr, seat, profile, config, model, now)? {
            crate::switch::ModelGate::Ready => {}
            crate::switch::ModelGate::Waiting => return Ok(Started::NotYet),
            crate::switch::ModelGate::Failed(reason) => {
                park(store, &seat.id, &reason, now)?;
                return Ok(Started::NotYet);
            }
        }
    }

    // 5. the seat is up and has its packet: one transaction. A crash between
    //    the two would leave a live reviewer that every later tick skips (an
    //    Active temp is not started again) and that never gets a job.
    let session_ref = started.agent_session.as_ref().map(|session| session.value.as_str());
    let packet = review_packet_for(store, config, &task.task, task.attempt, &task.candidate)?;
    store.write(|tx| {
        gear_app::seats::finish_onboard(tx, &seat.id, generation, session_ref, now)?;
        // The task's reviewer session is kept: a re-review of the same task
        // resumes it instead of starting a new conversation (F3).
        tx.set_meta(&session_cli_key(&config.project.id, &task.task), kind)?;
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
fn session_cli_key(project: &str, task: &str) -> String {
    format!("engine.review_session_cli.{project}.{task}")
}

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
fn checkout_command(
    path: &str,
    branch: &str,
    candidate: &str,
    token: &str,
    detach: Option<&str>,
) -> String {
    // T-69 (trial G10), R1 of the review: `git checkout -B` refuses a branch
    // that another worktree still holds. The engine names **one** holder — a
    // known reviewer seat with no live agent whose recorded checkout is this
    // branch — and this chain detaches it only while it is clean (its HEAD is
    // kept). Every other holder is left alone: the checkout then fails and the
    // engine stops after three tries and rings the captain.
    let holder = match detach {
        Some(holder) => format!(
            "if [ -z \"$(git -C '{holder}' status --porcelain 2>/dev/null)\" ]; then \
                 git -C '{holder}' checkout --detach >/dev/null 2>&1; \
             else echo {fail}${{T}}; exit 1; fi && ",
            holder = holder,
            fail = FAIL_MARKER,
        ),
        None => String::new(),
    };
    format!(
        "T={token}; cd {path} && git fetch origin && {holder}\
         git checkout -f -B {branch} {candidate} \
         && git clean -fd && echo {ok}${{T}}_$(git rev-parse HEAD)_$(git rev-parse --abbrev-ref HEAD) \
         || echo {fail}${{T}}",
        ok = OK_MARKER,
        fail = FAIL_MARKER,
    )
}

/// The worktree the engine may detach before a checkout (R1): a **known
/// reviewer seat** (its worktree and the branch it took are recorded), not
/// this seat, whose bound pane has **no live agent** in Herdr. Any other
/// holder of the branch is left to fail the checkout.
fn holder_to_detach(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    repo_path: &str,
    this_seat: &str,
    branch: &str,
) -> GearResult<Option<String>> {
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats {
        if seat.id == this_seat || seat.role != SeatRole::Temp {
            continue;
        }
        let (path, taken) = store.read(|tx| {
            Ok((
                tx.get_meta(&repo_path_key(&seat.id, repo_path))?,
                tx.get_meta(&branch_key(&seat.id, repo_path))?,
            ))
        })?;
        let (Some(path), Some(taken)) = (path, taken) else {
            continue;
        };
        if taken != branch {
            continue;
        }
        // The pane the reviewer used: a live binding, or the one its release
        // ended (a `Void` binding on a `Released` seat) — a released reviewer
        // still owns the checkout it left behind (R4 of the T-69 review). A
        // pending binding is a start that has not reached its agent yet, and
        // is never a holder this may detach.
        let pane = store.read(|tx| {
            Ok(tx
                .get_binding(&seat.id, seat.generation)?
                .and_then(|binding| match (binding.status, seat.state) {
                    (BindingStatus::Active, _) => Some(binding.pane_id),
                    (BindingStatus::Void, SeatState::Released) => Some(binding.pane_id),
                    _ => None,
                }))
        })?;
        let Some(pane) = pane else { continue };
        // Only a positive "no agent" counts (R1 of the second T-69 review): a
        // live agent is never touched, and a pane that cannot be observed —
        // an error, not an absence — is never detached either.
        match herdr.agent_get(&pane) {
            Ok(None) => {}
            Ok(Some(_)) | Err(_) => continue,
        }
        return Ok(Some(path));
    }
    Ok(None)
}

/// The branch a seat's recorded checkout took in a repo (R1 of the T-69
/// review): set with the checkout and kept after the seat is released — the
/// released reviewer still owns that checkout until its next job (R4).
fn branch_key(seat: &str, repo: &str) -> String {
    format!("engine.review_branch.{seat}.{repo}")
}

/// What the engine should do after reading the checkout's state.
enum Checked {
    /// The checkout is at the job's candidate: the reviewer may start.
    Ready,
    /// Not yet: the chain runs, or a fresh one was submitted.
    Waiting,
    /// It failed [`CHECKOUT_TRIES`] times: nothing starts, the captain is told.
    Stuck(String),
}

/// How often a checkout is submitted before the seat is parked and the captain
/// is rung (T-69).
const CHECKOUT_TRIES: i64 = 3;

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
    detach: Option<&str>,
    repo_path: &str,
) -> GearResult<Checked> {
    let key = checkout_key(seat, generation);
    let recorded = store
        .read(|tx| tx.get_meta(&key))?
        .filter(|token| !token.is_empty());
    let Some(token) = recorded else {
        // The checkout was never submitted: submit it and wait for its report.
        submit_checkout(store, herdr, seat, generation, pane, path, branch, candidate, detach, repo_path)?;
        return Ok(Checked::Waiting);
    };
    // The chain ran on an earlier tick: read what it did.
    let tail = herdr.read_tail(pane, CHECKOUT_TAIL_LINES).unwrap_or_default();
    match observe_checkout(&tail, &token) {
        Checkout::Finished { sha, branch: got } => {
            if same_commit(&sha, candidate) && got == branch {
                Ok(Checked::Ready)
            } else {
                // It landed somewhere else: check out again.
                submit_checkout(store, herdr, seat, generation, pane, path, branch, candidate, detach, repo_path)?;
                Ok(Checked::Waiting)
            }
        }
        Checkout::Failed => {
            // The fetch or the checkout failed. T-69: a branch that another
            // worktree holds with changes of its own fails every time; the
            // engine stops after a few tries and tells the captain instead of
            // fighting it tick after tick.
            let used = bump_checkout_fails(store, seat, generation)?;
            if used >= CHECKOUT_TRIES {
                let detail = tail
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                return Ok(Checked::Stuck(format!(
                    "the checkout of {branch} at {candidate} failed {used} times (the worktree {path}): {detail}"
                )));
            }
            submit_checkout(store, herdr, seat, generation, pane, path, branch, candidate, detach, repo_path)?;
            Ok(Checked::Waiting)
        }
        Checkout::Pending => Ok(Checked::Waiting),
    }
}

/// Count one failed checkout for this seat generation.
fn bump_checkout_fails(store: &mut Store, seat: &str, generation: i64) -> GearResult<i64> {
    store.write(|tx| {
        let key = format!("engine.review_checkout_fails.{seat}.{generation}");
        let used = tx.get_meta(&key)?.and_then(|v| v.parse::<i64>().ok()).unwrap_or(0) + 1;
        tx.set_meta(&key, &used.to_string())?;
        Ok(used)
    })
}

/// The key of an attempt whose checkout already failed three times.
fn checkout_stuck_key(project: &str, task: &str, attempt: i64) -> String {
    format!("engine.review_checkout_stuck.{project}.{task}.{attempt}")
}

/// Remember that this attempt's checkout is stuck: the seat is parked for a
/// human and the attempt is not handed to another seat (B02-04: a recovered
/// seat would restart the three-try loop with a fresh budget). A new attempt
/// of the task has its own key.
fn mark_checkout_stuck(
    store: &mut Store,
    project: &str,
    task: &str,
    attempt: i64,
    reason: &str,
) -> GearResult<()> {
    store.write(|tx| tx.set_meta(&checkout_stuck_key(project, task, attempt), reason))
}

/// Whether this attempt's checkout is already stuck (three failed tries): no
/// reviewer is started for it until a new attempt is handed in.
fn checkout_stuck(store: &mut Store, project: &str, task: &str, attempt: i64) -> GearResult<bool> {
    Ok(store
        .read(|tx| tx.get_meta(&checkout_stuck_key(project, task, attempt)))?
        .is_some_and(|reason| !reason.is_empty()))
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
    detach: Option<&str>,
    repo_path: &str,
) -> GearResult<()> {
    let token = next_checkout_token(store)?;
    let command = checkout_command(path, branch, candidate, &token, detach);
    store.write(|tx| {
        tx.set_meta(&checkout_key(seat, generation), &token)?;
        // The seat's recorded checkout: a later re-review on another seat reads
        // it to find the holder of this branch (R1 of the T-69 review).
        tx.set_meta(&branch_key(seat, repo_path), branch)
    })?;
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
/// when the CLI takes one, the arguments of the chain entry (T-35) and — when
/// the task was reviewed before — the profile's resume args with that
/// session, so a re-review resumes the job's session instead of starting a
/// new conversation (F3 of the T-18 review).
fn reviewer_args(
    store: &mut Store,
    config: &ProjectConfig,
    profile: &Profile,
    kind: &str,
    entry: Option<&ModelEntry>,
    task: &str,
) -> GearResult<Vec<String>> {
    let mut args = profile.unattended_args.clone();
    if let Some(start) = entry.and_then(|entry| crate::model::entry_args(kind, entry)) {
        args.extend(start);
    }
    let session = store
        .read(|tx| tx.get_meta(&session_key(&config.project.id, task)))?
        .filter(|session| !session.is_empty());
    let session_cli = store.read(|tx| tx.get_meta(&session_cli_key(&config.project.id, task)))?;
    if let Some(session) = session.filter(|_| session_cli.as_deref() == Some(kind)) {
        if let Some(resume) = &profile.resume_args {
            args.extend(resume.iter().map(|arg| arg.replace("{session}", &session)));
        }
    }
    Ok(args)
}

/// The model the writer of an attempt runs on **now**: the one the engine
/// recorded for that seat (a fallback changed it), else the writer's primary
/// (T-35). The reviewer never runs the writer's current model.
fn writer_current_model(
    store: &mut Store,
    config: &ProjectConfig,
    writer: &str,
) -> GearResult<Option<String>> {
    let recorded = store
        .read(|tx| tx.get_meta(&crate::model::model_meta_key(writer)))?
        .filter(|model| !model.is_empty());
    Ok(recorded.or_else(|| crate::model::writer_model(config, writer)))
}

/// The review job a reviewer seat holds: the task it is bound to, the attempt
/// under review and the full packet (T-115). `None` when the seat holds no
/// job, the attempt carries no candidate, or the verdict is already in: a
/// restarted reviewer must not be sent work that no longer waits for it.
pub(crate) struct ReviewJob {
    pub task: String,
    pub attempt: i64,
    pub packet: String,
}

/// The job of a reviewer seat, with its packet — what a reviewer is given
/// again when its model changed under it (T-115).
pub(crate) fn job_for_seat(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
) -> GearResult<Option<ReviewJob>> {
    let Some(task) = seat.task_id.clone() else {
        return Ok(None);
    };
    let Some(row) = store.read(|tx| tx.get_project(project))? else {
        return Ok(None);
    };
    let Ok(config) = gear_app::config::parse(&row.config_toml) else {
        return Ok(None);
    };
    let Some(attempt) = store
        .read(|tx| tx.attempts_of_task(project, &task))?
        .into_iter()
        .last()
    else {
        return Ok(None);
    };
    if store.read(|tx| tx.get_review_verdict(project, &task, attempt.n))?.is_some() {
        return Ok(None);
    }
    let Some(candidate) = attempt.candidate.clone() else {
        return Ok(None);
    };
    let packet = review_packet_for(store, &config, &task, attempt.n, &candidate)?;
    Ok(Some(ReviewJob { task, attempt: attempt.n, packet }))
}

/// The model a reviewer seat runs on now (T-115): the one a fallback recorded,
/// else the entry it was started on — the first review model that differs from
/// its writer's current model (the rule `start_reviews` uses).
pub(crate) fn seat_current_model(
    store: &mut Store,
    project: &str,
    config: &ProjectConfig,
    seat: &SeatRow,
) -> GearResult<Option<String>> {
    let recorded = store
        .read(|tx| tx.get_meta(&crate::model::model_meta_key(&seat.id)))?
        .filter(|model| !model.is_empty());
    if recorded.is_some() {
        return Ok(recorded);
    }
    let Some(task) = seat.task_id.clone() else {
        return Ok(None);
    };
    let writer = store
        .read(|tx| tx.attempts_of_task(project, &task))?
        .into_iter()
        .last()
        .map(|attempt| attempt.seat);
    let writer_model = match writer {
        Some(writer) => writer_current_model(store, config, &writer)?,
        None => None,
    };
    Ok(crate::model::review_entry(config, writer_model.as_deref())
        .map(|entry| entry.model().to_string()))
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
    // The version the hand-in was made against: the attempt's adopted contract
    // (a hand-in must name it, T-8), never an older one the attempt once had
    // (T-63: T-58's reviewer was sent v1 after the lane reworked to v2).
    let (version, goal, accept, done, bar, lane) = store.read(|tx| {
        let version = tx
            .get_attempt(&config.project.id, task, attempt)?
            .map(|row| row.adopted_version)
            .unwrap_or(1);
        let (goal, accept) = match tx.get_contract(&config.project.id, task, version)? {
            Some(contract) => (contract.goal, contract.accept),
            None => (String::new(), String::new()),
        };
        // The voyage the task sails on (T-22): the sentence the phase is
        // judged by and its quality bar (T-23). The lane is the task's lane:
        // the repo the work is in (T-53).
        let (done, bar, lane) = match tx.get_task(&config.project.id, task)? {
            Some(row) => {
                let lane = row.lane.clone().unwrap_or_default();
                let (done, bar) = match row
                    .milestone
                    .as_deref()
                    .map(|milestone| tx.get_voyage(&config.project.id, milestone))
                {
                    Some(Ok(Some(voyage))) => (
                        voyage.done.clone().unwrap_or_default(),
                        voyage.bar.clone().unwrap_or_default(),
                    ),
                    _ => (String::new(), String::new()),
                };
                (done, bar, lane)
            }
            None => (String::new(), String::new(), String::new()),
        };
        Ok((version, goal, accept, done, bar, lane))
    })?;
    // The diff is against the integration branch of the repo the work is in
    // (T-53): a child repo has its own branch.
    let repo = config.repo_of_lane(&lane);
    let packet = review_packet(
        task,
        attempt,
        version,
        &goal,
        &accept,
        &done,
        &bar,
        candidate,
        &repo.integration,
        &review_branch(task),
    );
    // The review's own budget (design §5.1) and what a verdict past it must
    // carry (design §6.2): the reviewer is charged from its own start, and a
    // verdict past its budget needs the friction report.
    let budget = store.read(|tx| {
        let model = tx
            .rounds_of_task(&config.project.id, task)?
            .iter()
            .find(|round| round.attempt == attempt && round.role == "review")
            .and_then(|round| round.model.clone());
        tx.round_budget_min(&config.project.id, task, attempt, "review", model.as_deref())
    })?;
    let packet = format!(
        "{packet}\nBudget: {budget} min. Past it, your hand-in must carry --friction \"…\" \
         (a permission you lacked and worked around, a test or build that took far too long, \
         an unclear contract, a missing tool or fixture, a step that cost much time)."
    );
    // T-84: the reviewer judges against the project standard when it has one.
    Ok(match gear_app::standard::line(&config.project.repo) {
        Some(line) => format!("{packet}\n{line}"),
        None => packet,
    })
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

/// The repo path of the seat's current checkout (the existing metadata format).
fn repo_key(seat: &str) -> String {
    format!("engine.review_repo.{seat}")
}

/// The seat's current worktree, used to submit its checkout command.
fn worktree_path(store: &mut Store, seat: &str) -> GearResult<Option<String>> {
    store.read(|tx| tx.get_meta(&path_key(seat)))
}

/// A seat keeps a checkout in each repo it has reviewed.
fn repo_path_key(seat: &str, repo: &str) -> String {
    format!("engine.review_path.{seat}.{repo}")
}

/// Reopen a repo's saved checkout even after the seat worked in another repo.
/// Fall back to the current-path metadata for seats created before this change.
fn worktree_in(
    store: &mut Store,
    seat: &str,
    repo: &Repo,
    root: &str,
) -> GearResult<Option<String>> {
    if let Some(path) = store.read(|tx| tx.get_meta(&repo_path_key(seat, &repo.path)))? {
        return Ok(Some(path));
    }
    let current = store.read(|tx| tx.get_meta(&repo_key(seat)))?;
    if current.as_deref().unwrap_or(root) == repo.path {
        worktree_path(store, seat)
    } else {
        Ok(None)
    }
}

/// The label of a review seat's worktree: `<project>-<seat>`, plus the repo
/// name for a seat that works in a child repo (T-53) — one seat, one
/// worktree per repo.
fn worktree_label(config: &ProjectConfig, seat: &str, repo: &Repo) -> String {
    if repo.name.is_empty() {
        format!("{}-{}", config.project.id, seat)
    } else {
        format!("{}-{}-{}", config.project.id, seat, repo.name)
    }
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
    // One rule with `names::desired` (T-134 review F1): the reviewer's agent
    // carries the task it reviews, so a name Herdr cleared is restored to the
    // same name the review started with.
    crate::names::review_agent_name(&config.project.id, task)
}

/// The packet the reviewer gets: what to review, where the rules are and the
/// exact command to hand the verdict in.
fn review_packet(
    task: &str,
    attempt: i64,
    version: i64,
    goal: &str,
    accept: &str,
    done: &str,
    bar: &str,
    candidate: &str,
    integration: &str,
    branch: &str,
) -> String {
    format!(
        "Review {task} attempt {attempt}. Run `gear` first (it records that you got this).\n\
         \n\
         Contract: v{version} (the version this hand-in was made against)\n\
         Goal: {goal}\n\
         Accept: {accept}\n\
         Done (the milestone's sentence): {done}\n\
         Bar (the quality this phase needs): {bar}\n\
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
         Every finding is either blocking or logged: a blocking finding names the part of the \
         done-sentence or the bar it breaks. Fail only for a failed mechanical fact, a P0, or a \
         blocking finding; everything else is logged and does not block.\n\
         \n\
         Hand your verdict in with exactly (the findings as text, one per line, each starting \
         with its id; `--blocking` names the ones that block):\n\
         gear send --kind review --task {task} --attempt {attempt} --verdict <pass|fail> \
         [--blocking <ids>] \"<the path to your review file, or the text>\"\n\
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
fn tell_captain(store: &mut Store, project: &str, task: &ReviewTask, reason: &str, now: Ts) -> GearResult<()> {
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
            meta: None,
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
                "The review of {} (attempt {}) waits: {reason}, so the \
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
    use gear_store::{testing, AttemptRow, BindingRow, BindingStatus, ContractRow, ProjectRow, TaskRow};

    const T0: i64 = 1_800_000_000;

    /// The review chain of the test config: the **entry form** (T-35) — the
    /// CLI and the reasoning effort are the entry's.
    const REVIEW_CHAIN: &str =
        r#"review = [{ model = "gpt-6.1-sol", cli = "codex", args = ["-c", "model_reasoning_effort=high"] }]"#;
    /// The same chain in the old plain form: the CLI is the temps kind.
    const REVIEW_CHAIN_PLAIN: &str = r#"review = ["gpt-6.1-sol"]"#;

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
review = [{ model = "gpt-6.1-sol", cli = "codex", args = ["-c", "model_reasoning_effort=high"] }]
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
review = [{ model = "gpt-6.1-sol", cli = "codex", args = ["-c", "model_reasoning_effort=high"] }]
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
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
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
                    estimate_min: None,
                     issue: None,updated: T0,
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

    /// T-63: the reviewer is sent the contract the hand-in was made against —
    /// the newest version at hand-in time, not the attempt's first one (M2.1:
    /// T-58's reviewer got v1 after the lane reworked to v2 and asked the
    /// captain which governs).
    #[test]
    fn the_review_packet_follows_the_hand_ins_contract_version() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&ProjectRow {
                    id: "p".into(),
                    herdr_session: "sess".into(),
                    config_toml: CONFIG.into(),
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
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
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
                    estimate_min: None,
                    issue: None,
                    updated: T0,
                })?;
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    version: 1,
                    goal: "the first goal".into(),
                    accept: "the first criteria".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)
            })
            .unwrap();
        // The captain rewrites the contract while the lane works on it.
        store
            .write(|tx| {
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    version: 2,
                    goal: "the revised goal".into(),
                    accept: "the revised criteria".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0 + 5,
                })?;
                let mut task = tx.get_task("p", "T-1")?.unwrap();
                task.contract_version = 2;
                tx.update_task(&task)?;
                let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
                attempt.pending_update = Some(2);
                tx.update_attempt(&attempt)
            })
            .unwrap();
        // The lane's page showed v2, so it works to v2 and hands in under it.
        store
            .write(|tx| gear_app::tasks::adopt_contract(tx, "p", "T-1", 1, 2, T0 + 10).map(|_| ()))
            .unwrap();
        store
            .write(|tx| {
                gear_app::tasks::submit_result(
                    tx,
                    "p",
                    "T-1",
                    1,
                    2,
                    "abc1234",
                    "ops/reports/T-1.md",
                    T0 + 20,
                )
            })
            .unwrap();

        let config = gear_app::config::parse(CONFIG).unwrap();
        let packet = review_packet_for(&mut store, &config, "T-1", 1, "abc1234").unwrap();

        assert!(
            packet.contains("Contract: v2 (the version this hand-in was made against)"),
            "{packet}"
        );
        assert!(packet.contains("Goal: the revised goal"), "{packet}");
        assert!(packet.contains("Accept: the revised criteria"), "{packet}");
        assert!(!packet.contains("the first goal"), "not the old contract: {packet}");
    }

    /// T-168 (design §6.2): the reviewer's packet says the review's own
    /// budget and what a verdict past it must carry.
    #[test]
    fn the_reviewer_packet_carries_the_budget_and_the_friction_flag() {
        let mut store = world(CONFIG);
        let config = gear_app::config::parse(CONFIG).unwrap();
        let packet = review_packet_for(&mut store, &config, "T-1", 1, "abc1234").unwrap();

        assert!(
            packet.contains("Budget: 30 min. Past it, your hand-in must carry --friction"),
            "{packet}"
        );
        // The words that belong in a report (design §6.1).
        assert!(packet.contains("a permission you lacked"), "{packet}");
    }

    /// The command's output, whether or not it succeeded (the checkout's
    /// failure path exits non-zero on purpose).
    fn sh_out(dir: &std::path::Path, command: &str) -> String {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir)
            .output()
            .unwrap();
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    /// Move `work`'s HEAD onto `branch` in its origin, from the origin's own
    /// side (T-107): the engine's tests never run `git push`.
    fn publish(work: &std::path::Path, branch: &str) {
        sh(
            work,
            &format!(
                "origin=$(git remote get-url origin); \
                 git -C \"$origin\" fetch -q \"$(pwd)\" +HEAD:refs/heads/{branch} && \
                 git fetch -q origin"
            ),
        );
    }

    fn sh(dir: &std::path::Path, command: &str) -> String {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{command}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// R1 of the T-69 review: the engine names **one** holder to detach — a
    /// known reviewer seat, not this one, whose recorded checkout is the
    /// branch and whose pane has no live agent. A live reviewer and an
    /// unowned holder are never named.
    #[test]
    fn only_an_idle_clean_reviewer_is_named_as_the_holder() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "review-1", SeatRole::Temp, "w9:p1", "codex", T0)?;
                testing::seed_active_seat(tx, "p", "review-2", SeatRole::Temp, "w9:p2", "codex", T0)?;
                tx.set_meta(&repo_path_key("p:review-1", "/repo"), "/repo-old-1")?;
                tx.set_meta(&branch_key("p:review-1", "/repo"), "review/t1")?;
                tx.set_meta(&repo_path_key("p:review-2", "/repo"), "/repo-old-2")?;
                tx.set_meta(&branch_key("p:review-2", "/repo"), "review/t1")?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        // review-2 has a live agent; review-1's pane has none.
        herdr.add_agent("w9:p2", "codex", gear_herdr::AgentStatus::Idle);

        let holder =
            holder_to_detach(&mut store, &mut herdr, "p", "/repo", "p:review-3", "review/t1").unwrap();
        assert_eq!(holder.as_deref(), Some("/repo-old-1"), "the idle reviewer");

        // That reviewer is on another branch now: the live one is not named.
        store.write(|tx| tx.set_meta(&branch_key("p:review-1", "/repo"), "review/t2")).unwrap();
        let holder =
            holder_to_detach(&mut store, &mut herdr, "p", "/repo", "p:review-3", "review/t1").unwrap();
        assert_eq!(holder, None, "a live reviewer is never detached");

        // Nobody recorded for the branch: an unowned holder is left alone.
        store.write(|tx| tx.set_meta(&branch_key("p:review-2", "/repo"), "review/t3")).unwrap();
        let holder =
            holder_to_detach(&mut store, &mut herdr, "p", "/repo", "p:review-3", "review/t1").unwrap();
        assert_eq!(holder, None, "an unowned holder is left alone");

        // A released reviewer still owns the checkout it left behind (R4 of
        // the second review): its binding ended at release (Void) on a
        // Released seat, and Herdr positively shows no agent.
        store
            .write(|tx| {
                tx.set_meta(&branch_key("p:review-1", "/repo"), "review/t1")?;
                tx.set_seat_state("p:review-1", SeatState::Released, Some("review done"), T0 + 30)?;
                tx.set_binding_status("p:review-1", 1, BindingStatus::Void)
            })
            .unwrap();
        let holder =
            holder_to_detach(&mut store, &mut herdr, "p", "/repo", "p:review-3", "review/t1").unwrap();
        assert_eq!(holder.as_deref(), Some("/repo-old-1"), "the released reviewer");

        // An observation error is not an absence (R1 of the second review):
        // a pane that cannot be looked at is never detached.
        herdr.observe_fails = true;
        let holder =
            holder_to_detach(&mut store, &mut herdr, "p", "/repo", "p:review-3", "review/t1").unwrap();
        assert_eq!(holder, None, "an unobservable pane is never detached");
    }

    /// T-69 (trial G10): the re-review of a task on another seat. A worktree
    /// that holds `review/t1` and is clean is detached (its HEAD kept) and the
    /// checkout goes on; one with uncommitted changes is left alone and the
    /// checkout reports a failure, so the engine stops after a few tries.
    #[test]
    fn a_clean_holder_of_the_review_branch_is_detached_a_dirty_one_is_not() {
        let root = std::env::temp_dir().join(format!(
            "gear-review-detach-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        sh(&root, "git init -q --bare origin.git");
        sh(&root, "git init -q -b dev work");
        let work = root.join("work");
        std::fs::write(work.join("README.md"), "dev\n").unwrap();
        sh(&work, "git add -A && git -c user.email=a@b -c user.name=a commit -qm dev");
        sh(&work, "git remote add origin ../origin.git");
        publish(&work, "dev");
        sh(&work, "git switch -qc review/t1 && echo one > one.txt && git add -A \
                   && git -c user.email=a@b -c user.name=a commit -qm one");
        publish(&work, "review/t1");
        let candidate = sh(&work, "git rev-parse review/t1");
        sh(&work, "git switch -q dev");
        // The previous reviewer's worktree holds the branch; the new seat's is
        // free.
        let old = root.join("old-seat");
        let new = root.join("new-seat");
        sh(&work, &format!("git worktree add -q {} review/t1", old.display()));
        sh(&work, &format!("git worktree add -q -b home/seat {} origin/dev", new.display()));

        // A clean holder: detached, its HEAD kept, and the checkout succeeds.
        let token = "t1";
        let old_path = old.display().to_string();
        let report = sh(&new, &checkout_command(&new.display().to_string(), "review/t1", &candidate, token, Some(&old_path)));
        assert!(report.contains(&format!("{OK_MARKER}{token}_")), "{report}");
        assert_eq!(sh(&old, "git rev-parse HEAD"), candidate, "its HEAD is kept");
        assert_eq!(sh(&old, "git rev-parse --abbrev-ref HEAD"), "HEAD", "detached");
        assert_eq!(sh(&new, "git rev-parse --abbrev-ref HEAD"), "review/t1");
        assert_eq!(sh(&new, "git rev-parse HEAD"), candidate);

        // A dirty holder: nothing is detached and the checkout reports it.
        sh(&work, &format!("git -C {} checkout -q --detach", new.display()));
        sh(&work, &format!("git -C {} checkout -q review/t1", old.display()));
        std::fs::write(old.join("uncommitted.txt"), "mine\n").unwrap();
        sh(&work, &format!("git worktree add -q -b home/seat2 {} origin/dev", root.join("new-seat-2").display()));
        let second = root.join("new-seat-2");
        let report = sh_out(&second, &checkout_command(&second.display().to_string(), "review/t1", &candidate, "t2", Some(&old_path)));
        assert!(report.contains(&format!("{FAIL_MARKER}t2")), "{report}");
        assert_eq!(sh(&old, "git rev-parse --abbrev-ref HEAD"), "review/t1", "not detached");
        assert!(old.join("uncommitted.txt").exists(), "its changes are kept");

        // An unowned holder (no path named by the engine) is left alone too:
        // the checkout fails rather than detaching what it does not know.
        sh(&work, &format!("git -C {} checkout -q review/t1", old.display()));
        let third = root.join("new-seat-3");
        sh(&work, &format!("git worktree add -q -b home/seat3 {} origin/dev", third.display()));
        let report = sh_out(&third, &checkout_command(&third.display().to_string(), "review/t1", &candidate, "t3", None));
        assert!(report.contains(&format!("{FAIL_MARKER}t3")), "{report}");
        assert_eq!(sh(&old, "git rev-parse --abbrev-ref HEAD"), "review/t1", "not detached");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A scratch repo with `origin`, `dev` and `review/t1` at a candidate,
    /// the previous reviewer's worktree holding that branch, and a free seat's
    /// worktree on its own branch. Returns (root, old, new, candidate).
    fn scratch_review_repo(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf, String) {
        let root = std::env::temp_dir().join(format!(
            "gear-review-release-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        sh(&root, "git init -q --bare origin.git");
        sh(&root, "git init -q -b dev work");
        let work = root.join("work");
        std::fs::write(work.join("README.md"), "dev\n").unwrap();
        sh(&work, "git add -A && git -c user.email=a@b -c user.name=a commit -qm dev");
        sh(&work, "git remote add origin ../origin.git");
        publish(&work, "dev");
        sh(&work, "git switch -qc review/t1 && echo one > one.txt && git add -A \
                   && git -c user.email=a@b -c user.name=a commit -qm one");
        publish(&work, "review/t1");
        let candidate = sh(&work, "git rev-parse review/t1");
        sh(&work, "git switch -q dev");
        let old = root.join("old-seat");
        let new = root.join("new-seat");
        sh(&work, &format!("git worktree add -q {} review/t1", old.display()));
        sh(&work, &format!("git worktree add -q -b home/review-1 {} origin/dev", new.display()));
        (root, old, new, candidate)
    }

    /// The engine's world after a reviewer was *released*: `review-9` finished
    /// its job through the real release lifecycle and still holds `review/t1`;
    /// `review-1` is the free seat the next review takes, with a real worktree
    /// Herdr has open. The task's candidate is the scratch repo's real one.
    fn world_with_a_released_holder(
        old: &std::path::Path,
        new: &std::path::Path,
        candidate: &str,
        herdr: &mut FakeHerdr,
    ) -> Store {
        let mut store = world(CONFIG);
        let old_path = old.display().to_string();
        let new_path = new.display().to_string();
        store
            .write(|tx| {
                // The previous reviewer: Active on its job, holding the branch.
                testing::seed_temp_seat(tx, "p", "review-9", "T-0", "w9:p9", "codex", T0)?;
                tx.set_meta(&repo_path_key("p:review-9", "/repo"), &old_path)?;
                tx.set_meta(&branch_key("p:review-9", "/repo"), "review/t1")?;
                // The free seat the next review takes: warm, with its worktree.
                let seat =
                    testing::seed_active_seat(tx, "p", "review-1", SeatRole::Temp, "w3:p3", "codex", T0)?;
                tx.set_seat_state(&seat.id, SeatState::Released, Some("last job done"), T0)?;
                tx.set_binding_status(&seat.id, 1, BindingStatus::Void)?;
                tx.set_meta(&repo_path_key("p:review-1", "/repo"), &new_path)?;
                tx.set_meta(&path_key("p:review-1"), &new_path)?;
                // The review must check out the commit the scratch repo holds.
                let attempt = tx.get_attempt("p", "T-1", 1)?.expect("the attempt");
                tx.update_attempt(&AttemptRow { candidate: Some(candidate.to_string()), ..attempt })
            })
            .unwrap();
        herdr.add_shell("w3:p3");
        herdr.add_worktree("home/review-1", &new_path, "w3:p3");
        store
    }

    /// T-69 (trial G10), R4 of the second review: through the real release
    /// lifecycle, a released reviewer's **clean** worktree still holding the
    /// job's branch is the one holder the engine names. The review goes to a
    /// different seat, the checkout (run for real) detaches the holder with
    /// its HEAD kept, lands on the candidate, and the reviewer starts.
    #[test]
    fn a_released_reviewers_clean_worktree_is_detached_and_the_review_starts() {
        let (root, old, new, candidate) = scratch_review_repo("clean");
        let mut herdr = FakeHerdr::new();
        let mut store = world_with_a_released_holder(&old, &new, &candidate, &mut herdr);

        // The release lifecycle, not a hand-set state: the pane closes, then
        // the agent is positively gone and the binding ends.
        crate::release::release_finished_reviews(&mut store, &mut herdr, "p", T0 + 5).unwrap();
        crate::release::release_finished_reviews(&mut store, &mut herdr, "p", T0 + 6).unwrap();
        let holder = store.read(|tx| tx.get_seat("p:review-9")).unwrap().unwrap();
        assert_eq!(holder.state, SeatState::Released, "the previous reviewer is released");
        let binding = store.read(|tx| tx.get_binding("p:review-9", 1)).unwrap().unwrap();
        assert_eq!(binding.status, BindingStatus::Void, "its binding ended at release");

        // The next review of the task takes the other seat; the released
        // reviewer is left where it was.
        let first = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        assert!(first.started.is_empty(), "{first:?}");
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.task_id.as_deref(), Some("T-1"), "a different seat takes the job");
        let holder = store.read(|tx| tx.get_seat("p:review-9")).unwrap().unwrap();
        assert_eq!(holder.state, SeatState::Released, "the released reviewer is not reloaded");

        // The checkout the engine submitted names the released worktree.
        let command = herdr
            .calls
            .iter()
            .rev()
            .find_map(|call| match call {
                Call::PaneRun { command, .. } if command.contains("git checkout -f -B") => {
                    Some(command.clone())
                }
                _ => None,
            })
            .expect("the checkout");
        assert!(
            command.contains(&format!("git -C '{}' status --porcelain", old.display())),
            "{command}"
        );

        // Run it for real: the holder is clean, so it is detached (HEAD kept)
        // and the new seat lands on the candidate.
        let report = sh(&new, &command);
        assert!(report.contains(OK_MARKER), "{report}");
        assert_eq!(sh(&old, "git rev-parse --abbrev-ref HEAD"), "HEAD", "detached");
        assert_eq!(sh(&old, "git rev-parse HEAD"), candidate, "its HEAD is kept");
        assert_eq!(sh(&new, "git rev-parse --abbrev-ref HEAD"), "review/t1");
        assert_eq!(sh(&new, "git rev-parse HEAD"), candidate);

        // The engine sees the checkout and starts the reviewer.
        let generation = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap().generation;
        confirm_checkout(&mut store, &mut herdr, "p:review-1", generation, &candidate, "review/t1");
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();
        assert_eq!(report.started, vec!["p:review-1".to_string()]);
        assert_eq!(starts(&herdr).len(), 1, "the reviewer starts");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// T-69 (trial G10), through the real release lifecycle: a released
    /// reviewer's worktree with changes of its own is **not** detached. The
    /// checkout fails three times, the seat is parked, the captain gets one
    /// item — and the holder keeps its files.
    #[test]
    fn a_dirty_released_holder_keeps_its_files_and_the_checkout_stops_after_three_tries() {
        let (root, old, new, candidate) = scratch_review_repo("dirty");
        let mut herdr = FakeHerdr::new();
        let mut store = world_with_a_released_holder(&old, &new, &candidate, &mut herdr);
        crate::release::release_finished_reviews(&mut store, &mut herdr, "p", T0 + 5).unwrap();
        crate::release::release_finished_reviews(&mut store, &mut herdr, "p", T0 + 6).unwrap();
        std::fs::write(old.join("uncommitted.txt"), "mine\n").unwrap();

        // The first submission, run for real: the holder is dirty, so the
        // chain reports a failure instead of detaching it.
        let first = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        assert!(first.started.is_empty(), "{first:?}");
        let command = herdr
            .calls
            .iter()
            .rev()
            .find_map(|call| match call {
                Call::PaneRun { command, .. } if command.contains("git checkout -f -B") => {
                    Some(command.clone())
                }
                _ => None,
            })
            .expect("the checkout");
        let report = sh_out(&new, &command);
        assert!(report.contains(FAIL_MARKER), "{report}");
        assert_eq!(sh(&old, "git rev-parse --abbrev-ref HEAD"), "review/t1", "not detached");
        assert!(old.join("uncommitted.txt").exists(), "its changes are kept");

        // The three tries: the third failure stops it.
        let generation = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap().generation;
        for tick in 0..3 {
            fail_checkout(&mut store, &mut herdr, "p:review-1", generation);
            start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20 + tick).unwrap();
        }

        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Unknown, "the seat is parked");
        assert!(
            seat.state_reason.as_deref().unwrap_or_default().contains("failed 3 times"),
            "{:?}",
            seat.state_reason
        );
        assert!(starts(&herdr).is_empty(), "no reviewer on a refused checkout");
        assert!(old.join("uncommitted.txt").exists(), "its changes are still kept");
        assert_eq!(sh(&old, "git rev-parse --abbrev-ref HEAD"), "review/t1");
        // Three submissions in all: one, and two after a failure each.
        let checkouts = herdr
            .calls
            .iter()
            .filter(|call| matches!(call, Call::PaneRun { command, .. } if command.contains("git checkout -f -B")))
            .count();
        assert_eq!(checkouts, 3, "three submissions");
        // The captain is told once, not every tick.
        let captain = store.read(|tx| tx.captain_of("p")).unwrap().unwrap();
        let items = store.read(|tx| tx.actionable_for("p", &captain.id, T0 + 100)).unwrap();
        let about: Vec<_> = items.iter().filter(|item| item.task.as_deref() == Some("T-1")).collect();
        assert_eq!(about.len(), 1, "{items:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_review_chain_keeps_the_unmanaged_reviewer() {
        let text = CONFIG.replace(REVIEW_CHAIN, "review = []");
        let mut store = world(&text);
        let mut herdr = FakeHerdr::new();
        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();
        assert_eq!(report.started, vec!["p:review-1"]);
        // T-76: the folder the reviewer runs in is trusted, so the args are
        // the profile's with the worktree filled in.
        assert_eq!(
            starts(&herdr)[0].1,
            gear_herdr::render_args(&cfg().profiles["codex"].unattended_args, "/fake-worktrees/home-review-1"),
            "{:?}",
            starts(&herdr)
        );
    }

    #[test]
    fn a_reviewer_session_never_crosses_clis() {
        let mut store = world(CONFIG);
        store.write(|tx| {
            tx.set_meta(&session_key("p", "T-1"), "codex-session")?;
            tx.set_meta(&session_cli_key("p", "T-1"), "codex")
        }).unwrap();
        let config = gear_app::config::parse(CONFIG).unwrap();
        let entry = ModelEntry::Plain("claude-sonnet-5-5".into());
        let args = reviewer_args(&mut store, &config, &cfg().profiles["claude"], "claude", Some(&entry), "T-1").unwrap();
        assert!(!args.iter().any(|arg| arg == "codex-session"));
    }

    /// T-35: a reviewer never runs the writer's **current** model: the lane
    /// fell back to the review chain's first model, so the review runs the
    /// next one — on the CLI that entry names, with its arguments.
    #[test]
    fn a_review_never_runs_the_writers_current_model() {
        let text = r#"
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
review = [
  { model = "gpt-6.1-sol", cli = "codex", args = ["-c", "model_reasoning_effort=high"] },
  { model = "claude-sonnet-5-5", cli = "claude", args = ["--effort", "high"] },
]
"#;
        let mut store = world(text);
        // The lane fell back to the review chain's first model: that is the
        // model it wrote the hand-in on.
        store
            .write(|tx| tx.set_meta(&crate::model::model_meta_key("p:heat"), "gpt-6.1-sol"))
            .unwrap();
        let mut herdr = FakeHerdr::new();

        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();

        assert_eq!(report.started, vec!["p:review-1".to_string()]);
        let starts = starts(&herdr);
        assert_eq!(starts.len(), 1, "{starts:?}");
        assert_eq!(starts[0].0, "claude", "the CLI of the review's chain entry");
        assert_eq!(
            starts[0].1,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--model".to_string(),
                "claude-sonnet-5-5".to_string(),
                "--effort".to_string(),
                "high".to_string(),
            ],
            "the arguments of the review's chain entry"
        );
        // The reviewer never runs the model it reviews: not gpt-6.1-sol.
        assert!(!starts[0].1.contains(&"gpt-6.1-sol".to_string()), "{starts:?}");
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
            // The plain form: the temps kind decides the CLI.
            .replace(REVIEW_CHAIN, REVIEW_CHAIN_PLAIN)
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

    /// T-84: the review packet carries the project standard when the project
    /// root has `gear/standard.md`, and says nothing when it has none.
    #[test]
    fn the_review_packet_carries_the_project_standard() {
        let root = tempfile::tempdir().unwrap();
        let config_text = CONFIG.replace("repo = \"/repo\"", &format!("repo = \"{}\"", root.path().display()));
        let config = gear_app::config::parse(&config_text).unwrap();
        let mut store = world(&config_text);

        let without = review_packet_for(&mut store, &config, "T-1", 1, "abc1234").unwrap();
        assert!(!without.contains("Project standard:"), "{without}");

        std::fs::create_dir_all(root.path().join("gear")).unwrap();
        std::fs::write(root.path().join("gear/standard.md"), "# Standard\n").unwrap();
        let with = review_packet_for(&mut store, &config, "T-1", 1, "abc1234").unwrap();
        assert!(with.contains("Project standard: "), "{with}");
        assert!(with.contains("the reviewer judges against it"), "{with}");
        // The packet itself is unchanged around it.
        assert!(with.contains("Review T-1 attempt 1"), "{with}");
        assert!(with.contains("Diff to review"), "{with}");
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
                    "-c".to_string(),
                    "check_for_update_on_startup=false".to_string(),
                    // T-76: the reviewer's worktree is the folder it runs in.
                    "-c".to_string(),
                    "projects={\"/fake-worktrees/home-review-1\"={trust_level=\"trusted\"}}".to_string(),
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
        // The findings are text with ids; `--blocking` names the ones that
        // block (T-23).
        assert!(
            text.contains("gear send --kind review --task T-1 --attempt 1 --verdict <pass|fail>"),
            "{text}"
        );
        assert!(text.contains("--blocking <ids>"), "{text}");
        // The findings live in GEAR's store (T-21, shipped): the writer reads
        // the text from the rework note and the packet, not from the branch.
        assert!(text.contains("keeps the text of your findings"), "{text}");
        assert!(!text.contains("git show"), "no branch pointer in the packet:\n{text}");
    }

    /// T-33 (F5): an OpenCode reviewer gets no packet until its status bar
    /// shows the review chain's model — the switch happens first, in place.
    #[test]
    fn an_opencode_reviewer_is_switched_before_the_packet() {
        let text = CONFIG
            .replace(
                "[temps]\nkind = \"codex\"\nprofile = \"codex\"",
                "[temps]\nkind = \"opencode\"\nprofile = \"opencode\"",
            )
            // The plain form: the temps kind decides the CLI.
            .replace(REVIEW_CHAIN, REVIEW_CHAIN_PLAIN)
            + "\n[models.switch.\"gpt-6.1-sol\"]\npicker = \"GPT-6.1 Sol\"\nbar = \"GPT-6.1 Sol\"\n";
        let mut store = world(&text);
        let mut herdr = FakeHerdr::new();
        // The reviewer starts on another model.
        herdr.start_screen = Some("Build auto · MiniMax-M3.1-Flash MiniMax · high".to_string());
        herdr.screen_after = vec![
            ("esc".into(), "  Ask anything…".into()),
            ("/model".into(), "  Search\n  Recent\n  GPT-6.1 Sol  OpenAI".into()),
            ("GPT-6.1 Sol".into(), "Build auto · GPT-6.1 Sol OpenAI · high".into()),
        ];

        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        let first = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();
        assert!(first.started.is_empty(), "the packet waits for the model: {first:?}");

        let mut delivered = false;
        for tick in 0..6 {
            let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 30 + tick).unwrap();
            if !report.started.is_empty() {
                delivered = true;
                break;
            }
        }
        assert!(delivered, "the reviewer starts once the bar confirms");
        // The packet is delivered once, and the reviewer was started once.
        let deliveries: Vec<_> = herdr
            .calls
            .iter()
            .filter(|call| matches!(call, Call::AgentStart { .. }))
            .collect();
        assert_eq!(deliveries.len(), 1, "{:?}", herdr.calls);
        let commands: Vec<String> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::PaneType { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(commands.iter().filter(|c| *c == "/model").count(), 1, "{commands:?}");
        assert!(commands.iter().any(|c| c == "GPT-6.1 Sol"), "{commands:?}");
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
        herdr.refuse_starts = Some(("agent_pane_busy".into(), 1));
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
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
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
                    estimate_min: None,
                     issue: None,updated: T0,
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

        // Attempt 2 of the same task is handed in — with a **new** candidate.
        // Not a style choice: `submit_result` refuses a candidate that already
        // failed on an earlier attempt of the task (T-118), so reusing
        // "abc1234" fails here with `candidate abc1234 already failed on
        // attempt 1` (T-124: that rule, not a race, was the one failure seen
        // — it reproduces 3 of 3 runs, alone or under load).
        // Nothing else in this test can vary: its store is in memory, its
        // Herdr is the fake, and every timestamp is given (T0 + n) — no clock,
        // no sleep, no temp path, no global.
        store
            .write(|tx| {
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + 25)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 2, T0 + 25)?;
                gear_app::tasks::submit_result(tx, "p", "T-1", 2, 1, "def5678", "ops/reports/T-1.md", T0 + 25)
            })
            .unwrap();

        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 30).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 2, "def5678", "review/t1");
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

    /// The project with a child repo: the lane works in it (T-53).
    fn config_with_repos() -> String {
        CONFIG
            .replace(
                "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/repo\"",
                "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/repo\"\n\
                 \n[[repos]]\nname = \"web\"\npath = \"web\"\nintegration = \"web-dev\"",
            )
            .replace(
                "[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]",
                "[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]\nrepo = \"web\"",
            )
    }

    /// The worktree the engine created for the reviewer: (repo, branch, base,
    /// label).
    fn created_worktree(herdr: &FakeHerdr) -> (String, String, String, String) {
        herdr.calls
            .iter()
            .find_map(|call| match call {
                Call::WorktreeCreate { repo, branch, base, label } => {
                    Some((repo.clone(), branch.clone(), base.clone(), label.clone()))
                }
                _ => None,
            })
            .expect("the reviewer got a worktree")
    }

    /// T-53: the reviewer's worktree is in the repo of the task's lane, cut
    /// from that repo's integration branch, and the packet names that branch.
    #[test]
    fn a_review_works_in_the_repo_of_the_task() {
        let mut store = world(&config_with_repos());
        let mut herdr = FakeHerdr::new();

        start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
        confirm_checkout(&mut store, &mut herdr, "p:review-1", 1, "abc1234", "review/t1");
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();

        assert_eq!(report.started, vec!["p:review-1".to_string()]);
        let (repo, branch, base, label) = created_worktree(&herdr);
        assert_eq!(repo, "/repo/web", "the worktree is in the task's repo");
        assert_eq!(branch, "home/review-1");
        assert_eq!(base, "web-dev", "the repo's own integration branch");
        assert_eq!(label, "p-review-1-web", "the label names the repo");
        // The packet asks for the diff against that repo's integration branch.
        let packet = store.read(|tx| packet_text(tx, "p:review-1", "T-1", 1)).unwrap();
        assert!(packet.contains("Diff to review: `web-dev..abc1234`"), "{packet}");
    }

    #[test]
    fn a_warm_reviewer_returns_to_its_saved_repo_checkout() {
        let config = config_with_repos().replace("lanes = 1", "lanes = 2") + r#"
[[lanes]]
name = "root"
duty = "root work"
owns = ["root/"]
kind = "opencode"
profile = "opencode"
"#;
        let mut store = world(&config);
        // Isolated repo doors prevent the fake's branch-only map from letting
        // root's home/review-1 stand in for web's checkout. Distinct paths and
        // panes also make a checkout in the wrong repo observable.
        let mut web = FakeHerdr::new();
        let mut root = FakeHerdr::new();
        for (door, path, pane) in [
            (&mut web, "/web-review-1", "web:p1"),
            (&mut root, "/root-review-1", "root:p1"),
        ] {
            door.add_shell(pane);
            door.add_worktree("home/review-1", path, pane);
        }
        for (n, lane, door, path, pane, repo, base) in [
            (1, "heat", &mut web, "/web-review-1", "web:p1", "/repo/web", "web-dev"),
            (2, "root", &mut root, "/root-review-1", "root:p1", "/repo", "dev"),
        ] {
            if n == 2 {
                queue_review_job(&mut store, n, lane);
            }
            start_reviews(&mut store, door, &cfg(), &cfg().profiles, "p", T0 + n * 10).unwrap();
            confirm_checkout(&mut store, door, "p:review-1", n, "abc1234", &format!("review/t{n}"));
            assert_eq!(start_reviews(&mut store, door, &cfg(), &cfg().profiles, "p", T0 + n * 10 + 1)
                .unwrap().started, vec!["p:review-1"]);
            assert!(door.calls.iter().any(|call| matches!(call,
                Call::WorktreeOpen { repo: got, .. } if got == repo)));
            let packet = store.read(|tx| packet_text(tx, "p:review-1", &format!("T-{n}"), 1)).unwrap();
            assert!(packet.contains(&format!("Diff to review: `{base}..abc1234`")), "{packet}");
            release_review_job(&mut store, n);
            // The real checkout is now on the job branch. Remove the fake's
            // anchor lookup and register that branch at the unchanged path.
            assert!(matches!(door.worktree_remove("wt-home-review-1"), Outcome::Ok(())));
            door.add_shell(pane);
            door.add_worktree(&format!("review/t{n}"), path, pane);
        }
        queue_review_job(&mut store, 3, "heat");
        let before = web.calls.len();
        start_reviews(&mut store, &mut web, &cfg(), &cfg().profiles, "p", T0 + 30).unwrap();
        let calls = &web.calls[before..];
        assert!(calls.iter().any(|call| matches!(call, Call::WorktreeOpenPath { repo, path, label }
            if repo == "/repo/web" && path == "/web-review-1" && label == "p-review-1-web")), "{calls:?}");
        assert!(!calls.iter().any(|call| matches!(call, Call::WorktreeCreate { .. } | Call::WorktreeOpen { .. })), "{calls:?}");
        assert!(calls.iter().any(|call| matches!(call, Call::PaneRun { pane, command }
            if pane == "web:p1" && command.contains("/web-review-1") && command.contains("git checkout -f -B review/t3 abc1234"))), "{calls:?}");
        confirm_checkout(&mut store, &mut web, "p:review-1", 3, "abc1234", "review/t3");
        assert_eq!(start_reviews(&mut store, &mut web, &cfg(), &cfg().profiles, "p", T0 + 31)
            .unwrap().started, vec!["p:review-1"]);
    }

    fn queue_review_job(store: &mut Store, n: i64, lane: &str) {
        store.write(|tx| {
            let mut task = tx.get_task("p", "T-1")?.unwrap();
            task.id = format!("T-{n}");
            task.rank = n;
            task.state = TaskState::Ready;
            task.lane = Some(lane.into());
            tx.insert_task(&task)?;
            tx.insert_contract(&ContractRow {
                project: "p".into(), task: task.id.clone(), version: 1,
                goal: "next job".into(), accept: "review it".into(),
                evidence_files: vec![], evidence_due_min: None, body: String::new(), created: T0,
            })?;
            gear_app::tasks::start_attempt(tx, "p", &task.id, "p:heat", 1, T0)?;
            gear_app::tasks::on_delivery_confirmed(tx, "p", &task.id, 1, T0)?;
            gear_app::tasks::submit_result(tx, "p", &task.id, 1, 1, "abc1234", "report.md", T0)
        }).unwrap();
    }

    fn release_review_job(store: &mut Store, n: i64) {
        store.write(|tx| {
            let mut task = tx.get_task("p", &format!("T-{n}"))?.unwrap();
            task.state = TaskState::Done;
            tx.update_task(&task)?;
            tx.set_seat_state("p:review-1", SeatState::Released, Some("review done"), T0 + n * 10 + 2)?;
            tx.set_binding_status("p:review-1", n, BindingStatus::Void)
        }).unwrap();
    }

    #[test]
    fn legacy_review_paths_are_archived_before_switching_repos() {
        for recorded in [None, Some("/repo"), Some("/repo/web")] {
            let mut store = world(&config_with_repos());
            store.write(|tx| {
                tx.set_meta(&path_key("p:review-1"), "/legacy-review")?;
                if let Some(repo) = recorded { tx.set_meta(&repo_key("p:review-1"), repo)?; }
                Ok(())
            }).unwrap();
            // Force the next review into the other repo.
            let config = if recorded == Some("/repo/web") { CONFIG } else { &config_with_repos() };
            store.write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config.into();
                tx.update_project(&project)
            }).unwrap();
            start_reviews(&mut store, &mut FakeHerdr::new(), &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
            let old_repo = gear_app::config::parse(&config_with_repos()).unwrap()
                .repo_of_lane(if recorded == Some("/repo/web") { "heat" } else { "root" });
            assert_eq!(worktree_in(&mut store, "p:review-1", &old_repo, "/repo").unwrap().as_deref(), Some("/legacy-review"));
        }
    }

    /// T-53: a warm reviewer whose last worktree is in another repo gets a
    /// worktree in the repo of the task it takes now.
    #[test]
    fn a_warm_reviewer_in_another_repo_gets_a_worktree_in_this_one() {
        // Two ways the seat's last worktree can be in another repo: it was
        // recorded as the project's root, or it is an old seat with no repo
        // recorded at all (its worktree is the project's root).
        for recorded in [Some("/repo".to_string()), None] {
            let mut store = world(&config_with_repos());
            store
                .write(|tx| {
                    tx.set_meta(&path_key("p:review-1"), "/root-review-1")?;
                    if let Some(repo) = &recorded {
                        tx.set_meta(&repo_key("p:review-1"), repo)?;
                    }
                    Ok(())
                })
                .unwrap();
            let mut herdr = FakeHerdr::new();

            start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();

            let (repo, _, base, _) = created_worktree(&herdr);
            assert_eq!(repo, "/repo/web", "the worktree is in the task's repo");
            assert_eq!(base, "web-dev");
            assert!(
                !herdr.calls.iter().any(|call| matches!(call, Call::WorktreeOpenPath { .. })),
                "the checkout of another repo is never reopened: {:?}",
                herdr.calls
            );
        }
    }

    /// T-143 (design §3.6): the temps quota counts the seats with a live
    /// agent — a Standby reviewer holds a slot like an active one, so a
    /// review never passes the quota.
    #[test]
    fn a_standby_reviewer_holds_a_temp_slot() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        store
            .write(|tx| {
                // T-1 is judged: it waits for no review.
                tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    passed: true,
                    findings: "ok".into(),
                    by_seat: "p:review-1".into(),
                    created: T0,
                })?;
                // The reviewer that judged it waits for the next review: it
                // holds the one temp slot of the project.
                let seat = testing::seed_temp_seat(tx, "p", "review-1", "T-1", "w2:p2", "codex", T0)?;
                tx.set_seat_state(&seat.id, SeatState::Standby, Some("review done"), T0)?;
                tx.clear_seat_task(&seat.id)?;
                // A free seat from an earlier job: warm, but it needs a slot.
                let warm = testing::seed_temp_seat(tx, "p", "review-2", "T-1", "w3:p3", "codex", T0)?;
                tx.set_seat_state(&warm.id, SeatState::Released, Some("review done"), T0)?;
                Ok(())
            })
            .unwrap();

        // The count itself: the standby reviewer holds the slot, the released
        // one holds nothing.
        assert!(
            !has_free_slot(&mut store, &mut herdr, "p", 1).unwrap(),
            "the standby reviewer holds the slot"
        );
        assert!(
            has_free_slot(&mut store, &mut herdr, "p", 2).unwrap(),
            "a second slot is free"
        );

        // A reviewer on its way out holds its slot too (the captain's decision
        // of M-2603): its agent is still leaving.
        store
            .write(|tx| tx.set_seat_state("p:review-1", SeatState::Releasing, Some("released"), T0 + 1))
            .unwrap();
        assert!(
            !has_free_slot(&mut store, &mut herdr, "p", 1).unwrap(),
            "the reviewer on its way out holds the slot"
        );
        store
            .write(|tx| tx.set_seat_state("p:review-1", SeatState::Standby, Some("review done"), T0))
            .unwrap();

        // A second task is handed in: the warm seat could take it, but the
        // standby reviewer holds the only slot of the project.
        queue_review_job(&mut store, 2, "heat");
        let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();

        assert!(report.started.is_empty(), "{report:?}");
        assert_eq!(report.waiting, vec!["T-2".to_string()], "{report:?}");
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
/// The review packet first sent to this seat for `task`/`attempt` (the
/// delivery that is not a nudge), so a nudge can carry it again.
fn packet_text(tx: &Tx<'_>, seat: &str, task: &str, attempt: i64) -> GearResult<String> {
    let first = tx
        .ops_of_seat_kind(seat, OpKind::Deliver)?
        .into_iter()
        .filter(|op| {
            op.task.as_deref() == Some(task)
                && op.attempt == Some(attempt)
                && op.payload.get("nudge").is_none()
        })
        .next_back();
    Ok(first
        .and_then(|op| op.payload.get("text").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_default())
}

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
            let Some(last) = tx.last_op(&seat.id, OpKind::Deliver)? else {
                return Ok(None); // the packet itself is not sent yet
            };
            if now - last.created < cooldown_s {
                return Ok(None);
            }
            // Never two deliveries in flight — but a review packet stays
            // "submitted" until the verdict, so a submitted delivery older than
            // the cooldown on an idle reviewer does not block: it is exactly
            // the case the nudge is for (the packet or its Enter was lost,
            // L-76). Anything not yet submitted still waits.
            let blocking = tx
                .slot_ops(&seat.id)?
                .into_iter()
                .any(|op| !(op.state == gear_model::OpState::Submitted && now - op.created >= cooldown_s));
            if blocking {
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
                        "You are idle and your verdict on {task} is not in. If you never got your review packet (it can be lost right after a start), here it is again — start from it. If you had started, continue where you stopped (a server error such as \"model at capacity\", or a pause), then hand in the verdict with the `gear send --kind review` command from the packet.\n\n{}",
                        packet_text(tx, &seat.id, &task, attempt.n)?
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
