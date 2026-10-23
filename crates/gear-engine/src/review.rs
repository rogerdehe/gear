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
//!   diff range, the rules in `ops/REVIEW.md` and the exact command to hand
//!   the verdict in.
//!
//! The lane is never blocked: the reviewer is a seat of its own.

use gear_app::config::ProjectConfig;
use gear_herdr::{HerdrAct, Outcome, Profile};
use gear_model::{
    AttemptState, ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, SeatRole, SeatState,
    TaskState, Ts,
};
use gear_store::{NewMessage, NewOp, SeatRow, Store, Tx};

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
    let Some(profile) = cfg.profiles.get(&temps.profile) else {
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
            None => create_temp_seat(store, project, &task.task, &temps.profile, now)?,
        };

        // 2. a seat that is already up needs nothing; a seat that was parked
        //    (unknown, needs check, missing) is never retried (design/10).
        if !matches!(seat.state, SeatState::Queued | SeatState::Starting) {
            continue;
        }

        // 3. a free temp slot? A reviewer that already holds one (starting)
        //    carries on; a queued one waits, and the captain is told once
        //    (design/10: QUEUED happens only when the quota is exhausted; the
        //    quota is the project config's `[quota].temps`).
        if !occupies_quota(seat.state)
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
            let Some(candidate) = attempt.candidate.clone() else {
                continue;
            };
            // A reviewer that is up already: it is not started twice. A
            // reviewer that waits (queued) or is not up yet (starting) is
            // still this task's reviewer: the tick starts it when it can.
            if seats.iter().any(|seat| {
                seat.role == SeatRole::Temp
                    && seat.task_id.as_deref() == Some(task.id.as_str())
                    && !matches!(seat.state, SeatState::Queued | SeatState::Starting)
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
            .find(|seat| seat.role == SeatRole::Temp && seat.task_id.as_deref() == Some(task)))
    })
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

/// The next free temp seat name: `r-1`, `r-2`, … (the R-number of the temp
/// request, as in the plan's staff lines).
fn next_temp_name(tx: &Tx<'_>, project: &str) -> GearResult<String> {
    let seats = tx.seats_of_project(project)?;
    let mut n = 1;
    loop {
        let name = format!("r-{n}");
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
    let branch = review_branch(&task.task);
    let label = agent_name(config, &task.task);
    let kind = config
        .temps
        .as_ref()
        .map(|temps| temps.kind.clone())
        .unwrap_or_default();

    // 1. the pane: the one of the seat's binding (a reviewer that was not
    //    started yet), else the worktree of the branch — opened, or created
    //    at the candidate the writer handed in.
    let binding = store.read(|tx| tx.get_binding(&seat.id, seat.generation))?;
    let (pane, generation) = match binding {
        Some(binding) => (binding.pane_id, seat.generation),
        None => {
            let worktree = match herdr.worktree_open(repo, &branch, &label) {
                Outcome::Ok(worktree) => worktree,
                Outcome::Refused { code, .. } if code == "worktree_not_found" => {
                    match herdr.worktree_create(repo, &branch, &task.candidate, &label) {
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
            let pane = worktree.pane_id;
            let generation = seat.generation + 1;

            // 2. the environment: the call context, and the model when the
            //    CLI takes one. It is set before the agent starts, so the
            //    agent inherits it.
            let mut env_command = format!(
                "export GEAR_CTX={}:{}:{}; export GEAR_DB={}",
                config.project.id, seat.name, generation, cfg.db_path
            );
            let mut args = profile.unattended_args.clone();
            if let Some(model) = model {
                if let Some(start) = model_start(&kind, model) {
                    if !start.env.is_empty() {
                        env_command = format!("{env_command}; {}", start.env);
                    }
                    args.extend(start.args);
                }
            }
            if let Outcome::Ok(()) = herdr.pane_run(&pane, &env_command) {
                // the environment is set
            } else {
                park(
                    store,
                    &seat.id,
                    "pane_run: the environment could not be set",
                    now,
                )?;
                return Ok(Started::NotYet);
            }

            // 3. the binding: the seat's first generation.
            if let Err(reason) =
                store.write(|tx| gear_app::seats::begin_onboard(tx, &seat.id, &pane, &kind, now))
            {
                park(
                    store,
                    &seat.id,
                    &format!("begin_onboard: {}", reason.detail),
                    now,
                )?;
                return Ok(Started::NotYet);
            }
            (pane, generation)
        }
    };

    // 4. the agent. Its args carry the model when the CLI takes one.
    let mut args = profile.unattended_args.clone();
    if let Some(model) = model {
        if let Some(start) = model_start(&kind, model) {
            args.extend(start.args);
        }
    }
    // The pane may already hold this reviewer: an engine restart can cut the
    // start between Herdr accepting it and the seat being marked Active (the
    // captain restarts the engine on every merge, so this is the common case,
    // not an exotic one). Adopt that agent — a second `agent start` would be
    // refused and the live reviewer would be parked without its job.
    let already = match herdr.agent_get(&pane) {
        Ok(Some(info)) if info.agent.as_deref() == Some(kind.as_str()) => Some(info),
        Ok(Some(_)) => {
            // Another agent is in the pane: do not start, and never take the
            // pane from it. A human decides.
            park(
                store,
                &seat.id,
                "the reviewer's pane is held by another agent",
                now,
            )?;
            return Ok(Started::NotYet);
        }
        Ok(None) => None,
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
    let started = match already {
        // The start was accepted before the restart: adopt it.
        Some(info) => info,
        None => match herdr.agent_start(&label, &kind, &pane, &args) {
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
        },
    };

    // 5. the seat is up and has its packet: one transaction. A crash between
    //    the two would leave a live reviewer that every later tick skips (an
    //    Active temp is not started again) and that never gets a job.
    let session_ref = started.agent_session.as_ref().map(|session| session.value.as_str());
    let packet = review_packet_for(store, config, &task.task, task.attempt, &task.candidate)?;
    store.write(|tx| {
        gear_app::seats::finish_onboard(tx, &seat.id, generation, session_ref, now)?;
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
         Rules: `ops/REVIEW.md` in your worktree — the mechanical acceptance you run yourself, \
         the findings with their severity, the template of `ops/reviews/{task}.md` and the \
         verdict rule.\n\
         \n\
         Hand your verdict in with exactly:\n\
         gear send --kind review --task {task} --attempt {attempt} --verdict <pass|fail> \
         \"ops/reviews/{task}.md\"\n\
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
        let from = if seat.state == SeatState::Queued {
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

    #[test]
    fn a_result_in_starts_one_reviewer() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();

        let report = start_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 10).unwrap();

        // one temp seat, bound to the task for life
        assert_eq!(report.started, vec!["p:r-1".to_string()]);
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.role, SeatRole::Temp);
        assert_eq!(seat.task_id.as_deref(), Some("T-1"));
        assert_eq!(seat.state, SeatState::Active);

        // one worktree, on the review branch, cut at the candidate
        assert_eq!(
            worktrees(&herdr),
            vec![("review/t1".to_string(), "abc1234".to_string())]
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
        assert_eq!(op.seat, "p:r-1");
        assert_eq!(op.task.as_deref(), Some("T-1"));
        let text = op.payload.get("text").and_then(|t| t.as_str()).unwrap();
        assert!(text.contains("the goal"), "{text}");
        assert!(text.contains("the accept criteria"), "{text}");
        assert!(text.contains("abc1234"), "{text}");
        assert!(text.contains("dev..abc1234"), "{text}");
        assert!(text.contains("ops/REVIEW.md"), "{text}");
        assert!(
            text.contains(
                "gear send --kind review --task T-1 --attempt 1 --verdict <pass|fail> \
                 \"ops/reviews/T-1.md\""
            ),
            "{text}"
        );
    }

    #[test]
    fn the_tick_starts_the_reviewer_and_delivers_the_packet() {
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        // The lane and the captain are there, so the tick observes them.
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Idle);
        herdr.add_agent("w1:p9", "claude", gear_herdr::AgentStatus::Idle);

        let report = crate::tick(&mut store, &mut herdr, &cfg(), T0 + 10).unwrap();

        assert_eq!(report.reviewers, vec!["p:r-1".to_string()]);
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

        start_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 10).unwrap();
        let report = start_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 20).unwrap();

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

        let report = start_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 10).unwrap();

        assert!(report.started.is_empty());
        assert_eq!(report.waiting, vec!["T-1".to_string()]);
        assert!(worktrees(&herdr).is_empty());
        assert!(starts(&herdr).is_empty());
        // The reviewer waits in a queued seat, and the captain is told once.
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Queued);
        assert_eq!(seat.task_id.as_deref(), Some("T-1"));
        let inbox = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 11)).unwrap();
        assert_eq!(inbox.len(), 1, "the captain is told: {inbox:?}");
        assert!(inbox[0].body.contains("T-1"), "{}", inbox[0].body);
        // The next tick does not tell the captain again.
        start_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 20).unwrap();
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
                    id: "p:r-1".into(),
                    project: "p".into(),
                    role: SeatRole::Temp,
                    name: "r-1".into(),
                    profile: "codex".into(),
                    generation: 1,
                    state: SeatState::Starting,
                    state_since: T0,
                    state_reason: Some("onboarding".into()),
                    task_id: Some("T-1".into()),
                })?;
                tx.insert_binding(&BindingRow {
                    seat: "p:r-1".into(),
                    generation: 1,
                    session: "sess".into(),
                    pane_id: pane.into(),
                    kind: "codex".into(),
                    session_ref: None,
                    status: BindingStatus::Pending,
                    handover_id: None,
                    created: T0,
                })?;
                Ok(())
            })
            .unwrap();
        // The shell is still starting.
        herdr.inject_next =
            Some(gear_herdr::fake::Inject::Refuse { code: "agent_pane_busy".into() });

        let report = start_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 10).unwrap();

        assert!(report.started.is_empty(), "the agent is not up yet");
        assert_eq!(report.waiting, vec!["T-1".to_string()]);
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Starting, "not parked: tried again");
        // The next tick starts the agent: the worktree and the environment are
        // already there.
        let report = start_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 20).unwrap();
        assert_eq!(report.started, vec!["p:r-1".to_string()]);
        assert_eq!(starts(&herdr).len(), 2, "the start was tried again");
        assert!(worktrees(&herdr).is_empty(), "no second worktree");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active);
    }

    #[test]
    fn an_accepted_start_is_adopted_not_repeated() {
        // P1 #1 of the review: an engine restart can cut the start between
        // Herdr accepting it and the seat going Active. The pane then holds a
        // live reviewer with a Pending binding; the tick must adopt it, never
        // start a second one (a second start is refused and would park a live
        // reviewer without its job).
        let mut store = world(CONFIG);
        let mut herdr = FakeHerdr::new();
        let pane = "wt1:p1";
        herdr.add_shell(pane);
        // Herdr accepted the start: the codex agent is in the pane.
        herdr.add_agent(pane, "codex", gear_herdr::AgentStatus::Idle);
        store
            .write(|tx| {
                tx.insert_seat(&SeatRow {
                    id: "p:r-1".into(),
                    project: "p".into(),
                    role: SeatRole::Temp,
                    name: "r-1".into(),
                    profile: "codex".into(),
                    generation: 1,
                    state: SeatState::Starting,
                    state_since: T0,
                    state_reason: Some("onboarding".into()),
                    task_id: Some("T-1".into()),
                })?;
                tx.insert_binding(&BindingRow {
                    seat: "p:r-1".into(),
                    generation: 1,
                    session: "sess".into(),
                    pane_id: pane.into(),
                    kind: "codex".into(),
                    session_ref: Some("ses_reviewer".into()),
                    status: BindingStatus::Pending,
                    handover_id: None,
                    created: T0,
                })?;
                Ok(())
            })
            .unwrap();

        let report = crate::tick(&mut store, &mut herdr, &cfg(), T0 + 10).unwrap();

        assert_eq!(report.reviewers, vec!["p:r-1".to_string()]);
        // No second start: the agent that was already there is adopted.
        assert!(starts(&herdr).is_empty(), "no second agent_start: {:?}", starts(&herdr));
        assert!(worktrees(&herdr).is_empty(), "the worktree was already made");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active);
        // … and it got its packet (the tick executes the delivery).
        let prompts = herdr.prompts_to(pane);
        assert_eq!(prompts.len(), 1, "{prompts:?}");
        assert!(prompts[0].starts_with("Review T-1 attempt 1."), "{}", prompts[0]);
        let in_flight = store.read(|tx| tx.slot_ops("p:r-1")).unwrap();
        assert_eq!(in_flight.len(), 1, "{in_flight:?}");
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
                    id: "p:r-1".into(),
                    project: "p".into(),
                    role: SeatRole::Temp,
                    name: "r-1".into(),
                    profile: "codex".into(),
                    generation: 1,
                    state: SeatState::Active,
                    state_since: T0,
                    state_reason: Some("onboarded".into()),
                    task_id: Some("T-1".into()),
                })?;
                tx.insert_binding(&BindingRow {
                    seat: "p:r-1".into(),
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
        assert_eq!(report.reviewers, vec!["p:r-1".to_string()]);
        // One packet reached the live reviewer, and it was not started again.
        assert_eq!(herdr.prompts_to(pane).len(), 1, "{:?}", herdr.prompts_to(pane));
        assert!(starts(&herdr).is_empty());

        // The next tick adds nothing: the packet is the guard.
        let report = crate::tick(&mut store, &mut herdr, &cfg(), T0 + 20).unwrap();
        assert!(report.reviewers.is_empty(), "{:?}", report.reviewers);
        assert_eq!(herdr.prompts_to(pane).len(), 1);
    }
}
