//! The GEAR plan board (PRD §15).
//!
//! The board shows the plan, lane queues, waiting items and temp slots.
//! Lane summaries name the observed Herdr agent state separately from task
//! state. Observation is read-only and happens outside store transactions;
//! unavailable observations are shown as unknown. The board never writes.
//!
//! The board does not start the engine either: it reads the engine's lock
//! file to know whether the engine is down. A view that revived what it
//! reports on could never show that the engine is down; the next `gear`
//! call starts it (PRD §16).
//!
//! This crate is a library. It builds the board from the store and draws it
//! on a terminal; `gear-cli` calls it from the `gear board` subcommand.

mod deps;
pub mod frame;
pub mod graph;
pub mod idle;
pub mod style;
mod screen;
pub mod voyage;

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use gear_model::{ErrorCode, GearError, GearResult, OpKind, OpState, SeatRole, SeatState, TaskState, Ts};
use gear_store::{BindingStatus, SeatRow, Store, Tx};

use crate::frame::Tab;
use crate::idle::IdleReason;
use gear_herdr::{AgentInfo, AgentStatus, HerdrRead};

pub use deps::DepNode;
pub use screen::{Event, Keys, Terminal};

/// How long a frame stays on the screen before the board redraws it.
pub const REFRESH: Duration = Duration::from_secs(2);

/// The lane name of a task that belongs to no lane.
const UNASSIGNED: &str = "unassigned";

/// The one warning line the board may print.
const ENGINE_DOWN: &str = "! engine down — the next gear call starts it";

/// What the board shows in the middle: the queues of the lanes, the open
/// tasks as a dependency tree (PRD §15), or the graph of the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// One block per lane, with its queue.
    Lanes,
    /// The open tasks as a dependency tree.
    Deps,
    /// The work as a graph: dev, the reviews and one column per lane.
    Graph,
}

impl View {
    /// The other view: the key `d` toggles between them.
    pub fn toggled(self) -> View {
        match self {
            View::Lanes => View::Deps,
            View::Deps => View::Graph,
            View::Graph => View::Lanes,
        }
    }
}

/// How the board is built and shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The project to show; `None` shows every project in the store.
    pub project: Option<String>,
    /// The database path. The engine's lock file sits next to it, so the
    /// board can tell whether the engine is down.
    pub db: Option<PathBuf>,
    /// Print the board as JSON, once — the machine-readable shape of the
    /// same view.
    pub json: bool,
    /// What the board shows in the middle.
    pub view: View,
    /// How long a frame stays on the screen.
    pub refresh: Duration,
    /// Draw in colour. `run` turns it off when `NO_COLOR` is set; the JSON
    /// and the text printed once (no terminal) never carry colour.
    pub color: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            project: None,
            db: None,
            json: false,
            view: View::Lanes,
            refresh: REFRESH,
            color: true,
        }
    }
}

/// The board: the plan of every project that is shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Board {
    /// The engine's liveness; the board warns only when it is down.
    pub engine: EngineState,
    /// One block per project.
    pub projects: Vec<ProjectBoard>,
}

/// The plan of one project.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectBoard {
    /// The same voyage measurements shown first on the captain home page.
    pub voyages: Vec<voyage::VoyageReport>,
    /// The voyage the Workload view measures: the active one, else the latest
    /// departed voyage that is finished (its measurements are still true).
    pub workload: Option<voyage::VoyageReport>,
    pub project: String,
    pub captain_quota: Option<CaptainQuotaBoard>,
    pub totals: Totals,
    /// The lanes that have work, in seat order; a lane a task names but no
    /// seat carries comes last.
    pub lanes: Vec<LaneBoard>,
    /// The open tasks as a dependency tree.
    pub deps: Vec<DepNode>,
    /// The work as a graph: the tasks with their stage, in the order the
    /// graph draws them (design §8.1).
    pub graph: Vec<graph::GraphTask>,
    /// The lanes of the graph, one column each.
    pub graph_lanes: Vec<String>,
    /// The items waiting for the captain.
    pub waiting: Waiting,
    /// The temp slots of the project.
    pub temps: Temps,
    /// The deliveries the engine has not confirmed yet: the captain's home
    /// page shows them, because a delivery that is not confirmed is a lane
    /// that may not have started.
    pub unconfirmed_deliveries: usize,
    /// The quota readings of the providers (§9.1).
    pub quota: Vec<QuotaReading>,
    /// The captain's decisions of this voyage (§7.1): the log the board
    /// shows in its fourth view.
    pub decisions: Vec<DecisionBoard>,
    /// The friction reports the agents left (§6.1), open and closed.
    pub frictions: Vec<FrictionBoard>,
    /// The tasks that overlap the integration branch of their repo (T-188):
    /// information lines, never items that wait for the captain.
    pub overlaps: Vec<Overlap>,
}

/// One task that overlaps the integration branch of its repo (T-188).
///
/// The overlap is an information line: the lane merges the integration branch
/// in and the merge gate catches a conflict. It never waits for the captain.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Overlap {
    /// The task that overlaps the integration branch.
    pub task: String,
    /// The repo the task is in; empty for the project root.
    pub repo: String,
    /// The integration branch the task overlaps.
    pub integration: String,
    /// The files the task and the integration branch share.
    pub files: Vec<String>,
}

/// One decision of the captain's log, as the board shows it (§7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionBoard {
    /// When the decision was taken.
    pub at: String,
    /// The kind of the decision (`scope`, `cut`, `override`…).
    pub kind: String,
    /// What was decided.
    pub what: String,
    /// Why it was decided.
    pub why: Option<String>,
    /// The decision is worth a look at the landing (§7.2).
    pub star: bool,
}

/// One friction report of an agent, as the board shows it (§6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrictionBoard {
    /// The task it was reported for.
    pub task: String,
    /// The lane that reported it.
    pub lane: String,
    /// What the report says.
    pub body: String,
    /// The captain closed it.
    pub closed: bool,
    /// What the captain did about it.
    pub outcome: Option<String>,
}

/// A captain quota pause, as shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptainQuotaBoard {
    pub reset_at: Ts,
    pub remaining_s: i64,
    pub awaiting_view: bool,
}

/// The counts of the whole plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub tasks: usize,
    pub done: usize,
    pub in_progress: usize,
    pub waiting: usize,
    /// The attempts of the whole project that reached a hand-in.
    pub handed_in: usize,
    /// The hand-ins a review sent back.
    pub rejected: usize,
}

/// How well never-stop is going for one lane (docs/field-notes/decomposition.md):
/// how often its hand-ins come back, and how long since it last got a task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rework {
    /// The attempts that reached a hand-in: submitted, accepted or rejected.
    pub handed_in: usize,
    /// The hand-ins a review sent back.
    pub rejected: usize,
    /// The age of the lane's last Deliver op; `None` when it never got one.
    pub last_task_age_s: Option<i64>,
}

/// One lane and its queue, in plan order.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneBoard {
    pub lane: String,
    /// Herdr observation, never inferred from task or seat state.
    pub agent_state: &'static str,
    pub tasks: Vec<TaskBoard>,
    /// The lane's never-stop measures.
    pub rework: Rework,
    /// Set when the seat is not running (T-82): its state, the reason the
    /// engine recorded and the plan line that acts on it — instead of an
    /// `agent unknown` nobody can do anything with (trial G02, G04).
    pub down: Option<DownSeat>,
    /// Why the lane is idle (design §8.2); None while it works.
    pub idle: Option<IdleReason>,
    /// The text of the reason (§8.2).
    pub idle_text: Option<String>,
    /// The tasks the lane has finished.
    pub done_count: usize,
    /// The tasks the lane still has.
    pub left_count: usize,
    /// The minutes of estimate the lane has left.
    pub left_min: i64,
}

/// A quota reading of a provider (§9.1): the windows it reports and when
/// the reading was taken.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaReading {
    pub provider: String,
    pub windows: Vec<QuotaWindow>,
    pub read_at: Ts,
}

/// One window of a provider's quota (§9.1).
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaWindow {
    pub name: String,
    pub used_percent: f64,
    pub resets_at: Option<Ts>,
}

/// A seat that is not running, what it said about it, and the way back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownSeat {
    /// The seat state in words: `needs check`, `unknown`, `parked`, …
    pub state: String,
    /// The reason the seat recorded (the engine's step failure, a park, …).
    pub reason: String,
    /// The plan `# Staff` line that acts on it, when there is one:
    /// `- retry heat`.
    pub action: Option<String>,
}

/// One task of a lane's queue.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskBoard {
    pub id: String,
    pub state: TaskState,
    /// The goal of the task (its title in the frame).
    pub title: String,
    /// The estimate of the task in minutes.
    pub est_min: Option<i64>,
    /// The time the task took, in minutes.
    pub actual_min: Option<i64>,
    /// How many rounds the task had.
    pub rounds: usize,
    /// How many rounds the task has in the rounds table (§4.1): a dev round
    /// and a review round are two rounds of one attempt.
    pub stored_rounds: usize,
    /// The tokens of the task's rounds, when they are counted (§4.3).
    pub tokens: Option<i64>,
    /// The cost of the task's rounds, when it is known (§4.4).
    pub cost_usd: Option<f64>,
}

/// The captain's inbox: how many items wait, and how long the oldest has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Waiting {
    pub count: usize,
    /// The age of the oldest waiting item in seconds; `None` when nothing
    /// waits.
    pub oldest_age_s: Option<i64>,
}

/// The temp slots of the project: in use and the quota.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Temps {
    pub in_use: usize,
    pub quota: usize,
    /// Temp seats running a job (active, not blocked).
    pub running: usize,
    /// Temp seats queued or starting.
    pub starting: usize,
    /// Temp seats whose agent waits on a dialog (the engine's blocked timer).
    pub blocked: usize,
}

/// Whether the engine is running: it holds its lock file while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineState {
    Running,
    Down,
}

impl EngineState {
    /// Probe the engine's lock file, which sits next to the database
    /// (design/08). The lock is free when the engine is down.
    pub fn probe(db: Option<&std::path::Path>) -> EngineState {
        let Some(dir) = db.and_then(|db| db.parent()) else {
            return EngineState::Down;
        };
        // Opened without creating it: no lock file means the engine never ran.
        let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join("engine.lock"))
        else {
            return EngineState::Down;
        };
        match file.try_lock() {
            // The lock is free: nobody holds the engine.
            Ok(()) => EngineState::Down,
            Err(std::fs::TryLockError::WouldBlock) => EngineState::Running,
            Err(std::fs::TryLockError::Error(_)) => EngineState::Down,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            EngineState::Running => "running",
            EngineState::Down => "down",
        }
    }
}

/// Build the board from the store. Read-only: nothing is written.
pub fn build(store: &mut Store, opts: &Options, now: Ts) -> GearResult<Board> {
    build_with_agents(store, opts, now, None)
}

/// Build from an observation obtained before opening the read transaction.
/// None means observation failed; an empty slice means no agents were seen.
pub fn build_with_agents(
    store: &mut Store, opts: &Options, now: Ts, agents: Option<&[AgentInfo]>,
) -> GearResult<Board> {
    let engine = EngineState::probe(opts.db.as_deref());
    let projects = store.read(|tx| {
        let ids = match &opts.project {
            Some(id) => vec![id.clone()],
            None => tx.project_ids()?,
        };
        ids.iter().map(|id| project_with_agents(tx, id, now, agents)).collect()
    })?;
    Ok(Board { engine, projects })
}

/// The plan of one project: the model both the board and the captain's home
/// page show. One read transaction; nothing is written.
pub fn project(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<ProjectBoard> {
    project_with_agents(tx, project, now, None)
}

/// The shared project model with an optional read-only Herdr snapshot.
pub fn project_with_agents(
    tx: &Tx<'_>, project: &str, now: Ts, agents: Option<&[AgentInfo]>,
) -> GearResult<ProjectBoard> {
    let row = tx.get_project(project)?.ok_or_else(|| {
        GearError::new(ErrorCode::NoTarget, format!("project {project} is not registered"))
    })?;
    let seats = tx.seats_of_project(project)?;
    let tasks = tx.tasks_of_project(project)?;
    // The seats whose provider is in a back-off (T-75): the engine recorded
    // the wait, and the board says so instead of "working".
    let backoff = tx.seats_in_backoff(project)?;

    // Keep active empty lanes visible, but never recreate a retired lane
    // from a historical task assignment below.
    let retired: std::collections::BTreeSet<_> = seats.iter()
        .filter(|seat| seat.role == SeatRole::Lane && seat.state == SeatState::Retired)
        .map(|seat| seat.name.as_str()).collect();
    let mut lanes = Vec::new();
    for seat in seats.iter().filter(|seat| seat.role == SeatRole::Lane && seat.state != SeatState::Retired) {
        lanes.push(LaneBoard {
            lane: seat.name.clone(),
            agent_state: observed_agent_state(tx, seat, agents, &backoff)?,
            tasks: Vec::new(),
            rework: Rework::default(),
            down: down_seat(seat),
            idle: None,
            idle_text: None,
            done_count: 0,
            left_count: 0,
            left_min: 0,
        });
    }

    let mut totals = Totals::default();
    for task in &tasks {
        // A draft task is not in the plan yet; the board shows the plan.
        if task.state == TaskState::Draft {
            continue;
        }
        totals.tasks += 1;
        match task.state {
            TaskState::Done => totals.done += 1,
            TaskState::Running => totals.in_progress += 1,
            TaskState::Waiting => totals.waiting += 1,
            _ => {}
        }
        let name = task.lane.clone().unwrap_or_else(|| UNASSIGNED.to_string());
        if retired.contains(name.as_str()) {
            continue;
        }
        let line = TaskBoard {
            id: task.id.clone(),
            state: task.state,
            title: tx
                .get_contract(project, &task.id, task.contract_version)?
                .map(|contract| contract.goal)
                .unwrap_or_default(),
            est_min: task.estimate_min,
            actual_min: actual_min_of(tx, project, &task.id, now)?,
            rounds: tx.attempts_of_task(project, &task.id)?.len(),
            stored_rounds: tx.rounds_of_task(project, &task.id)?.len(),
            tokens: tokens_of(tx, project, &task.id)?,
            cost_usd: cost_of(tx, project, &task.id)?,
        };
        match lanes.iter_mut().find(|lane| lane.lane == name) {
            Some(lane) => lane.tasks.push(line),
            None => lanes.push(LaneBoard {
                lane: name,
                agent_state: "unknown",
                tasks: vec![line],
                rework: Rework::default(),
                down: None,
                idle: None,
                idle_text: None,
                done_count: 0,
                left_count: 0,
                left_min: 0,
            }),
        }
    }
    // Every registered lane stays in the model, even with nothing queued: an
    // empty lane is exactly what the captain has to see to refill it (the
    // review of T-9).

    // The never-stop measures: per lane, the hand-ins a review sent back and
    // the time since the lane last received a task.
    //
    // An attempt belongs to the seat that made it, not to the lane its task
    // sits on now: tasks move between lanes (the review of T-16).
    let mut attempts: Vec<(String, gear_model::AttemptState)> = Vec::new();
    for task in &tasks {
        for attempt in tx.attempts_of_task(project, &task.id)? {
            attempts.push((attempt.seat.clone(), attempt.state));
        }
    }
    // The project totals: every attempt of the project, whatever its seat —
    // a temp worker's hand-in counts too (F2 of the T-16 review).
    for (_, state) in &attempts {
        match state {
            gear_model::AttemptState::Rejected => {
                totals.rejected += 1;
                totals.handed_in += 1;
            }
            gear_model::AttemptState::Submitted | gear_model::AttemptState::Accepted => {
                totals.handed_in += 1;
            }
            _ => {}
        }
    }
    for lane in &mut lanes {
        let seat_id = format!("{project}:{}", lane.lane);
        let mut handed_in = 0;
        let mut rejected = 0;
        for (seat, state) in &attempts {
            if *seat != seat_id {
                continue;
            }
            match state {
                gear_model::AttemptState::Rejected => {
                    rejected += 1;
                    handed_in += 1;
                }
                gear_model::AttemptState::Submitted | gear_model::AttemptState::Accepted => {
                    handed_in += 1;
                }
                _ => {}
            }
        }
        let last = tx.last_op(&format!("{project}:{}", lane.lane), OpKind::Deliver)?;
        lane.rework = Rework {
            handed_in,
            rejected,
            last_task_age_s: last.map(|op| (now - op.created).max(0)),
        };
    }

    // The open tasks as a dependency tree (the other view of the plan).
    let deps = deps::tree(&tasks);

    // Why each lane is idle (design §8.2) and what it has left.
    for lane in &mut lanes {
        let reason = idle::idle_reason(tx, project, &lane.lane, now)?;
        lane.idle_text = reason.as_ref().map(|reason| idle::describe(reason, tx, project));
        lane.idle = reason;
        lane.done_count = lane
            .tasks
            .iter()
            .filter(|task| task.state == TaskState::Done)
            .count();
        lane.left_count = lane
            .tasks
            .iter()
            .filter(|task| !task.state.is_terminal())
            .count();
        lane.left_min = lane
            .tasks
            .iter()
            .filter(|task| !task.state.is_terminal())
            .filter_map(|task| task.est_min)
            .sum();
    }

    // The work as a graph (design §8.1).
    let graph = graph::tasks(tx, project, now)?;
    let graph_lanes = graph::lanes(tx, project)?;

    // The captain's inbox: the items that wait for an answer.
    let waiting = match tx.captain_of(project)? {
        Some(captain) => {
            let items = tx.actionable_for(project, &captain.id, now)?;
            let oldest = items.iter().map(|item| item.created).min();
            Waiting {
                count: items.len(),
                oldest_age_s: oldest.map(|created| (now - created).max(0)),
            }
        }
        None => Waiting::default(),
    };

    // T-69: the temp slots are shown as what they are doing — running,
    // starting, blocked — not as one number. A temp seat is `blocked` while
    // the engine's blocked timer for it is live (its agent waits on a dialog).
    let temps_seats: Vec<&SeatRow> = seats
        .iter()
        .filter(|seat| seat.role == SeatRole::Temp)
        .collect();
    let blocked_seat = |seat: &SeatRow| {
        tx.get_meta(&format!("seat-blocked:{}:since", seat.id))
            .ok()
            .flatten()
            .is_some_and(|since| !since.is_empty())
    };
    let temps = Temps {
        in_use: temps_seats
            .iter()
            .filter(|seat| holds_temp_slot(seat.state))
            .count(),
        quota: row.quota_temps.max(0) as usize,
        running: temps_seats
            .iter()
            .filter(|seat| matches!(seat.state, SeatState::Active) && !blocked_seat(seat))
            .count(),
        starting: temps_seats
            .iter()
            .filter(|seat| matches!(seat.state, SeatState::Queued | SeatState::Starting))
            .count(),
        blocked: temps_seats.iter().filter(|seat| blocked_seat(seat)).count(),
    };

    // The deliveries in flight: a Deliver op that is not settled yet.
    let mut unconfirmed_deliveries = 0;
    for state in [
        OpState::Intended,
        OpState::Executing,
        OpState::Submitted,
        OpState::Unknown,
    ] {
        unconfirmed_deliveries += tx
            .ops_in_state(project, state)?
            .iter()
            .filter(|op| op.kind == OpKind::Deliver)
            .count();
    }

    let voyages = voyage::reports(tx, project, now)?;
    let workload = workload_voyage(tx, project, now)?;
    // The decisions belong to the voyage the board measures (F3).
    let decisions = decisions_of(
        tx,
        project,
        workload.as_ref().map(|voyage| voyage.milestone.as_str()),
    )?;
    Ok(ProjectBoard {
        voyages,
        workload,
        project: project.to_string(),
        captain_quota: tx.captain_quota(project)?.filter(|state| !state.resumed).map(|state| CaptainQuotaBoard {
            reset_at: state.reset_at, remaining_s: (state.reset_at - now).max(0),
            awaiting_view: state.wake_op.is_some(),
        }),
        totals,
        lanes,
        deps,
        graph,
        graph_lanes,
        quota: quota_readings(tx)?,
        decisions,
        frictions: frictions_of(tx, project)?,
        // The overlaps with the integration branch (T-188): information
        // lines, never items that wait for the captain.
        overlaps: integration_overlaps_of(tx, project)?,
        waiting,
        temps,
        unconfirmed_deliveries,
    })
}

/// The overlaps of the project with the integration branches (T-188).
///
/// The engine's scan writes the meta keys
/// `parallel:<project>[:<repo>]:integration:<task>`; the value holds the files
/// the task and the integration branch share. A cleared key (an empty value)
/// shows nothing.
pub fn integration_overlaps(store: &mut Store, project: &str) -> GearResult<Vec<Overlap>> {
    store.read(|tx| integration_overlaps_of(tx, project))
}

/// The overlaps of the project, read inside an open transaction.
pub fn integration_overlaps_of(tx: &Tx<'_>, project: &str) -> GearResult<Vec<Overlap>> {
    let prefix = format!("parallel:{project}:");
    let mut overlaps = Vec::new();
    for key in tx.meta_keys_with_prefix(&prefix)? {
        let Some(rest) = key.strip_prefix(&prefix) else { continue };
        let Some((repo, task)) = split_overlap_key(rest) else { continue };
        let Some(raw) = tx.get_meta(&key)? else { continue };
        // A cleared key shows nothing: the scan clears it when the overlap
        // is gone.
        if raw.trim().is_empty() {
            continue;
        }
        let (files, integration) = overlap_value(&raw);
        if files.is_empty() {
            continue;
        }
        overlaps.push(Overlap {
            task,
            repo,
            integration: integration.unwrap_or_default(),
            files,
        });
    }
    overlaps.sort_by(|left, right| {
        (&left.task, &left.repo).cmp(&(&right.task, &right.repo))
    });
    Ok(overlaps)
}

/// The repo and the task of one overlap key: `integration:<task>` for the
/// project root, `<repo>:integration:<task>` for a child repo.
///
/// Only that exact shape is an overlap (F1): the bookkeeping keys of the
/// scan — `pair:<pair>:files`, `pair:<pair>:item` — are not overlaps, and
/// the task is a task id, not the rest of the key.
fn split_overlap_key(rest: &str) -> Option<(String, String)> {
    // A repo named `integration` is the one case where the key has two
    // `integration` segments.
    if let Some(task) = rest.strip_prefix("integration:integration:") {
        return task_id(task).map(|task| ("integration".to_string(), task));
    }
    let (repo, tail) = match rest.split_once(':') {
        Some((repo, tail)) if repo != "integration" => (repo.to_string(), tail),
        _ => (String::new(), rest),
    };
    let task = tail.strip_prefix("integration:")?;
    // `pair` is the namespace of the scan's bookkeeping, not a repo.
    if repo == "pair" {
        return None;
    }
    task_id(task).map(|task| (repo, task))
}

/// A task id: one segment, not empty. The suffixes of the bookkeeping keys
/// (`:files`, `:item`, `:notified`) are not part of a task id.
fn task_id(task: &str) -> Option<String> {
    if task.is_empty() || task.contains(':') {
        return None;
    }
    Some(task.to_string())
}

/// The files and the integration branch of one overlap value.
///
/// The scan writes the files and the branch as a JSON object; a value that
/// only holds the files is read as the files it names.
fn overlap_value(raw: &str) -> (Vec<String>, Option<String>) {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Object(_)) => {
            let value: serde_json::Value = serde_json::from_str(raw).unwrap_or_default();
            let files = value
                .get("files")
                .and_then(serde_json::Value::as_array)
                .map(|files| files.iter().filter_map(|file| file.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let integration = value
                .get("integration")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            (files, integration)
        }
        Ok(serde_json::Value::Array(files)) => (
            files.iter().filter_map(|file| file.as_str().map(str::to_string)).collect(),
            None,
        ),
        Ok(serde_json::Value::String(file)) => (vec![file], None),
        // A plain value: the shared files, separated by commas.
        _ => (
            raw.split(',').map(|file| file.trim().to_string()).filter(|file| !file.is_empty()).collect(),
            None,
        ),
    }
}

/// The information line of one overlap: the task, the files it shares and
/// the integration branch it shares them with.
pub fn overlap_line(overlap: &Overlap) -> String {
    let integration = if !overlap.integration.is_empty() {
        overlap.integration.clone()
    } else if !overlap.repo.is_empty() {
        overlap.repo.clone()
    } else {
        "integration".to_string()
    };
    format!(
        "overlap: {} shares {} with {integration}",
        overlap.task,
        overlap.files.join(", ")
    )
}

/// The seat's own account of why it is not running (T-82): None while it runs
/// or is about to (Active, Standby, Starting, Queued). A seat in NeedsCheck or
/// Unknown can be retried with one plan line; the others say what they are.
fn down_seat(seat: &gear_store::SeatRow) -> Option<DownSeat> {
    let state = match seat.state {
        SeatState::Active | SeatState::Standby | SeatState::Starting | SeatState::Queued => {
            return None
        }
        SeatState::NeedsCheck => "needs check",
        other => other.as_str(),
    };
    Some(DownSeat {
        state: state.to_string(),
        reason: seat.state_reason.clone().unwrap_or_default(),
        action: match seat.state {
            // The captain's way back: the plan's `# Staff` line (T-82).
            SeatState::NeedsCheck | SeatState::Unknown => Some(format!("- retry {}", seat.name)),
            // Held is left by the plan line that releases it (T-133).
            SeatState::Held => Some(format!("- release {}", seat.name)),
            _ => None,
        },
    })
}

/// Only label the agent in this generation's active binding.
///
/// A seat whose provider is in a back-off says so before anything else
/// (T-75): it waits for the provider, and that is not a stall.
fn observed_agent_state(
    tx: &Tx<'_>, seat: &gear_store::SeatRow, agents: Option<&[AgentInfo]>, backoff: &[String],
) -> GearResult<&'static str> {
    if backoff.iter().any(|waiting| waiting == &seat.id) {
        return Ok("provider back-off");
    }
    let Some(agents) = agents else { return Ok("unknown"); };
    let Some(binding) = tx.get_binding(&seat.id, seat.generation)?
        .filter(|binding| binding.status == BindingStatus::Active)
    else { return Ok("unknown"); };
    let Some(agent) = agents.iter().find(|agent| agent.pane_id == binding.pane_id && agent.agent.is_some())
    else { return Ok("missing"); };
    if agent.agent.as_deref() != Some(binding.kind.as_str())
        || binding.session_ref.as_deref().is_some_and(|expected|
            agent.agent_session.as_ref().map(|session| session.value.as_str()) != Some(expected))
    {
        return Ok("unknown");
    }
    Ok(match agent.agent_status {
        AgentStatus::Idle => "idle", AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked", AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    })
}

/// Shared snapshot/board wording: agent state and task state have separate labels.
pub fn lane_line(lane: &LaneBoard) -> String {
    let current = lane.tasks.iter().find(|task| task.state == TaskState::Running)
        .or_else(|| lane.tasks.iter().find(|task| task.state == TaskState::Waiting))
        .or_else(|| lane.tasks.iter().find(|task| task.state == TaskState::Verifying));
    let current = current.map(|task| format!("{} {}", task.id, state_word(task.state)))
        .unwrap_or_else(|| "no current task".into());
    let queued = lane.tasks.iter().filter(|task| task.state == TaskState::Ready).count();
    match &lane.down {
        // A seat that is not running says why, and how it gets back (T-82):
        // the state, the reason, the plan line — never a bare "agent unknown".
        Some(down) => {
            let mut line = format!("{} · {}", lane.lane, down.state);
            if !down.reason.is_empty() {
                line.push_str(": ");
                line.push_str(&down.reason);
            }
            if let Some(action) = &down.action {
                line.push_str(" · ");
                line.push_str(action);
            }
            format!("{line} · {current} · {queued} queued")
        }
        None => format!("{} · agent {} · {} · {} queued", lane.lane, lane.agent_state, current, queued),
    }
}

/// The seat states that hold a temp slot (design/10): STARTING, ACTIVE,
/// RELEASING and UNKNOWN occupy the quota; QUEUED and RELEASED do not.
fn holds_temp_slot(state: SeatState) -> bool {
    matches!(
        state,
        SeatState::Starting | SeatState::Active | SeatState::Releasing | SeatState::Unknown
    )
}

/// The board as text: what the user reads on the screen.
pub fn render(board: &Board, view: View) -> String {
    let mut head = vec!["GEAR board".to_string()];
    if board.engine == EngineState::Down {
        head.push(ENGINE_DOWN.to_string());
    }
    let mut blocks: Vec<Vec<String>> = vec![head];
    for project in &board.projects {
        blocks.push(project_lines(project, view));
    }
    blocks
        .iter()
        .map(|block| block.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The lines of one project block.
fn project_lines(project: &ProjectBoard, view: View) -> Vec<String> {
    let mut lines = vec![
        format!("project {}", project.project),
        format!(
            "Totals: {} tasks · {} done · {} in progress · {} waiting · rework {}/{}",
            project.totals.tasks,
            project.totals.done,
            project.totals.in_progress,
            project.totals.waiting,
            project.totals.rejected,
            project.totals.handed_in
        ),
    ];
    let mut voyage_lines: Vec<_> = project.voyages.iter().flat_map(|v| v.lines()).collect();
    if !voyage_lines.is_empty() {
        voyage_lines.push(String::new());
        voyage_lines.append(&mut lines);
        lines = voyage_lines;
    }
    if let Some(quota) = &project.captain_quota {
        let status = if quota.awaiting_view {
            "quota back — waiting for the captain to run gear".to_string()
        } else if quota.remaining_s == 0 {
            "reset reached — waiting to wake the captain".to_string()
        } else {
            format!("out — reset in {} min", (quota.remaining_s + 59) / 60)
        };
        lines.push(format!("Captain quota: {status}; new tasks paused, current work/reviews/merges continue"));
    }
    match view {
        View::Lanes => {
            if !project.lanes.is_empty() {
                lines.push(String::new());
                for lane in &project.lanes {
                    lines.push(format!("Lane {}{}", lane_line(lane), lane_measures(&lane.rework)));
                    for task in &lane.tasks {
                        lines.push(format!("  {} {}", task.id, state_word(task.state)));
                    }
                }
            }
        }
        View::Deps => {
            if !project.deps.is_empty() {
                lines.push(String::new());
                lines.push("Dependencies".to_string());
                for node in &project.deps {
                    lines.push(dep_line(node));
                }
            }
        }
        View::Graph => {
            lines.push(String::new());
            lines.extend(graph::lines(&project.graph, &project.graph_lanes, None));
        }
    }
    // The overlaps with the integration branch (T-188): information lines.
    // They are not items that wait for the captain.
    if !project.overlaps.is_empty() {
        lines.push(String::new());
        for overlap in &project.overlaps {
            lines.push(overlap_line(overlap));
        }
    }
    lines.push(String::new());
    lines.push(waiting_line(&project.waiting));
    lines.push(format!(
        "Temp slots: {} running · {} starting · {} blocked ({} of {} in use)",
        project.temps.running,
        project.temps.starting,
        project.temps.blocked,
        project.temps.in_use,
        project.temps.quota
    ));
    lines
}

/// One line of the dependency tree: the task, its state, and what it still
/// waits for.
fn dep_line(node: &DepNode) -> String {
    let mut line = format!(
        "  {}{} {}",
        "  ".repeat(node.depth),
        node.id,
        state_word(node.state)
    );
    if !node.waits.is_empty() {
        line.push_str(&format!(" · waits {}", node.waits.join(" ")));
    }
    line
}

/// The minutes a task took: from its first attempt to its merge, or until
/// now when it is not merged yet.
fn actual_min_of(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    now: Ts,
) -> GearResult<Option<i64>> {
    let times = tx.attempt_times_of_task(project, task)?;
    let start = times.iter().filter_map(|(_, times)| times.started).min();
    let merged = times.iter().filter_map(|(_, times)| times.merged).max();
    let end = merged.or(Some(now));
    Ok(match (start, end) {
        (Some(start), Some(end)) if end >= start => Some((end - start) / 60),
        _ => None,
    })
}

/// The quota readings of the providers (§9.1): the meta keys
/// `quota.reading.<provider>`.
/// The voyage the Workload view measures: the active one, else the latest
/// departed voyage that is finished (F5).
///
/// The finished voyages are not in `voyages` (the home page and the station
/// watch the active ones only), but their measurements are still true: a
/// landed voyage keeps its burn-up and its lanes.
/// The tokens of a task's rounds (§4.3): None when a round counted none of
/// them, so a sum is never a total that hides an unknown part (F5).
fn tokens_of(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<Option<i64>> {
    let rounds = tx.rounds_of_task(project, task)?;
    let mut total = 0;
    for round in &rounds {
        // The fields' presence says what was counted: a round that counted
        // nothing is a measured zero, not an unknown (F5). A round that has
        // none of the four counted fields counted nothing at all: the sum
        // hides it, so the sum is unknown (F5 of the T-166 review). The
        // fields that are absent on a measured round stay absent.
        if round.input.is_none()
            && round.output.is_none()
            && round.cache_read.is_none()
            && round.cache_write.is_none()
        {
            return Ok(None);
        }
        total += round.input.unwrap_or(0)
            + round.output.unwrap_or(0)
            + round.cache_read.unwrap_or(0)
            + round.cache_write.unwrap_or(0);
    }
    Ok((!rounds.is_empty()).then_some(total))
}

/// The cost of a task's rounds (§4.4): None when a round priced none of them,
/// so a sum is never a total that hides an unknown part (F5).
fn cost_of(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<Option<f64>> {
    let rounds = tx.rounds_of_task(project, task)?;
    let mut total = 0.0;
    for round in &rounds {
        // A round that was never priced is unknown, not a price of zero:
        // the sum that hides it is not a total (F5 of the T-166 review).
        let Some(cost) = round.cost_usd else {
            return Ok(None);
        };
        total += cost;
    }
    Ok((!rounds.is_empty()).then_some(total))
}

/// The captain's decisions of the voyage, as the board shows them (§7.1).
fn decisions_of(tx: &Tx<'_>, project: &str, voyage: Option<&str>) -> GearResult<Vec<DecisionBoard>> {
    let Some(voyage) = voyage else {
        return Ok(Vec::new());
    };
    Ok(tx
        .decisions_of(project, voyage)?
        .into_iter()
        .map(|decision| DecisionBoard {
            at: decision.at,
            kind: decision.kind,
            what: decision.what,
            why: decision.why,
            star: decision.star,
        })
        .collect())
}

/// The friction reports of the project (§6.1): the open ones and the closed
/// ones, oldest first.
fn frictions_of(tx: &Tx<'_>, project: &str) -> GearResult<Vec<FrictionBoard>> {
    Ok(tx
        .frictions_of_project(project)?
        .into_iter()
        .map(|friction| FrictionBoard {
            task: friction.task,
            lane: friction
                .seat
                .rsplit(':')
                .next()
                .unwrap_or_default()
                .to_string(),
            body: friction.body,
            closed: friction.closed_at.is_some(),
            outcome: friction.outcome,
        })
        .collect())
}

fn workload_voyage(
    tx: &Tx<'_>,
    project: &str,
    now: Ts,
) -> GearResult<Option<voyage::VoyageReport>> {
    let active = voyage::reports(tx, project, now)?;
    if let Some(report) = active.first() {
        return Ok(Some(report.clone()));
    }
    Ok(voyage::reports_including_finished(tx, project, now)?
        .into_iter()
        .filter(|report| !active.iter().any(|one| one.milestone == report.milestone))
        // A departed voyage: its elapsed time is known.
        .filter(|report| report.elapsed_min.is_some())
        // The latest departure is the one with the least elapsed time.
        .min_by_key(|report| report.elapsed_min.unwrap_or(i64::MAX)))
}

fn quota_readings(tx: &Tx<'_>) -> GearResult<Vec<QuotaReading>> {
    let mut readings = Vec::new();
    for key in tx.meta_keys_with_prefix("quota.reading.")? {
        let Some(raw) = tx.get_meta(&key)? else { continue };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else { continue };
        let mut windows = Vec::new();
        for window in value
            .get("windows")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&Vec::new())
        {
            let Some(name) = window.get("name").and_then(serde_json::Value::as_str) else { continue };
            windows.push(QuotaWindow {
                name: name.to_string(),
                used_percent: window
                    .get("used_percent")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0),
                resets_at: window.get("resets_at").and_then(serde_json::Value::as_i64),
            });
        }
        if windows.is_empty() {
            continue;
        }
        readings.push(QuotaReading {
            provider: value
                .get("provider")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| key.trim_start_matches("quota.reading."))
                .to_string(),
            windows,
            read_at: value
                .get("read_at")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
        });
    }
    Ok(readings)
}

/// The line about the captain's inbox.
fn waiting_line(waiting: &Waiting) -> String {
    let items = match waiting.count {
        1 => "1 item".to_string(),
        count => format!("{count} items"),
    };
    match waiting.oldest_age_s {
        Some(age) => format!(
            "Waiting for the captain: {items} · oldest {}",
            human_age(age)
        ),
        None => format!("Waiting for the captain: {items}"),
    }
}

/// An age in seconds as a human reads it: 45s, 12m, 3h05m, 2d04h.
fn human_age(seconds: i64) -> String {
    match seconds {
        s if s < 60 => format!("{s}s"),
        s if s < 3_600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h{:02}m", s / 3_600, (s % 3_600) / 60),
        s => format!("{}d{:02}h", s / 86_400, (s % 86_400) / 3_600),
    }
}

/// The never-stop segment of a lane line: ` · rework 1/3 · last task 12m ago`.
fn lane_measures(rework: &Rework) -> String {
    match rework.last_task_age_s {
        Some(age) => format!(
            " · rework {}/{} · last task {} ago",
            rework.rejected,
            rework.handed_in,
            human_age(age)
        ),
        None => format!(" · rework {}/{} · no task yet", rework.rejected, rework.handed_in),
    }
}

/// The board's word for a task state (PRD §15): the states of the plan, plus
/// the two ways a task can end without a result. The captain's home page
/// uses the same words (T-9).
pub fn state_word(state: TaskState) -> &'static str {
    match state {
        TaskState::Ready => "queued",
        TaskState::Running => "in progress",
        TaskState::Waiting => "waiting",
        TaskState::Verifying => "in review",
        TaskState::Done => "done",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
        TaskState::Draft => "draft",
    }
}

/// The board as JSON: the same view, machine-readable.
pub fn to_json(board: &Board) -> GearResult<String> {
    let projects: Vec<serde_json::Value> = board
        .projects
        .iter()
        .map(|project| {
            serde_json::json!({
                "project": project.project,
                "voyages": project.voyages.iter().map(|v| serde_json::json!({
                    "milestone": v.milestone, "done": v.done, "eta": v.eta,
                    "elapsed_min": v.elapsed_min, "pace": v.pace(),
                    "remaining_estimate_min": v.remaining_estimate_min,
                    "projected_min": v.projected_min, "projected_at": v.projected_at,
                    "scope_min": v.scope_min, "departure_scope_min": v.departure_scope_min,
                    "rework_rounds": v.rework_rounds, "slowest": v.slowest,
                })).collect::<Vec<_>>(),
                "totals": {
                    "tasks": project.totals.tasks,
                    "done": project.totals.done,
                    "in_progress": project.totals.in_progress,
                    "waiting": project.totals.waiting,
                    "handed_in": project.totals.handed_in,
                    "rejected": project.totals.rejected,
                },
                "lanes": project.lanes.iter().map(|lane| serde_json::json!({
                    "lane": lane.lane,
                    "agent_state": lane.agent_state,
                    "rework": {
                        "handed_in": lane.rework.handed_in,
                        "rejected": lane.rework.rejected,
                        "last_task_age_s": lane.rework.last_task_age_s,
                    },
                    "tasks": lane.tasks.iter().map(|task| serde_json::json!({
                        "id": task.id,
                        "state": state_word(task.state),
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "decisions": project.decisions.iter().map(|decision| serde_json::json!({
                    "at": decision.at, "kind": decision.kind, "what": decision.what,
                    "why": decision.why, "star": decision.star,
                })).collect::<Vec<_>>(),
                "frictions": project.frictions.iter().map(|friction| serde_json::json!({
                    "task": friction.task, "lane": friction.lane, "body": friction.body,
                    "closed": friction.closed, "outcome": friction.outcome,
                })).collect::<Vec<_>>(),
                "overlaps": project.overlaps.iter().map(|overlap| serde_json::json!({
                    "task": overlap.task, "repo": overlap.repo,
                    "integration": overlap.integration, "files": overlap.files,
                })).collect::<Vec<_>>(),
                "deps": project.deps.iter().map(|node| serde_json::json!({
                    "id": node.id,
                    "state": state_word(node.state),
                    "waits": node.waits,
                    "depth": node.depth,
                })).collect::<Vec<_>>(),
                "captain_quota": project.captain_quota.as_ref().map(|quota| serde_json::json!({
                    "reset_at": quota.reset_at, "remaining_s": quota.remaining_s,
                    "awaiting_view": quota.awaiting_view,
                })),
                "captain_waiting": {
                    "count": project.waiting.count,
                    "oldest_age_s": project.waiting.oldest_age_s,
                },
                "temps": {
                    "in_use": project.temps.in_use,
                    "quota": project.temps.quota,
                    "running": project.temps.running,
                    "starting": project.temps.starting,
                    "blocked": project.temps.blocked,
                },
                "unconfirmed_deliveries": project.unconfirmed_deliveries,
            })
        })
        .collect();
    let doc = serde_json::json!({ "engine": board.engine.as_str(), "projects": projects });
    serde_json::to_string(&doc).map_err(|e| {
        GearError::new(ErrorCode::BadFormat, format!("cannot serialize the board: {e}"))
    })
}

/// Show the board until the user quits (`q`).
///
/// Without a terminal — a pipe, a test — the board is printed once and the
/// command returns: a full-screen view must not hang a caller that cannot
/// see it. [`run_with`] draws the frames.
pub fn run(store: &mut Store, opts: &Options, out: &mut dyn Write, mut herdr: Option<&mut dyn HerdrRead>) -> GearResult<()> {
    if opts.json || !screen::is_terminal() {
        let agents = herdr.as_mut().and_then(|herdr| herdr.agents().ok());
        let board = build_with_agents(store, opts, now_ts(), agents.as_deref())?;
        return print_once(&board, opts, out);
    }
    let mut keys = screen::Terminal::new();
    screen::enter(out).map_err(write_error)?;
    // NO_COLOR (no-color.org): any value but the empty one turns the colours
    // off.
    let opts = Options {
        color: opts.color && style::colour_allowed(std::env::var_os("NO_COLOR").as_deref()),
        ..opts.clone()
    };
    let result = run_with(store, &opts, out, &mut keys, herdr);
    // The terminal is given back even when the board failed.
    let _ = screen::leave(out);
    result
}

/// The frames over injected keys: the board is redrawn every
/// [`Options::refresh`], `q` leaves it, the arrows and the tab key change the
/// view, the numbers pick a view, the up and down keys move the selection and
/// `Enter` opens the panel of the selected row.
pub fn run_with(
    store: &mut Store,
    opts: &Options,
    out: &mut dyn Write,
    keys: &mut dyn Keys,
    mut herdr: Option<&mut dyn HerdrRead>,
) -> GearResult<()> {
    let mut tab = tab_of(opts.view);
    let mut selected = 0usize;
    let mut panel: Option<Vec<style::Line>> = None;
    let mut first = true;
    loop {
        let agents = herdr.as_mut().and_then(|herdr| herdr.agents().ok());
        let board = match build_with_agents(store, opts, now_ts(), agents.as_deref()) {
            Ok(board) => board,
            Err(e) => {
                // The first frame decides whether the board can start at all.
                if first {
                    return Err(e);
                }
                // A frame that cannot be read is shown as the failure it is:
                // an observation failure is not an all-clear (17 §5).
                let text = format!(
                    "GEAR board\n\n! the plan could not be read: {}: {}",
                    e.code, e.detail
                );
                screen::draw(out, &text).map_err(write_error)?;
                let deadline = std::time::Instant::now() + opts.refresh;
                let _ = keys.next_event(deadline.saturating_duration_since(std::time::Instant::now()));
                continue;
            }
        };
        // The selection never leaves the rows of the view (F4): a view that
        // shrank, or a key pressed at its end, does not lose the selection.
        let rows = frame::view(&board, tab, selected).rows.len();
        selected = selected.min(rows.saturating_sub(1));
        let text = frame::canvas(&board, tab, selected, panel.as_deref(), keys.size()).ansi(opts.color);
        first = false;
        screen::draw(out, &text).map_err(write_error)?;
        let deadline = std::time::Instant::now() + opts.refresh;
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            let Some(event) = keys.next_event(deadline - now) else { continue };
            if panel.is_some() {
                // Any key closes the panel (the prototype): the board's own
                // keys come after it.
                panel = None;
                break;
            }
            match event {
                Event::Key('q') | Event::Key('Q') | Event::Key('\u{3}') => return Ok(()),
                Event::Right => {
                    tab = tab.next();
                    selected = 0;
                    panel = None;
                    break;
                }
                Event::Left => {
                    tab = tab.previous();
                    selected = 0;
                    panel = None;
                    break;
                }
                Event::Key('\t') => {
                    tab = tab.next();
                    selected = 0;
                    panel = None;
                    break;
                }
                Event::BackTab => {
                    tab = tab.previous();
                    selected = 0;
                    panel = None;
                    break;
                }
                Event::Key(character) if character.is_ascii_digit() => {
                    if let Some(picked) = Tab::of_number(character as u8 - b'0') {
                        tab = picked;
                        selected = 0;
                        panel = None;
                        break;
                    }
                }
                Event::Up | Event::Key('k') => {
                    selected = selected.saturating_sub(1);
                    panel = None;
                    break;
                }
                Event::Down | Event::Key('j') => {
                    selected = (selected + 1).min(rows.saturating_sub(1));
                    panel = None;
                    break;
                }
                Event::Key('\r') | Event::Key('\n') => {
                    panel = open_panel(&board, store, tab, selected)?;
                    break;
                }
                Event::Mouse { button: 0, x, y } => {
                    // A click on the tab row picks a view; a click on a row of
                    // the view picks that row. The rows are the ones the
                    // frame draws: the tabs on their own row, the data below
                    // the heading.
                    if y == frame::TAB_ROW as u16 + 1 {
                        if let Some(picked) = frame::tab_at(x) {
                            tab = picked;
                            selected = 0;
                            panel = None;
                            break;
                        }
                    } else {
                        // Only the rows the frame draws are rows of the view
                        // (F8): the click is on the screen row, the spacer
                        // below the data and the footer's row select nothing.
                        let row = y.saturating_sub(1) as usize;
                        let last = keys.size().1.saturating_sub(2);
                        if row >= frame::FIRST_ROW && row < last {
                            // The rows above the viewport are scrolled away:
                            // the clicked row is the one the frame drew.
                            let scroll = frame::scroll_of(rows, selected, keys.size().1);
                            let index = (row - frame::FIRST_ROW) + scroll;
                            // A click outside the rows of the view is ignored.
                            if index < rows {
                                selected = index;
                                panel = None;
                                break;
                            }
                        }
                    }
                }
                Event::Key(_) | Event::Mouse { .. } => {}
            }
        }
    }
}

/// The tab the board starts in: the old views of the board map to the tabs of
/// the new one (the lanes are the first view, the dependencies are the graph).
fn tab_of(view: View) -> frame::Tab {
    match view {
        View::Deps | View::Graph => frame::Tab::Graph,
        View::Lanes => frame::Tab::Now,
    }
}

/// The panel of the selected row: the lane's work in the first view, the
/// rounds of the task in the graph.
fn open_panel(
    board: &Board,
    store: &mut Store,
    tab: frame::Tab,
    selected: usize,
) -> GearResult<Option<Vec<style::Line>>> {
    let Some(project) = board.projects.first() else { return Ok(None) };
    match tab {
        frame::Tab::Now | frame::Tab::Workload => Ok(project.lanes.get(selected).map(|lane| {
            frame::styled_lane_panel(lane, frame::lane_color_of(project, &lane.lane))
        })),
        frame::Tab::Graph => {
            let Some(task) = project.graph.get(selected) else { return Ok(None) };
            let rows = store.read(|tx| graph::styled_panel(tx, &project.project, &task.id, now_ts()))?;
            Ok(Some(rows))
        }
        frame::Tab::Tokens => {
            // The task panel of the task the row names: its rounds with their
            // tokens.
            let tasks = frame::token_tasks(project);
            let Some((_, task)) = tasks.get(selected) else {
                return Ok(None);
            };
            let rows = store.read(|tx| graph::styled_panel(tx, &project.project, &task.id, now_ts()))?;
            Ok(Some(rows))
        }
        _ => Ok(None),
    }
}

/// Print the board once: the JSON or the text, depending on the options.
fn print_once(board: &Board, opts: &Options, out: &mut dyn Write) -> GearResult<()> {
    if opts.json {
        writeln!(out, "{}", to_json(board)?).map_err(write_error)?;
    } else {
        let size = (screen::DEFAULT_WIDTH, screen::DEFAULT_ROWS);
        writeln!(out, "{}", frame::frame(board, tab_of(opts.view), 0, None, size)).map_err(write_error)?;
    }
    out.flush().map_err(write_error)
}

/// Unix seconds — the clock the store and the engine share.
fn now_ts() -> Ts {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A board that could not be written is reported.
fn write_error(e: std::io::Error) -> GearError {
    GearError::new(ErrorCode::BadFormat, format!("cannot write the board: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_of_the_states_are_the_ones_of_the_plan() {
        assert_eq!(state_word(TaskState::Ready), "queued");
        assert_eq!(state_word(TaskState::Running), "in progress");
        assert_eq!(state_word(TaskState::Waiting), "waiting");
        assert_eq!(state_word(TaskState::Verifying), "in review");
        assert_eq!(state_word(TaskState::Done), "done");
    }

    #[test]
    fn ages_read_like_a_clock() {
        assert_eq!(human_age(0), "0s");
        assert_eq!(human_age(45), "45s");
        assert_eq!(human_age(60), "1m");
        assert_eq!(human_age(12 * 60), "12m");
        assert_eq!(human_age(3 * 3_600 + 5 * 60), "3h05m");
        assert_eq!(human_age(2 * 86_400 + 4 * 3_600), "2d04h");
    }

    #[test]
    fn a_free_lock_means_the_engine_is_down() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("gear.db");
        // No lock file at all: the engine never ran.
        assert_eq!(EngineState::probe(Some(&db)), EngineState::Down);
        // A lock file nobody holds: the engine is down.
        let path = dir.path().join("engine.lock");
        std::fs::write(&path, "1234\n").unwrap();
        assert_eq!(EngineState::probe(Some(&db)), EngineState::Down);
        // The engine holds its lock while it runs.
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        file.try_lock().unwrap();
        assert_eq!(EngineState::probe(Some(&db)), EngineState::Running);
    }

    #[test]
    fn a_missing_database_path_means_the_engine_is_down() {
        assert_eq!(EngineState::probe(None), EngineState::Down);
    }
}
