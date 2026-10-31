//! Continue / assign decisions (design/09). Pure: no store, no Herdr. The
//! engine builds a [`LaneView`] per lane seat each tick and turns the
//! [`Action`] into ops (08).

use crate::delivery::FailedDelivery;
use crate::observe::SeatObs;
use gear_herdr::ScreenFlag;
use gear_model::GearResult;
use gear_model::{AttemptState, MsgKind, MsgState, OpKind, SeatRole, SeatState, TaskState, Ts};
use gear_store::{AttemptRow, NewMessage, OpRow, SeatRow, Store, TaskRow, Tx};

#[derive(Debug, Clone, PartialEq)]
pub struct LaneView {
    /// This tick's observation (seat row inside); None = observation failed.
    pub obs: Option<SeatObs>,
    /// The seat's current non-terminal attempt, if any.
    pub attempt: Option<AttemptRow>,
    /// Any deliver/doorbell op for this seat in Intended/Executing/Submitted/Unknown.
    pub pending_op: Option<OpRow>,
    pub last_deliver_at: Option<Ts>,
    /// Continues sent since the last verifiable progress (design/09).
    pub nudges_since_progress: u32,
    /// This lane's Ready tasks in rank order (readiness already checked).
    pub ready_tasks: Vec<TaskRow>,
    /// design/06 "safe conditions for taking the next item" hold for a Submitted attempt.
    pub safe_for_next: bool,
    /// An in-place model switch is under way (T-33, F6): no prompt may land in
    /// the agent while its picker or dialog is open.
    pub switch_pending: bool,
    /// The last delivery to this seat that Herdr accepted and the lane never
    /// confirmed (T-97). It is delivered once more in a new agent session — or
    /// at once when a switch swallowed it — never every tick.
    pub failed_delivery: Option<FailedDelivery>,
    /// T-290: this seat's next packet is a **rework** (the first ready task
    /// carries a `rework_note` and its last attempt is this seat's, Rejected)
    /// and its agent queues input typed while it works (pi, claude). The
    /// packet may go while the agent is Working: it waits in the input queue
    /// and is read after the current step.
    pub rework_mid_turn: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchCfg {
    pub cooldown_s: i64,
    /// Continues without verifiable progress before escalating (default 5).
    pub nudge_budget: u32,
}

impl Default for DispatchCfg {
    fn default() -> Self {
        DispatchCfg {
            cooldown_s: 90,
            nudge_budget: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None {
        reason: String,
    },
    /// Same task, same attempt.
    Continue {
        task: String,
    },
    /// First ready task.
    Assign {
        task: String,
    },
    /// The task's contract moved on; the lane adopts it at its next view.
    Adopt {
        task: String,
        version: i64,
    },
    /// Tell the captain; stop nudging.
    Escalate {
        task: String,
        reason: String,
    },
}

/// The decision order of design/09 "Decision order", first match wins:
/// 1. seat state not Active/Standby/Retiring → None;
/// 2. `obs` None → None;
/// 3. `pending_op` present → None (never resend while a delivery is unsettled);
/// 4. agent not idle (`obs.idle` false) → None, unless the next packet is a
///    rework for an agent that queues input while it works
///    (`v.rework_mid_turn` and the agent reports Working, T-290);
/// 5. screen flag RateLimit/Quota/Auth → None;
/// 6. `now - last_deliver_at < cooldown_s` → None;
/// 7. attempt Started/Stopped/Delivered → Continue, or Escalate when
///    `nudges_since_progress >= nudge_budget` (a Delivered attempt was
///    confirmed but the agent never started, so it is nudged like a stopped
///    one);
///    a Started attempt gets no delivery at all (design §10.3: one packet per
///    round);
///    attempt Submitted → Assign the first ready task if `safe_for_next`, else None;
///    attempt Unknown → None (an unknown outcome is never retried);
///    no attempt (or a terminal one, which the view contract excludes) →
///    Assign the first ready task, else None;
///    seat Retiring never Assigns (Continue allowed);
///    temp seats (role Temp) never Assign.
/// `reason` texts are free-form but non-empty.
pub fn decide(v: &LaneView, cfg: &DispatchCfg, now: Ts) -> Action {
    // 1–2. The seat must be one that may take work, and must have been
    //      observed at all.
    let Some(obs) = &v.obs else {
        return none("observation failed this tick");
    };
    let seat = &obs.seat;
    if !matches!(
        seat.state,
        SeatState::Active | SeatState::Standby | SeatState::Retiring
    ) {
        return none("the seat may not take work in its current state");
    }
    // 3. A delivery that is not settled yet blocks everything.
    if v.pending_op.is_some() {
        return none("a delivery for this seat is still unsettled");
    }
    // 3b. A model switch is typing into this agent (T-33, F6): its picker,
    //     its variant dialog and the bar check own the pane until it confirms.
    if v.switch_pending {
        return none("a model switch is under way for this seat");
    }
    // 4. Design §10.2: only `idle` and `done` are ready. A working, blocked
    //    or unknown agent is never ready for a delivery — the text waits for
    //    the agent to be idle again (T-68: a seat on a dialog is not
    //    re-delivered every tick).
    //    T-290: one exception — a rework packet for a lane whose agent queues
    //    input while it works (pi, claude) goes into the queue and is read
    //    after the current step. A first packet, a lane with other work in
    //    flight, and an opencode or codex lane still wait for idle.
    if !obs.idle {
        let mid_turn = v.rework_mid_turn && obs.status == Some(gear_herdr::AgentStatus::Working);
        if !mid_turn {
            return none("the agent is not ready");
        }
    }
    // 5. A rate limit / quota / auth wall is not nudged through.
    if obs.flags.iter().any(|flag| {
        matches!(
            flag,
            ScreenFlag::RateLimit | ScreenFlag::Quota | ScreenFlag::Auth
        )
    }) {
        return none("the screen shows a ratelimit, quota or auth problem");
    }
    // 6. Respected since the last delivery (the design's 90 s default).
    if let Some(last) = v.last_deliver_at {
        if now - last < cfg.cooldown_s {
            return none("cooldown since the last delivery");
        }
    }
    // 7. Continue the current attempt, or assign the next ready task.
    let may_assign = seat.state != SeatState::Retiring && seat.role != SeatRole::Temp;
    match &v.attempt {
        Some(attempt) => match attempt.state {
            // Started/Stopped: the agent worked and stopped. Delivered: the
            // delivery was confirmed but the agent never started (R-31) — it
            // is nudged the same way instead of waiting forever.
            AttemptState::Started | AttemptState::Stopped | AttemptState::Delivered => {
                // T-97: a delivery that was never confirmed — the assignment
                // or a continue — is not typed into the same agent session
                // again. It goes once more when the pane reports a new session
                // (a hand restart) or the seat has a new generation (a restart
                // GEAR did), and at once when a model switch swallowed it.
                if let Some(failed) = &v.failed_delivery {
                    if failed.task == attempt.task && failed.attempt == attempt.n {
                        let new_session = obs.session != failed.session;
                        let new_generation = seat.generation != failed.generation;
                        if !(failed.retry_now || new_session || new_generation) {
                            return none(
                                "the delivery was not confirmed in this agent session; it goes again in a new one",
                            );
                        }
                    }
                }
                // A contract update the lane has not adopted yet is named once,
                // before any continue (T-8, design/06). The marker holds the
                // newest version, so two bumps still give one delivery.
                if let Some(version) = attempt.pending_update {
                    if version > attempt.adopted_version
                        && !(attempt.state == AttemptState::Delivered && attempt.sent_at.is_none())
                    {
                        return Action::Adopt {
                            task: attempt.task.clone(),
                            version,
                        };
                    }
                }
                // Design §10.3: an attempt that already Started got its packet
                // and the lane checked in — the engine never delivers it a
                // second one. The lane that stops working is reached by the
                // overrun question of design §5.2, not by a second packet.
                if attempt.state == AttemptState::Started {
                    return none("the attempt already started; no second packet");
                }
                if v.nudges_since_progress >= cfg.nudge_budget {
                    Action::Escalate {
                        task: attempt.task.clone(),
                        reason: format!(
                            "no verifiable progress after {} continues",
                            v.nudges_since_progress
                        ),
                    }
                } else {
                    Action::Continue {
                        task: attempt.task.clone(),
                    }
                }
            }
            AttemptState::Submitted => {
                // Do not re-implement a submitted task; a second task only at
                // a safe point (design/06).
                if !v.safe_for_next {
                    return none("the submitted attempt is not at a safe point");
                }
                assign_first(&v.ready_tasks, may_assign)
            }
            // An unknown outcome is never retried, and a second task would
            // break "one task per seat" (design/08).
            AttemptState::Unknown => none("the current attempt's outcome is unknown"),
            // A terminal attempt belongs to a finished task; the seat is free.
            AttemptState::Accepted | AttemptState::Rejected | AttemptState::Abandoned => {
                assign_first(&v.ready_tasks, may_assign)
            }
        },
        None => assign_first(&v.ready_tasks, may_assign),
    }
}

/// A `None` action with a reason (design/09 asks for one).
fn none(reason: &str) -> Action {
    Action::None {
        reason: reason.to_string(),
    }
}

/// A delivery's text, with the project standard's line when the project has
/// one (T-84).
fn with_standard(text: String, standard: &Option<String>) -> String {
    match standard {
        Some(line) => format!("{text}\n{line}"),
        None => text,
    }
}

/// The first ready task, if this seat may take one at all.
fn assign_first(ready: &[TaskRow], may_assign: bool) -> Action {
    if !may_assign {
        return none("a retiring or temp seat never takes a new assignment");
    }
    match ready.first() {
        Some(task) => Action::Assign {
            task: task.id.clone(),
        },
        None => none("no ready task for this lane"),
    }
}

/// A decided delivery, ready to become a Deliver op (08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliverAction {
    pub project: String,
    pub seat: String,
    pub generation: i64,
    pub task: String,
    pub attempt: i64,
    pub text: String,
}

/// design/09 "Delivery content", exactly:
/// `Continue <task> (contract v<version>). Run `gear` first (…). Task packet: <packet>. Report progress/blocked/done with gear send.`
pub fn continue_text(task: &str, version: i64, packet: &str) -> String {
    format!(
        "Continue {task} (contract v{version}). Run `gear` first (it records that you got this). Task packet: {packet}. Report progress/blocked/done with gear send."
    )
}

/// design/09 "Delivery content", exactly:
/// `New task <task> (contract v<version>). Branch: <branch> (in your worktree). Run `gear` first (…). Task packet: <packet>. Read it before starting; report with gear send. After your result, end your turn: …`
///
/// This is the wording of a **rework** (attempt n ≥ 2): the branch and the
/// worktree are already there, so nothing is said about starting them.
pub fn assign_text(task: &str, version: i64, branch: &str, packet: &str) -> String {
    format!(
        "New task {task} (contract v{version}). Branch: {branch} (in your worktree). Run `gear` first (it records that you got this). Task packet: {packet}. Read it before starting; report with gear send. After your result, end your turn: the next task comes when you are idle."
    )
}

/// design/09 "Delivery content" for the **first attempt** of a task (attempt
/// 1), with the command that starts the branch (T-222, F-75: four lanes
/// started a task on the previous task's branch, so the candidate carried
/// another task or work landed on a handed-in branch).
///
/// `New task <task> (contract v<version>). Branch: <branch>. Start it from the integration branch: git fetch <remote> && git switch -c <branch> <remote>/<integration> (if it exists already: git switch <branch>). Run `gear` first (…). …`
///
/// The remote is the project's `[conventions].remote` (T-262): a team that
/// calls it `upstream` reads its branches from there.
pub fn first_attempt_text(
    task: &str,
    version: i64,
    branch: &str,
    remote: &str,
    integration: &str,
    packet: &str,
) -> String {
    format!(
        "New task {task} (contract v{version}). Branch: {branch}. Start it from the integration branch: git fetch {remote} && git switch -c {branch} {remote}/{integration} (if it exists already: git switch {branch}). Run `gear` first (it records that you got this). Task packet: {packet}. Read it before starting; report with gear send. After your result, end your turn: the next task comes when you are idle."
    )
}

/// The one line of a contract update (T-8), exactly as the contract puts it:
/// `T-<n> contract changed to v<k>: run `gear` and `gear --show T-<n>``.
pub fn contract_changed_text(task: &str, version: i64) -> String {
    format!("{task} contract changed to v{version}: run `gear` and `gear --show {task}`")
}

/// Build the [`LaneView`] of one lane seat from the store (read-only `tx`):
/// - `attempt`: the seat's open attempt (`open_attempts_of_seat`, first); if its
///   task is not Running/Verifying (e.g. Waiting) the view has **no attempt and
///   no ready tasks** — the continue preconditions of design/09 live here;
/// - `pending_op`: first of `slot_ops(seat)`;
/// - `last_deliver_at`: `created` of `last_op(seat, Deliver)`;
/// - `ready_tasks`: tasks of the project with `lane == seat.name`, state Ready,
///   priority ≠ `wont`, every dep Done — in rank order;
/// - `nudges_since_progress` 0; `safe_for_next` true when every open attempt
///   is handed in (Submitted): review runs in the background (never-stop);
/// - `rework_mid_turn`: the seat's agent queues input while it works (pi,
///   claude), it has no open attempt that is not Submitted, and the first ready
///   task is a rework this seat's last attempt failed (T-290).
pub fn lane_view(
    tx: &Tx<'_>,
    seat: &SeatRow,
    obs: Option<SeatObs>,
    now: Ts,
) -> GearResult<LaneView> {
    let _ = now; // the design/09 cooldown uses last_deliver_at, not this tick's time

    // The open attempt, but only while its task may still be worked on: a
    // Waiting task (or one that is not Running/Verifying) blocks both the
    // continue and any assign (design/09, R-31).
    // Never-stop (PRD §0, lesson L-44): a handed-in attempt (Submitted, its
    // candidate pushed) waits for review in the background and is a safe
    // point by definition. The attempt the lane is working on now — any open
    // one that is not Submitted — comes first; only when every open attempt is
    // handed in does the view carry a Submitted one, and then the next task
    // may be assigned.
    // T-458: an attempt whose task is in a declared wait is parked — a lane
    // that reported blocked is free for its next startable task (M5: mast sat
    // idle about 40 minutes while its attempt stayed Started). A parked
    // attempt is neither the current one nor a reason to hold the next task;
    // it comes back when its wait ends.
    let mut open = Vec::new();
    for attempt in tx.open_attempts_of_seat(&seat.id)? {
        let task = tx.get_task(&seat.project, &attempt.task)?;
        if task.as_ref().is_some_and(|task| task.wait_on.is_some()) {
            continue;
        }
        open.push(attempt);
    }
    // The newest open attempt comes first (T-458): a lane may hold a parked
    // one and a new one at once, and the new one is what it works on.
    open.sort_by_key(|a| (a.state == AttemptState::Submitted, std::cmp::Reverse(a.started)));
    // The seat has no open attempt that is not Submitted (T-290): an empty
    // list, or one that is handed in, is fine.
    let no_open_unsubmitted = open.iter().all(|a| a.state == AttemptState::Submitted);
    let safe_for_next = !open.is_empty() && no_open_unsubmitted;
    let attempt = match open.into_iter().next() {
        Some(attempt) => {
            let task = tx.get_task(&seat.project, &attempt.task)?;
            let continues = task
                .map(|task| match task.state {
                    TaskState::Running | TaskState::Verifying => true,
                    // A delivery that was never confirmed never started the
                    // task: the attempt is the seat's open one all the same,
                    // and the delivery goes again (T-97).
                    TaskState::Ready => attempt.state == AttemptState::Delivered,
                    _ => false,
                })
                .unwrap_or(false);
            if !continues {
                return Ok(empty_view(obs));
            }
            Some(attempt)
        }
        None => None,
    };

    let ready_tasks = ready_tasks_of(tx, seat)?;
    // T-290: the next packet is a rework this seat worked on before, and the
    // agent queues input typed while it works: the packet may go at once.
    let rework_mid_turn = no_open_unsubmitted
        && tx
            .get_binding(&seat.id, seat.generation)?
            .is_some_and(|binding| gear_herdr::queues_input(&binding.kind))
        && match ready_tasks.first() {
            Some(task) if task.rework_note.is_some() => tx
                .attempts_of_task(&seat.project, &task.id)?
                .last()
                .is_some_and(|last| last.seat == seat.id && last.state == AttemptState::Rejected),
            _ => false,
        };

    Ok(LaneView {
        obs,
        attempt,
        pending_op: tx.slot_ops(&seat.id)?.into_iter().next(),
        last_deliver_at: tx.last_op(&seat.id, OpKind::Deliver)?.map(|op| op.created),
        nudges_since_progress: 0,
        ready_tasks,
        safe_for_next,
        // The plan is durable in meta (T-33): a switch in progress is not
        // forgotten while the quota line is gone.
        switch_pending: tx
            .get_meta(&format!("engine.switch.{}", seat.id))?
            .is_some_and(|value| !value.is_empty())
            || tx
                .get_meta(&crate::restart::pending_meta_key(&seat.id))?
                .is_some_and(|value| !value.is_empty()),
        failed_delivery: crate::delivery::failed(tx, &seat.id)?,
        rework_mid_turn,
    })
}

/// A view with nothing to do: no attempt, no pending delivery, no ready task.
fn empty_view(obs: Option<SeatObs>) -> LaneView {
    LaneView {
        obs,
        attempt: None,
        pending_op: None,
        last_deliver_at: None,
        nudges_since_progress: 0,
        ready_tasks: Vec::new(),
        safe_for_next: false,
        switch_pending: false,
        failed_delivery: None,
        rework_mid_turn: false,
    }
}

/// This lane's Ready tasks in rank order: not `wont`, every dependency Done.
pub(crate) fn ready_tasks_of(tx: &Tx<'_>, seat: &SeatRow) -> GearResult<Vec<TaskRow>> {
    use std::collections::BTreeSet;
    let tasks = tx.tasks_of_project(&seat.project)?;
    let lane_repo = tx
        .get_project(&seat.project)?
        .and_then(|p| gear_app::config::parse(&p.config_toml).ok())
        .map(|config| config.repo_of_lane(&seat.name).path);
    // A dependency counts only when its work is merged (T-236, F-76): Done is
    // set at the review pass, before the merge reaches the integration branch.
    let mut done: BTreeSet<String> = BTreeSet::new();
    for task in tasks.iter().filter(|task| task.state == TaskState::Done) {
        if gear_app::tasks::merged_at(tx, &seat.project, &task.id)?.is_some() {
            done.insert(task.id.clone());
        }
    }
    Ok(tasks
        .into_iter()
        .filter(|task| task.lane.as_deref() == Some(seat.name.as_str()))
        .filter(|task| {
            task.repo
                .as_ref()
                .zip(lane_repo.as_ref())
                .is_none_or(|(repo, current)| repo == current)
        })
        .filter(|task| task.state == TaskState::Ready)
        .filter(|task| task.priority != "wont")
        .filter(|task| task.deps.iter().all(|dep| done.contains(dep)))
        .collect())
}

/// Where a lane reads a task packet: the CLI shows the rendered packet.
pub fn packet_ref(task: &str) -> String {
    format!("`gear --show {task}`")
}

/// The task branch a lane works on (T-253): the project's
/// `[conventions].task_branch` template with `{project}`, `{lane}` and `{n}`
/// filled in — `<lane>/t<n>` by default (17 §3).
pub fn task_branch(template: &str, project: &str, lane: &str, task: &str) -> String {
    gear_model::branch::expand(template, project, lane, task)
}

/// The project's `[conventions]`, parsed from its latest config text. A project
/// that is gone or a config that does not parse is the default conventions: a
/// branch is always named, never guessed from nothing (T-253).
pub(crate) fn conventions(tx: &Tx<'_>, project: &str) -> gear_app::config::Conventions {
    tx.get_project(project)
        .ok()
        .flatten()
        .and_then(|row| gear_app::config::parse(&row.config_toml).ok())
        .map(|config| config.conventions)
        .unwrap_or_default()
}

/// The integration branch a lane's branch starts from (T-222): the lane's own
/// repo (T-52) and its integration branch, else the project's convention. A
/// config that does not parse falls back to the default convention.
pub fn integration_branch(tx: &Tx<'_>, project: &str, lane: &str) -> String {
    let parsed = tx
        .get_project(project)
        .ok()
        .flatten()
        .and_then(|row| gear_app::config::parse(&row.config_toml).ok());
    let integration = match parsed {
        Some(config) => config.repo_of_lane(lane).integration,
        None => gear_app::config::Conventions::default().integration,
    };
    let integration = integration.trim().to_string();
    if integration.is_empty() {
        gear_app::config::Conventions::default().integration
    } else {
        integration
    }
}

/// One decision per Lane-role seat of `project` (other roles skipped), in
/// seat-id order: `lane_view(tx, seat, obs, now)` with that seat's entry from
/// `observed` (None when absent) → `decide`.
pub fn plan_dispatch(
    tx: &Tx<'_>,
    project: &str,
    observed: &[SeatObs],
    cfg: &DispatchCfg,
    now: Ts,
) -> GearResult<Vec<(SeatRow, Action)>> {
    let mut decisions = Vec::new();
    for seat in tx.seats_of_project(project)? {
        if seat.role != SeatRole::Lane {
            // The captain has no task queue (design/09).
            continue;
        }
        // The seat's entry from this tick's observation, if it was observed.
        let obs = observed.iter().find(|o| o.seat.id == seat.id).cloned();
        let view = lane_view(tx, &seat, obs, now)?;
        let action = decide(&view, cfg, now);
        let action = rebind_action(tx, &seat.id, action)?;
        let action =
            if matches!(action, Action::Assign { .. }) && tx.captain_quota_paused(project)? {
                none("captain quota: current tasks may finish; new tasks are paused")
            } else {
                action
            };
        decisions.push((seat, action));
    }
    Ok(decisions)
}

/// A pending rebind permits current work to finish, but does not start more.
/// After config changes, nothing goes to the old worktree while it is moving.
fn rebind_action(tx: &Tx<'_>, seat: &str, action: Action) -> GearResult<Action> {
    let applied = tx
        .get_meta(&format!("lane-rebind-applied:{seat}"))?
        .is_some_and(|s| !s.is_empty());
    let pending = tx
        .get_meta(&gear_app::plan::rebind_pending_key(seat))?
        .is_some_and(|s| !s.is_empty());
    let continuing_unsent = if let Action::Assign { task } = &action {
        tx.open_attempts_of_seat(seat)?
            .iter()
            .any(|a| a.task == *task && a.state == AttemptState::Delivered && a.sent_at.is_none())
    } else {
        false
    };
    Ok(if applied {
        none("lane rebind is waiting for the new worktree")
    } else if pending && matches!(action, Action::Assign { .. }) && !continuing_unsent {
        none("lane rebind is waiting for open attempts")
    } else {
        action
    })
}

/// The meta key that holds what a lane's home worktree still had
/// uncommitted before its next first attempt (T-291).
pub(crate) fn dirty_home_key(seat: &str) -> String {
    format!("engine.dirty_home.{seat}")
}

/// T-291 (M4 F-90): before a **first** attempt is created, read the lane's
/// home worktree and keep what it holds uncommitted in
/// `engine.dirty_home.<seat>` — `{"task": …, "paths": [the first five],
/// "count": n}`, else "" (clean, no worktree path, or a failed read). The git
/// call runs outside any transaction; nothing here blocks a delivery.
fn note_dirty_home(
    store: &mut Store,
    cfg: &crate::EngineCfg,
    seat: &SeatRow,
    task: &str,
) -> GearResult<()> {
    // A task that already has an attempt is a rework: its worktree story is
    // the branch it worked on, not the home worktree.
    if !store
        .read(|tx| tx.attempts_of_task(&seat.project, task))?
        .is_empty()
    {
        return Ok(());
    }
    let path = store.read(|tx| tx.get_meta(&crate::onboard::worktree_path_key(&seat.id)))?;
    let value = dirty_home_json(cfg, path.as_deref(), task);
    store.write(|tx| tx.set_meta(&dirty_home_key(&seat.id), &value))
}

/// The value `note_dirty_home` writes: the worktree's uncommitted paths, or
/// "" when it is clean or git could not read it.
fn dirty_home_json(cfg: &crate::EngineCfg, path: Option<&str>, task: &str) -> String {
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return String::new();
    };
    let Ok(status) = cfg.review_git.run(path, &["status", "--porcelain"]) else {
        return String::new();
    };
    let all: Vec<String> = status.lines().filter_map(porcelain_path).collect();
    if all.is_empty() {
        return String::new();
    }
    let count = all.len();
    let paths: Vec<&String> = all.iter().take(5).collect();
    serde_json::json!({"task": task, "paths": paths, "count": count}).to_string()
}

/// The path of one `git status --porcelain` line: the two status letters and
/// their space, then the path (a rename or copy names its new path last).
fn porcelain_path(line: &str) -> Option<String> {
    let rest = line.get(3..)?.trim();
    let path = rest.rsplit(" -> ").next().unwrap_or(rest).trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// The sentence a first packet gets when its worktree still holds earlier
/// uncommitted work (T-291), or None when the record is not for this task.
fn dirty_home_notice(value: &str, task: &str, branch: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(value).ok()?;
    if value["task"].as_str() != Some(task) {
        return None;
    }
    let count = value["count"].as_u64()?;
    let paths = value["paths"]
        .as_array()?
        .iter()
        .filter_map(|path| path.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        " Your worktree has uncommitted changes from earlier work ({count} paths: {paths}). Before you switch: commit them as a WIP commit on the branch they belong to and push it; never `git stash` (the stash is shared by every worktree of the repository) and never carry them into {branch}."
    ))
}

/// Where the first sentence of `text` that names `branch` ends, just after
/// its full stop (None when no sentence names it).
fn branch_sentence_end(text: &str, branch: &str) -> Option<usize> {
    let mut start = 0;
    for sentence in text.split_inclusive(". ") {
        let stop = start + sentence.len();
        if sentence.contains(branch) {
            return Some(if sentence.ends_with(". ") {
                stop - 1
            } else {
                stop
            });
        }
        start = stop;
    }
    None
}

/// Put `notice` right after the branch sentence of a packet, keeping the rest
/// of the text after it (T-291).
fn insert_after_branch_sentence(text: &mut String, branch: &str, notice: &str) {
    let Some(at) = branch_sentence_end(text, branch) else {
        return;
    };
    let rest = text.split_off(at);
    text.push_str(notice);
    let rest = rest.trim_start();
    if !rest.is_empty() {
        text.push(' ');
        text.push_str(rest);
    }
}

/// Create the attempt before preparing its session. An unsent attempt is
/// retained across ticks, including engine restarts, while preparation waits.
pub fn prepare_dispatch(
    store: &mut Store,
    herdr: &mut dyn gear_herdr::HerdrAct,
    cfg: &crate::EngineCfg,
    project: &str,
    planned: &[(SeatRow, Action)],
    now: Ts,
) -> GearResult<Vec<(SeatRow, Action)>> {
    let mut ready = Vec::new();
    for (seat, action) in planned {
        let task = match action {
            Action::Assign { task } | Action::Continue { task } => task,
            _ => {
                ready.push((seat.clone(), action.clone()));
                continue;
            }
        };
        // T-291 (M4 F-90): a first attempt lands in the lane's home worktree.
        // What that worktree still holds uncommitted is read now, before the
        // attempt exists, and the packet names it.
        if matches!(action, Action::Assign { .. }) {
            note_dirty_home(store, cfg, seat, task)?;
        }
        let attempt = store.write(|tx| {
            if matches!(
                rebind_action(tx, &seat.id, action.clone())?,
                Action::None { .. }
            ) {
                return Ok(None);
            }
            if let Some(existing) = tx
                .open_attempts_of_seat(&seat.id)?
                .into_iter()
                .find(|a| a.task == *task && a.state == AttemptState::Delivered)
            {
                return Ok(Some(existing));
            }
            if matches!(action, Action::Assign { .. }) {
                return gear_app::tasks::start_attempt(
                    tx,
                    project,
                    task,
                    &seat.id,
                    seat.generation,
                    now,
                )
                .map(Some);
            }
            tx.open_attempts_of_seat(&seat.id)?
                .into_iter()
                .find(|a| a.task == *task)
                .map(Some)
                .ok_or_else(|| {
                    gear_model::GearError::new(
                        gear_model::ErrorCode::NoTarget,
                        "open attempt disappeared",
                    )
                })
        })?;
        let Some(attempt) = attempt else { continue };
        if crate::session::prepare_lane(store, herdr, cfg, seat, task, attempt.n, now)?
            == crate::session::Prep::Ready
        {
            let action = if attempt.sent_at.is_none() && attempt.state == AttemptState::Delivered {
                Action::Assign { task: task.clone() }
            } else {
                action.clone()
            };
            ready.push((seat.clone(), action));
        }
    }
    Ok(ready)
}

/// Resume an open lane round after its agent disappears or is restarted.
/// Preparing never sends another task packet for an already-started attempt.
pub fn recover_open_rounds(
    store: &mut Store,
    herdr: &mut dyn gear_herdr::HerdrAct,
    cfg: &crate::EngineCfg,
    project: &str,
    observed: &[SeatObs],
    planned: &[(SeatRow, Action)],
    now: Ts,
) -> GearResult<()> {
    for obs in observed {
        let seat = &obs.seat;
        if seat.role != SeatRole::Lane
            || !matches!(seat.state, SeatState::Active | SeatState::Retiring)
        {
            continue;
        }
        if planned.iter().any(|(s, action)| {
            s.id == seat.id && matches!(action, Action::Assign { .. } | Action::Continue { .. })
        }) {
            continue;
        }
        let recovery = store.read(|tx| {
            if !tx.slot_ops(&seat.id)?.is_empty() {
                return Ok(None);
            }
            let Some(round) = tx.open_round_of_seat(&seat.id)? else {
                return Ok(None);
            };
            let Some(attempt) = tx.get_attempt(project, &round.task, round.attempt)? else {
                return Ok(None);
            };
            let unsent = attempt.state == AttemptState::Delivered && attempt.sent_at.is_none();
            if !matches!(
                attempt.state,
                AttemptState::Delivered | AttemptState::Started | AttemptState::Stopped
            ) {
                return Ok(None);
            }
            let mismatch = crate::session::round_session(tx, &round)?
                .as_ref()
                .is_some_and(|session| {
                    obs.presence == crate::observe::Presence::Missing
                        || (obs.presence == crate::observe::Presence::Present
                            && obs
                                .session
                                .as_deref()
                                .is_some_and(|current| current != session))
                });
            let running = tx
                .get_meta(&format!("engine.session_step.{}", seat.id))?
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .is_some_and(|s| {
                    let identity = format!(
                        "{}:{}:{}:{}",
                        round.project, round.task, round.attempt, round.role
                    );
                    s["round"] == identity
                        && (unsent || s["action"] == "resume")
                        && (s["generation"].as_i64() != Some(seat.generation)
                            || (s["step"] != "done" && s["step"] != "failed"))
                });
            Ok((mismatch || running).then_some((round.task, round.attempt)))
        })?;
        if let Some((task, attempt)) = recovery {
            if crate::session::prepare_lane(store, herdr, cfg, seat, &task, attempt, now)?
                == crate::session::Prep::Ready
            {
                // A restarted agent can resume the same session. It is a new
                // recipient for the failed packet even though its session id
                // is unchanged; allow one retry and clear the marker on send.
                store.write(|tx| {
                    if let Some(mut failed) = crate::delivery::failed(tx, &seat.id)? {
                        if failed.task == task && failed.attempt == attempt {
                            failed.retry_now = true;
                            crate::delivery::record(tx, &seat.id, &failed)?;
                        }
                    }
                    Ok(())
                })?;
            }
        }
    }
    Ok(())
}

/// Turn decisions into Deliver ops in the caller's transaction:
/// - Assign → `gear_app::tasks::start_attempt(tx, project, task, seat.id, seat.generation, now)`
///   then a delivery with `assign_text(task, contract_version, task_branch(template, project, seat.name, task), packet_ref(task))`;
/// - Continue → a delivery for the seat's open attempt with `continue_text(task, adopted_version, packet_ref(task))`;
/// - None / Escalate → nothing.
/// Returns the Intended ops (via `ops::intend_deliveries`).
pub fn intend_dispatch(
    tx: &Tx<'_>,
    project: &str,
    actions: &[(SeatRow, Action)],
    now: Ts,
) -> GearResult<Vec<OpRow>> {
    let mut deliveries: Vec<DeliverAction> = Vec::new();
    // T-84: the project standard travels with the assignment and the continue.
    let standard = gear_app::standard::line_for(tx, project)?;
    // The branch templates are the project's own (T-253): read them once.
    let conventions = conventions(tx, project);
    for (seat, action) in actions {
        let guarded = rebind_action(tx, &seat.id, action.clone())?;
        match &guarded {
            Action::Assign { task } => {
                if tx.captain_quota_paused(project)? {
                    continue;
                }
                // The attempt exists before the delivery that announces it.
                let attempt = match tx.open_attempts_of_seat(&seat.id)?.into_iter().find(|a| {
                    a.task == *task && a.state == AttemptState::Delivered && a.sent_at.is_none()
                }) {
                    Some(attempt) => attempt,
                    None => gear_app::tasks::start_attempt(
                        tx,
                        project,
                        task,
                        &seat.id,
                        seat.generation,
                        now,
                    )?,
                };
                // T-222 (F-75): the first attempt says how to start the branch;
                // a rework (attempt n ≥ 2) keeps the old wording — the branch
                // and the worktree are already there.
                let branch = task_branch(&conventions.task_branch, project, &seat.name, task);
                let mut text = if attempt.n == 1 {
                    first_attempt_text(
                        task,
                        attempt.contract_version,
                        &branch,
                        &conventions.remote,
                        &integration_branch(tx, project, &seat.name),
                        &packet_ref(task),
                    )
                } else {
                    assign_text(task, attempt.contract_version, &branch, &packet_ref(task))
                };
                // T-291 (M4 F-90): a first attempt into a worktree that still
                // holds earlier uncommitted work says so, right after the
                // branch sentence, and the record is spent.
                if attempt.n == 1 {
                    if let Some(value) = tx.get_meta(&dirty_home_key(&seat.id))? {
                        if let Some(notice) = dirty_home_notice(&value, task, &branch) {
                            insert_after_branch_sentence(&mut text, &branch, &notice);
                            tx.set_meta(&dirty_home_key(&seat.id), "")?;
                        }
                    }
                }
                let action = attempt.session_action.as_deref().unwrap_or("");
                // T-481: a follow-up whose session cannot be used keeps the
                // lane's own conversation instead of starting one, so the
                // pointer to the task it follows goes with the `keep` too —
                // the design's §3.3 sentence covers every case where the
                // follow-up's session is not the one the round runs in.
                if attempt.n == 1
                    && (action.starts_with("new")
                        || action == "fresh"
                        || action == "resume-fallback"
                        || action == "keep")
                {
                    if let Some(followup) = tx.get_task(project, task)?.and_then(|t| t.followup_of)
                    {
                        text.push_str(&format!("\nThis task follows {followup}; read its report: gear --show {followup}"));
                    }
                }
                deliveries.push(DeliverAction {
                    project: project.to_string(),
                    seat: seat.id.clone(),
                    generation: seat.generation,
                    task: task.clone(),
                    attempt: attempt.n,
                    text: with_standard(text, &standard),
                });
            }
            Action::Continue { task } => {
                // Continue the attempt that is actually open, never a guess.
                let Some(attempt) = tx
                    .open_attempts_of_seat(&seat.id)?
                    .into_iter()
                    .find(|attempt| attempt.task == *task)
                else {
                    continue;
                };
                deliveries.push(DeliverAction {
                    project: project.to_string(),
                    seat: seat.id.clone(),
                    generation: seat.generation,
                    task: task.clone(),
                    attempt: attempt.n,
                    text: with_standard(
                        continue_text(task, attempt.adopted_version, &packet_ref(task)),
                        &standard,
                    ),
                });
            }
            Action::Adopt { task, version } => {
                // One line for the contract update; the lane's next view adopts
                // it (T-8). The attempt is the open one, never a guess.
                let Some(attempt) = tx
                    .open_attempts_of_seat(&seat.id)?
                    .into_iter()
                    .find(|attempt| attempt.task == *task)
                else {
                    continue;
                };
                if attempt.state == AttemptState::Started {
                    // Design §10.3: an attempt that already started got its
                    // packet — the engine never delivers it a second one. The
                    // contract change goes as an instruction message, which the
                    // doorbell rings; the lane adopts the new contract at its
                    // next view (T-8).
                    contract_notice(tx, seat, task, attempt.n, *version, now)?;
                } else {
                    deliveries.push(DeliverAction {
                        project: project.to_string(),
                        seat: seat.id.clone(),
                        generation: seat.generation,
                        task: task.clone(),
                        attempt: attempt.n,
                        text: contract_changed_text(task, *version),
                    });
                }
            }
            // A seat that must wait, or one handed to the captain: no delivery.
            Action::None { .. } | Action::Escalate { .. } => {}
        }
    }
    let ops = crate::ops::intend_deliveries(tx, &deliveries, now)?;
    // The attempt is on its way again: the record of the failed delivery has
    // done its job (T-97). A second failure writes a fresh one.
    for delivery in &deliveries {
        crate::delivery::clear(tx, &delivery.seat)?;
    }
    Ok(ops)
}

/// The notice that the contract of an open attempt changed, for an attempt
/// that already started (T-8, design §10.3).
///
/// It is an instruction message, not a delivery: the doorbell rings it (at
/// once, so a working lane reads it), and the lane adopts the new contract at
/// its next view. One notice per task and version: the lane's answer closes
/// it, so the engine does not write a second one every tick.
fn contract_notice(
    tx: &Tx<'_>,
    seat: &SeatRow,
    task: &str,
    attempt: i64,
    version: i64,
    now: Ts,
) -> GearResult<bool> {
    let thread = format!("contract:{task}:{version}");
    let open = tx
        .thread_messages(&seat.project, &thread)?
        .into_iter()
        .any(|message| matches!(message.state, MsgState::Open | MsgState::Waiting));
    if open {
        return Ok(false);
    }
    tx.insert_message(&NewMessage {
        project: seat.project.clone(),
        thread,
        task: Some(task.to_string()),
        attempt: Some(attempt),
        sender: "engine".to_string(),
        sender_gen: 0,
        recipient: seat.id.clone(),
        kind: MsgKind::Instruction,
        urgent: true,
        body: contract_changed_text(task, version),
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created: now,
        meta: None,
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::Presence;
    use gear_herdr::AgentStatus;
    use gear_model::{SeatRole as Role, SeatState as State};
    use gear_store::{SeatRow, TaskRow};
    use std::collections::BTreeSet;

    const NOW: i64 = 1_800_000_000;

    fn obs(idle: bool) -> SeatObs {
        SeatObs {
            seat: SeatRow {
                id: "p:heat".into(),
                project: "p".into(),
                role: Role::Lane,
                name: "heat".into(),
                profile: "opencode".into(),
                generation: 1,
                state: State::Active,
                state_since: NOW - 1000,
                state_reason: None,
                task_id: None,
            },
            presence: Presence::Present,
            status: Some(if idle {
                AgentStatus::Idle
            } else {
                AgentStatus::Working
            }),
            flags: BTreeSet::new(),
            idle,
            screen: None,
            session: None,
        }
    }

    fn attempt(state: AttemptState) -> AttemptRow {
        AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(),
            task: "T-1".into(),
            n: 1,
            seat: "p:heat".into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state,
            candidate: None,
            report: None,
            safe_point: None,
            started: NOW - 500,
            updated: NOW - 500,
        }
    }

    fn view(attempt: Option<AttemptRow>) -> LaneView {
        LaneView {
            switch_pending: false,
            obs: Some(obs(true)),
            attempt,
            pending_op: None,
            last_deliver_at: Some(NOW - 1000),
            nudges_since_progress: 0,
            ready_tasks: vec![TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-2".into(),
                rank: 1,
                state: gear_model::TaskState::Ready,
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
                updated: NOW,
            }],
            safe_for_next: false,
            failed_delivery: None,
            rework_mid_turn: false,
        }
    }

    #[test]
    fn delivered_attempt_is_continued() {
        // Delivery confirmed but the agent never started (design/09 gap, R-31):
        // nudge it like a started attempt instead of leaving it silent.
        let cfg = DispatchCfg::default();
        let action = decide(&view(Some(attempt(AttemptState::Delivered))), &cfg, NOW);
        assert_eq!(action, Action::Continue { task: "T-1".into() });

        let mut v = view(Some(attempt(AttemptState::Delivered)));
        v.nudges_since_progress = cfg.nudge_budget;
        match decide(&v, &cfg, NOW) {
            Action::Escalate { task, reason } => {
                assert_eq!(task, "T-1");
                assert_eq!(
                    reason,
                    format!(
                        "no verifiable progress after {} continues",
                        cfg.nudge_budget
                    )
                );
            }
            other => panic!("expected an escalation, got {other:?}"),
        }
    }

    #[test]
    fn unknown_attempt_is_not_nudged() {
        // An unknown outcome is never retried (design/08).
        let action = decide(
            &view(Some(attempt(AttemptState::Unknown))),
            &DispatchCfg::default(),
            NOW,
        );
        assert!(matches!(action, Action::None { .. }));
    }
    #[test]
    fn rebind_waits_for_existing_work_and_blocks_the_old_worktree() {
        let mut store = gear_store::Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.set_meta(&gear_app::plan::rebind_pending_key("p:heat"), "pending")?;
                assert!(matches!(
                    rebind_action(tx, "p:heat", Action::Assign { task: "T-2".into() })?,
                    Action::None { .. }
                ));
                assert_eq!(
                    rebind_action(tx, "p:heat", Action::Continue { task: "T-1".into() })?,
                    Action::Continue { task: "T-1".into() }
                );
                tx.set_meta("lane-rebind-applied:p:heat", "moving")?;
                assert!(matches!(
                    rebind_action(tx, "p:heat", Action::Continue { task: "T-1".into() })?,
                    Action::None { .. }
                ));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn planned_assignment_is_rechecked_after_a_rebind_is_requested() {
        let mut store = gear_store::Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                gear_store::testing::seed_project(tx, "p", "sess", NOW)?;
                let seat = gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "heat",
                    SeatRole::Lane,
                    "w1:p1",
                    "opencode",
                    NOW,
                )?;
                tx.set_meta("lane-rebind-applied:p:heat", "moving")?;
                let actions = vec![(seat, Action::Assign { task: "T-1".into() })];
                assert!(intend_dispatch(tx, "p", &actions, NOW)?.is_empty());
                Ok(())
            })
            .unwrap();
    }

    /// A config the engine can read: the project, its quota, its captain and
    /// its conventions.
    fn config_toml(integration: &str) -> String {
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/repo\"\n\
             [quota]\nlanes = 4\ntemps = 2\n\
             [captain]\npane = \"w1:p0\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             [conventions]\nintegration = \"{integration}\"\n"
        )
    }

    /// A ready task of the lane `heat`.
    fn ready_task(id: &str) -> TaskRow {
        TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: id.into(),
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
            updated: NOW,
        }
    }

    /// The text of the delivery the engine intends for the lane.
    fn assign_delivery_text(store: &mut gear_store::Store, task: &str, attempt: i64) -> String {
        store
            .write(|tx| {
                let actions = vec![(
                    tx.get_seat("p:heat")?.unwrap(),
                    Action::Assign {
                        task: task.to_string(),
                    },
                )];
                let ops = intend_dispatch(tx, "p", &actions, NOW)?;
                assert_eq!(ops.len(), 1, "one delivery");
                assert_eq!(
                    ops[0].attempt,
                    Some(attempt),
                    "the attempt the delivery is for"
                );
                Ok(ops[0].payload["text"].as_str().unwrap().to_string())
            })
            .unwrap()
    }

    /// T-222 (F-75): the first packet of a task says how to start its branch
    /// from the integration branch.
    #[test]
    fn a_first_attempt_says_how_to_start_the_branch() {
        let mut store = gear_store::Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                let mut project = gear_store::testing::seed_project(tx, "p", "sess", NOW)?;
                project.config_toml = config_toml("trunk");
                tx.update_project(&project)?;
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "heat",
                    SeatRole::Lane,
                    "w1:p2",
                    "opencode",
                    NOW,
                )?;
                tx.insert_task(&ready_task("T-1"))?;
                Ok(())
            })
            .unwrap();
        let text = assign_delivery_text(&mut store, "T-1", 1);
        assert!(
            text.contains(
                "Branch: heat/t1. Start it from the integration branch: git fetch origin && git switch -c heat/t1 origin/trunk (if it exists already: git switch heat/t1)."
            ),
            "{text}"
        );
    }

    /// A rework (attempt n ≥ 2) keeps the old wording: the branch and the
    /// worktree are already there.
    #[test]
    fn a_rework_keeps_the_old_branch_line() {
        let mut store = gear_store::Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                let mut project = gear_store::testing::seed_project(tx, "p", "sess", NOW)?;
                project.config_toml = config_toml("trunk");
                tx.update_project(&project)?;
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "heat",
                    SeatRole::Lane,
                    "w1:p2",
                    "opencode",
                    NOW,
                )?;
                tx.insert_task(&ready_task("T-1"))?;
                tx.insert_attempt(&AttemptRow {
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                    project: "p".into(),
                    task: "T-1".into(),
                    n: 2,
                    seat: "p:heat".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: AttemptState::Delivered,
                    candidate: None,
                    report: None,
                    safe_point: None,
                    started: NOW,
                    updated: NOW,
                })?;
                Ok(())
            })
            .unwrap();
        let text = assign_delivery_text(&mut store, "T-1", 2);
        assert!(
            text.contains("Branch: heat/t1 (in your worktree)."),
            "{text}"
        );
        assert!(
            !text.contains("git switch"),
            "a rework does not start a branch: {text}"
        );
    }

    /// The lane's own repo names its integration branch (T-52).
    #[test]
    fn the_lane_repo_names_the_integration_branch() {
        let mut store = gear_store::Store::open_in_memory().unwrap();
        store.write(|tx| {
            let mut project = gear_store::testing::seed_project(tx, "p", "sess", NOW)?;
            project.config_toml = format!(
                "{}\n[[repos]]\nname = \"web\"\npath = \"web\"\nintegration = \"web-dev\"\n\
                 [[lanes]]\nname = \"heat\"\nduty = \"d\"\nowns = []\nkind = \"opencode\"\nprofile = \"opencode\"\nrepo = \"web\"\n",
                config_toml("trunk")
            );
            tx.update_project(&project)?;
            gear_store::testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", NOW)?;
            tx.insert_task(&ready_task("T-1"))?;
            Ok(())
        })
        .unwrap();
        let text = assign_delivery_text(&mut store, "T-1", 1);
        assert!(
            text.contains("git switch -c heat/t1 origin/web-dev"),
            "{text}"
        );
    }

    /// The exact wording of the two packets.
    #[test]
    fn the_two_packet_writings() {
        assert_eq!(
            first_attempt_text("T-1", 1, "heat/t1", "origin", "dev", "`gear --show T-1`"),
            "New task T-1 (contract v1). Branch: heat/t1. Start it from the integration branch: git fetch origin && git switch -c heat/t1 origin/dev (if it exists already: git switch heat/t1). Run `gear` first (it records that you got this). Task packet: `gear --show T-1`. Read it before starting; report with gear send. After your result, end your turn: the next task comes when you are idle."
        );
        assert_eq!(
            assign_text("T-1", 1, "heat/t1", "`gear --show T-1`"),
            "New task T-1 (contract v1). Branch: heat/t1 (in your worktree). Run `gear` first (it records that you got this). Task packet: `gear --show T-1`. Read it before starting; report with gear send. After your result, end your turn: the next task comes when you are idle."
        );
    }

    // ----------------------------------------------------------------
    // T-291 (M4 F-90): a first packet names what its home worktree still
    // holds uncommitted.
    // ----------------------------------------------------------------

    /// One temp git repo for the module, as the lane's home worktree: `src/a.rs`
    /// is committed and clean on entry. The tests below share it, so each takes
    /// [`home_lock`] and resets it first.
    fn home() -> &'static std::path::Path {
        static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        HOME.get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("src")).unwrap();
            std::fs::write(dir.path().join("src/a.rs"), "fn a() {}\n").unwrap();
            sh(
                dir.path(),
                "git init -q -b work && git add -A && \
                 git -c user.email=gear@example.invalid -c user.name=gear commit -qm init",
            );
            dir
        })
        .path()
    }

    fn home_path() -> String {
        home().to_str().unwrap().to_string()
    }

    /// The module's tests share the one repo: one at a time, clean on entry.
    fn clean_home() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        sh(home(), "git checkout -q -- . && git clean -qfdx");
        guard
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

    /// The lane `heat` ready for the first assignment of `T-1`, with `path` as
    /// its home worktree.
    fn home_world(
        path: &str,
    ) -> (
        gear_store::Store,
        gear_herdr::fake::FakeHerdr,
        crate::EngineCfg,
    ) {
        let mut store = gear_store::Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                let mut project = gear_store::testing::seed_project(tx, "p", "sess", NOW)?;
                project.config_toml = config_toml("trunk");
                tx.update_project(&project)?;
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "heat",
                    SeatRole::Lane,
                    "w1:p2",
                    "opencode",
                    NOW,
                )?;
                tx.set_meta(&crate::onboard::worktree_path_key("p:heat"), path)?;
                // The agent is up with a fresh session: the packet needs no
                // pane work, so the test reads its text at once.
                tx.set_meta(&format!("engine.session_fresh.{}", "p:heat"), "1")?;
                tx.insert_task(&ready_task("T-1"))?;
                Ok(())
            })
            .unwrap();
        let mut h = gear_herdr::fake::FakeHerdr::new();
        h.add_agent_named("w1:p2", "opencode", gear_herdr::AgentStatus::Idle, "p-heat");
        let cfg = crate::EngineCfg {
            profiles: gear_herdr::builtins(),
            ..Default::default()
        };
        (store, h, cfg)
    }

    /// plan → (prepare, intend) → the text of the one delivery it makes.
    fn first_packet(
        store: &mut gear_store::Store,
        h: &mut gear_herdr::fake::FakeHerdr,
        cfg: &crate::EngineCfg,
        task: &str,
    ) -> String {
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let actions = vec![(
            seat,
            Action::Assign {
                task: task.to_string(),
            },
        )];
        let prepared = prepare_dispatch(store, h, cfg, "p", &actions, NOW).unwrap();
        assert_eq!(prepared.len(), 1, "the packet is ready");
        let ops = store
            .write(|tx| intend_dispatch(tx, "p", &prepared, NOW))
            .unwrap();
        assert_eq!(ops.len(), 1, "one delivery");
        ops[0].payload["text"].as_str().unwrap().to_string()
    }

    /// A dirty home worktree: the first packet names its paths, says how to
    /// deal with them, and the record is spent.
    #[test]
    fn a_dirty_home_worktree_is_named_in_the_first_packet() {
        let _home = clean_home();
        std::fs::write(home().join("src/a.rs"), "fn a() { let _ = 1; }\n").unwrap();
        let (mut store, mut h, cfg) = home_world(&home_path());
        let text = first_packet(&mut store, &mut h, &cfg, "T-1");
        assert!(
            text.contains(
                "Your worktree has uncommitted changes from earlier work (1 paths: src/a.rs)"
            ),
            "{text}"
        );
        assert!(text.contains("never `git stash`"), "{text}");
        assert!(text.contains("never carry them into heat/t1"), "{text}");
        // Right after the sentence that names the branch, before the command
        // that starts it.
        let branch = "Branch: heat/t1.";
        let at = text.find(branch).unwrap();
        assert!(
            text[at + branch.len()..].starts_with(" Your worktree has uncommitted changes"),
            "{text}"
        );
        // The command that starts the branch is still there, after the notice.
        assert!(
            text.contains("heat/t1. Start it from the integration branch: git fetch origin"),
            "{text}"
        );
        // The record is spent: a rework check of the same meta finds nothing.
        assert_eq!(
            store
                .read(|tx| tx.get_meta(&dirty_home_key("p:heat")))
                .unwrap()
                .as_deref(),
            Some("")
        );
    }

    /// A clean worktree gives today's text exactly.
    #[test]
    fn a_clean_home_worktree_gives_the_usual_text() {
        let _home = clean_home();
        let (mut store, mut h, cfg) = home_world(&home_path());
        let text = first_packet(&mut store, &mut h, &cfg, "T-1");
        assert_eq!(
            text,
            first_attempt_text("T-1", 1, "heat/t1", "origin", "trunk", "`gear --show T-1`")
        );
    }

    /// A rework (attempt 2) never carries the line, even when a record for the
    /// task is still there.
    #[test]
    fn a_rework_never_carries_the_dirty_home_line() {
        let _home = clean_home();
        std::fs::write(home().join("src/a.rs"), "fn a() { let _ = 2; }\n").unwrap();
        let (mut store, mut h, cfg) = home_world(&home_path());
        store
            .write(|tx| {
                tx.set_meta(
                    &dirty_home_key("p:heat"),
                    &serde_json::json!({"task": "T-1", "paths": ["src/a.rs"], "count": 1})
                        .to_string(),
                )?;
                tx.insert_attempt(&AttemptRow {
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                    project: "p".into(),
                    task: "T-1".into(),
                    n: 2,
                    seat: "p:heat".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: AttemptState::Delivered,
                    candidate: None,
                    report: None,
                    safe_point: None,
                    started: NOW,
                    updated: NOW,
                })
            })
            .unwrap();
        let text = first_packet(&mut store, &mut h, &cfg, "T-1");
        assert!(
            !text.contains("uncommitted changes from earlier work"),
            "{text}"
        );
        assert!(!text.contains("git stash"), "{text}");
    }

    /// A worktree git cannot read is a clean one: nothing to say, nothing
    /// blocks the delivery.
    #[test]
    fn a_failed_read_says_nothing() {
        let (mut store, mut h, cfg) = home_world("/nonexistent/gear/t291");
        let text = first_packet(&mut store, &mut h, &cfg, "T-1");
        assert_eq!(
            text,
            first_attempt_text("T-1", 1, "heat/t1", "origin", "trunk", "`gear --show T-1`")
        );
        assert_eq!(
            store
                .read(|tx| tx.get_meta(&dirty_home_key("p:heat")))
                .unwrap()
                .as_deref(),
            Some("")
        );
    }
}
