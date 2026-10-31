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
pub mod forecast;
pub mod frame;
pub mod golden;
pub mod graph;
pub mod idle;
mod screen;
pub mod stations;
pub mod style;
pub mod voyage;

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use gear_model::{
    ErrorCode, GearError, GearResult, MsgKind, OpKind, OpState, SeatRole, SeatState, TaskState, Ts,
};
use gear_store::{BindingStatus, MessageRow, RoundRow, SeatRow, Store, TaskRow, Tx};

use crate::frame::Tab;
use crate::idle::IdleReason;
use gear_herdr::{AgentInfo, AgentStatus, HerdrRead};

pub use deps::DepNode;
pub use screen::{Event, Keys, Terminal};

/// How long a frame stays on the screen before the board redraws it.
pub const REFRESH: Duration = Duration::from_secs(2);

/// How old a kept board may get before it is built again (T-384). The board
/// shows a clock in its header, and some of its texts are an age or a
/// deadline: those must stay true, so a kept board is rebuilt once a minute
/// rather than once a refresh.
pub const REBUILD_AFTER: Duration = Duration::from_secs(60);

/// How often the board asks Herdr for its agents (T-384). The agent list is
/// kept between asks: a board that waited on it every refresh paid a Herdr
/// call every two seconds for a list that rarely moves.
pub const AGENTS_EVERY: Duration = Duration::from_secs(10);

/// The picks one notch of the mouse wheel moves the selection (T-411).
pub const WHEEL_PICKS: usize = 3;

/// The knobs the tests of T-384 turn: the board builds nothing while nothing
/// changes, and a test proves it by counting the builds and shortening the two
/// intervals it waits on. The values are per thread — a test runs on its own
/// thread, so one test cannot shorten another's minute — and production never
/// sets them.
pub mod hook {
    use std::cell::Cell;
    use std::time::Duration;

    thread_local! {
        static BUILDS: Cell<usize> = Cell::new(0);
        static STALE_AFTER: Cell<Duration> = Cell::new(super::REBUILD_AFTER);
        static AGENTS_EVERY: Cell<Duration> = Cell::new(super::AGENTS_EVERY);
    }

    /// One more board was built; [`run_with`](crate::run_with) counts them.
    pub(crate) fn built() {
        BUILDS.with(|count| count.set(count.get() + 1));
    }

    /// The boards [`run_with`](crate::run_with) built on this thread since the
    /// last [`reset`].
    pub fn builds() -> usize {
        BUILDS.with(Cell::get)
    }

    /// Count the builds from zero and put both intervals back to their
    /// defaults.
    pub fn reset() {
        BUILDS.with(|count| count.set(0));
        STALE_AFTER.with(|value| value.set(super::REBUILD_AFTER));
        AGENTS_EVERY.with(|value| value.set(super::AGENTS_EVERY));
    }

    /// The age at which a kept board is built again.
    pub fn stale_after() -> Duration {
        STALE_AFTER.with(Cell::get)
    }

    /// How often the board asks Herdr for its agents.
    pub fn agents_every() -> Duration {
        AGENTS_EVERY.with(Cell::get)
    }

    /// Shorten the age for a test.
    pub fn set_stale_after(after: Duration) {
        STALE_AFTER.with(|value| value.set(after));
    }

    /// Shorten the agent-list interval for a test.
    pub fn set_agents_every(every: Duration) {
        AGENTS_EVERY.with(|value| value.set(every));
    }
}

/// The lane name of a task that belongs to no lane.
const UNASSIGNED: &str = "unassigned";

/// What a lane of a landed voyage reads (T-350): the voyage keeps its data,
/// and its lanes are retired at the landing.
const RETIRED_AT_THE_LANDING: &str = "retired at the landing";

/// The lane a task names, or the placeholder of an unassigned task.
fn lane_of(task: &TaskRow) -> String {
    task.lane.clone().unwrap_or_else(|| UNASSIGNED.to_string())
}

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
    /// The voyage to show; `None` shows the voyage measured today (T-350).
    pub milestone: Option<String>,
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
            milestone: None,
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
    /// The time the board was built at: the header's clock, and nothing else.
    /// A frame depends on the store and on this `now`, never on the wall clock.
    pub now: Ts,
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
    /// The same deliveries named, oldest first (T-271): the home page says
    /// which ones they are, so the count is never a number without names.
    pub unconfirmed: Vec<Unconfirmed>,
    /// The quota readings of the providers (§9.1).
    pub quota: Vec<QuotaReading>,
    /// The captain's decisions of this voyage (§7.1): the log the board
    /// shows in its fourth view.
    pub decisions: Vec<DecisionBoard>,
    /// The confirmed goal changes of the measured voyage (T-335): the user's
    /// decisions on its `done`, `bar` and `eta`, oldest first; empty without
    /// a measured voyage. The S4 header draws them red.
    pub goal_changes: Vec<GoalChangeBoard>,
    /// The latest confirmed goal change of the shown voyage (T-358), as
    /// (time, before, after): the S4 header draws it red under the goal box.
    /// `None` while the voyage's goal never changed.
    pub goal_changed: Option<(String, String, String)>,
    /// The goal check of the shown voyage (T-354): each clause of the goal and
    /// whether it is met. The plan's clauses when the store has them, else the
    /// clauses of the voyage's done sentence, every one unmet.
    pub goal_check: Vec<(bool, String)>,
    /// The friction reports the agents left (§6.1), open and closed.
    pub frictions: Vec<FrictionBoard>,
    /// The tasks that overlap the integration branch of their repo (T-188):
    /// information lines, never items that wait for the captain.
    pub overlaps: Vec<Overlap>,
    /// The stations of the measured voyage, in plan order, with their
    /// progress (T-334); empty without a measured voyage.
    pub stations: Vec<stations::StationProgress>,
    /// Each lane's queue played forward (T-349), in the order of `lanes`: when
    /// the lane ends and how long it sits idle. Empty when no measured voyage
    /// has open work.
    pub forecast: Vec<forecast::LaneEnd>,
    /// What the shown tasks wait on, in plan order (T-360): a task in review
    /// names the reviewer and how long it has held it, a task that waits for
    /// the captain names what he owes. The board's Now view lists them as items
    /// to handle, each with who acts.
    pub waiting_on: Vec<String>,
    /// Every voyage of the project that departed, newest departure first
    /// (T-350): the board can be built for any of them.
    pub voyage_list: Vec<VoyageEntry>,
    /// The voyage the board measures and shows (T-350): the one it was built
    /// for.
    pub shown: Option<VoyageEntry>,
    /// The tasks that left the shown voyage for another (T-351), as (task, the
    /// voyage it moved to), in the order the voyage recorded them. A moved task
    /// carries the new voyage's milestone, so the shown voyage counts it
    /// nowhere.
    pub moved_out: Vec<(String, String)>,
}

/// One voyage of the project, as the board names it (T-350).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoyageEntry {
    /// The milestone id the plan uses (`M1`, `M2`, …).
    pub milestone: String,
    pub title: String,
    /// The voyage's `done` sentence: the goal the board boxes (T-353).
    pub done: Option<String>,
    /// The voyage's quality bar, as the plan wrote it (T-353).
    pub bar: Option<String>,
    /// Some task of the voyage is not settled: it is still sailing.
    pub sailing: bool,
    /// When the voyage departed; `None` while it has no task.
    pub departure: Option<Ts>,
    /// When a landed voyage was merged in: the latest merge of its tasks.
    /// `None` while it sails, or when no attempt of it was merged.
    pub ended: Option<Ts>,
}

/// One delivery the engine has not confirmed yet (T-271).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unconfirmed {
    /// The op, e.g. `O-12`.
    pub op: String,
    /// The seat's name (not its id).
    pub seat: String,
    /// The task the delivery carried, when it named one.
    pub task: Option<String>,
    /// How long it has been in flight: `now` − the op's creation.
    pub age_s: i64,
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

/// One confirmed goal change of the measured voyage (T-335).
///
/// T-410 records each confirmed change of a departed voyage's `done`, `bar`
/// or `eta` (T-331 its title and stations) as a starred decision of kind
/// `user` with source `gear`; the S4 header draws them red.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalChangeBoard {
    /// When the decision was taken.
    pub at: String,
    /// What changed: `<field> of <M> changed from "<old>" to "<new>"`.
    pub what: String,
    /// Why it changed.
    pub why: Option<String>,
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
    /// The help the lane gives on an unanswered instruction of the captain
    /// (T-323), its text: `helping <lane>: …`.
    pub helping: Option<String>,
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
    /// The queue position in the plan, ascending (the store's rank): the Now
    /// view's items to handle follow the plan, not the lanes' grouping (T-360).
    pub rank: i64,
    pub state: TaskState,
    /// The task's stage in the graph, and its words (T-347): the lane lines of
    /// the board and of the captain's home page name the stage in the same
    /// words as the board's views.
    pub stage: graph::Stage,
    pub stage_words: String,
    /// Whether the task's latest round is a new-session recovery (T-481,
    /// design/10 §resume rules): its earlier work ran on another CLI, so the
    /// round could not continue that conversation. The board says so, as the
    /// seat's home page does.
    pub recovery: bool,
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
    /// The fresh tokens of the task's rounds (input, output, cache write),
    /// when they are counted (§4.3).
    pub tokens: Option<i64>,
    /// The cache reads of the task's rounds, when they are counted: they are
    /// shown apart from the fresh tokens, since a task that re-reads its
    /// context shows millions of them (the user, 2026-10-28).
    pub cache_read: Option<i64>,
    /// The cost of the task's rounds, when it is known (§4.4).
    pub cost_usd: Option<f64>,
    /// The fresh tokens of the task's development rounds (T-363): the `dev`
    /// and `rework` rounds, apart from the review rounds.
    pub dev_tokens: Option<i64>,
    /// The fresh tokens of the task's review rounds (T-363).
    pub review_tokens: Option<i64>,
    /// The cost of the task's development rounds (T-363).
    pub dev_cost: Option<f64>,
    /// The cost of the task's review rounds (T-363).
    pub review_cost: Option<f64>,
    /// The tasks it depends on (T-349): the board's forecast waits for them to
    /// merge before the task can start.
    pub deps: Vec<String>,
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
    store: &mut Store,
    opts: &Options,
    now: Ts,
    agents: Option<&[AgentInfo]>,
) -> GearResult<Board> {
    let engine = EngineState::probe(opts.db.as_deref());
    let projects = store.read(|tx| {
        let ids = match &opts.project {
            Some(id) => vec![id.clone()],
            None => tx.project_ids()?,
        };
        ids.iter()
            .map(|id| {
                // The board shows one voyage only (README change 6): the one
                // named, else the measured one, so a task of another voyage
                // never reaches it (T-350).
                let milestone = match &opts.milestone {
                    Some(milestone) => Some(milestone.clone()),
                    None => workload_voyage(tx, id, now)?.map(|voyage| voyage.milestone),
                };
                project_for(tx, id, now, agents, milestone.as_deref())
            })
            .collect()
    })?;
    Ok(Board {
        engine,
        now,
        projects,
    })
}

/// The plan of one project: the model both the board and the captain's home
/// page show. One read transaction; nothing is written.
pub fn project(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<ProjectBoard> {
    project_with_agents(tx, project, now, None)
}

/// The shared project model with an optional read-only Herdr snapshot.
///
/// `None` keeps the choice the captain home page makes: the voyage measured
/// today, with its tasks and every open task of the project (T-246). The board
/// names a voyage instead and shows that voyage only (T-350).
pub fn project_with_agents(
    tx: &Tx<'_>,
    project: &str,
    now: Ts,
    agents: Option<&[AgentInfo]>,
) -> GearResult<ProjectBoard> {
    project_for(tx, project, now, agents, None)
}

/// The plan of one project for one voyage (T-350): `milestone` names the
/// voyage to show, `None` measures the voyage of today.
pub fn project_for(
    tx: &Tx<'_>,
    project: &str,
    now: Ts,
    agents: Option<&[AgentInfo]>,
    milestone: Option<&str>,
) -> GearResult<ProjectBoard> {
    let row = tx.get_project(project)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("project {project} is not registered"),
        )
    })?;
    let seats = tx.seats_of_project(project)?;
    let all_tasks = tx.tasks_of_project(project)?;
    // The voyage the board measures: the one named, else the active one, else
    // the latest departed one (F5 of the T-166 review).
    let workload = match milestone {
        Some(milestone) => voyage::reports_including_finished(tx, project, now)?
            .into_iter()
            .find(|report| report.milestone == milestone),
        None => workload_voyage(tx, project, now)?,
    };
    // The tasks the board shows. A named voyage shows its own tasks and no
    // task of another voyage (T-350). A cancelled task stays in its lane's
    // queue — it was cut, not deleted — and the totals leave it out (T-415).
    // Without a name, the home page keeps today's rule: the measured voyage's
    // tasks and every open task of the project (T-246).
    let tasks: Vec<_> = match milestone {
        Some(shown) => all_tasks
            .iter()
            .filter(|task| task.milestone.as_deref() == Some(shown))
            .cloned()
            .collect(),
        None => match &workload {
            Some(voyage) => all_tasks
                .iter()
                .filter(|task| {
                    !task.state.is_terminal()
                        || task.milestone.as_deref() == Some(voyage.milestone.as_str())
                })
                .cloned()
                .collect(),
            None => all_tasks.clone(),
        },
    };
    // Every voyage of the project, and the measured one as a row (T-350).
    let entries = voyage_entries(tx, project, &all_tasks)?;
    let shown = match &workload {
        Some(voyage) => entries
            .iter()
            .find(|entry| entry.milestone == voyage.milestone)
            .cloned(),
        None => None,
    };
    // A landed voyage keeps its data: its lanes are retired at the landing.
    let landed = shown.as_ref().is_some_and(|entry| !entry.sailing);
    // A voyage the plan described carries a done sentence (T-445): a
    // milestone it never described has none, and the first row must not list
    // it next to the real voyages.
    let mut voyage_list: Vec<VoyageEntry> = entries
        .into_iter()
        .filter(|entry| entry.departure.is_some())
        .filter(|entry| {
            entry
                .done
                .as_deref()
                .is_some_and(|done| !done.trim().is_empty())
        })
        .collect();
    // The newest departure is the voyage the captain has just sent out.
    voyage_list.sort_by(|left, right| right.departure.cmp(&left.departure));
    // The seats whose provider is in a back-off (T-75): the engine recorded
    // the wait, and the board says so instead of "working".
    let backoff = tx.seats_in_backoff(project)?;

    // Keep active empty lanes visible, but never recreate a retired lane
    // from a historical task assignment below.
    let retired: std::collections::BTreeSet<_> = seats
        .iter()
        .filter(|seat| seat.role == SeatRole::Lane && seat.state == SeatState::Retired)
        .map(|seat| seat.name.as_str())
        .collect();
    let mut lanes = Vec::new();
    if landed {
        // A landed voyage keeps its data (T-350): the lanes of its tasks,
        // retired ones included, in seat order, a lane with no seat after
        // them. Every one of them retired at the landing.
        let mut names: Vec<String> = Vec::new();
        for seat in seats.iter().filter(|seat| seat.role == SeatRole::Lane) {
            if !names.contains(&seat.name) && tasks.iter().any(|task| lane_of(task) == seat.name) {
                names.push(seat.name.clone());
            }
        }
        for task in &tasks {
            let name = lane_of(task);
            if !names.contains(&name) {
                names.push(name);
            }
        }
        for name in names {
            let seat = seats.iter().find(|seat| seat.name == name);
            let agent_state = match seat {
                Some(seat) => observed_agent_state(tx, seat, agents, &backoff)?,
                None => "unknown",
            };
            lanes.push(LaneBoard {
                lane: name,
                agent_state,
                tasks: Vec::new(),
                rework: Rework::default(),
                down: seat.and_then(down_seat),
                idle: None,
                idle_text: Some(RETIRED_AT_THE_LANDING.into()),
                helping: None,
                done_count: 0,
                left_count: 0,
                left_min: 0,
            });
        }
    } else {
        for seat in seats
            .iter()
            .filter(|seat| seat.role == SeatRole::Lane && seat.state != SeatState::Retired)
        {
            lanes.push(LaneBoard {
                lane: seat.name.clone(),
                agent_state: observed_agent_state(tx, seat, agents, &backoff)?,
                tasks: Vec::new(),
                rework: Rework::default(),
                down: down_seat(seat),
                idle: None,
                idle_text: None,
                helping: None,
                done_count: 0,
                left_count: 0,
                left_min: 0,
            });
        }
    }

    let mut totals = Totals::default();
    for task in &tasks {
        // A draft task is not in the plan yet; the board shows the plan.
        if task.state == TaskState::Draft {
            continue;
        }
        // A cancelled task is not live work (T-415): it stays in the Tasks
        // view below, but the totals count only what the voyage still can
        // deliver, as the stations do (station.rs leaves it out too).
        if task.state != TaskState::Cancelled {
            totals.tasks += 1;
            match task.state {
                TaskState::Done => totals.done += 1,
                TaskState::Running => totals.in_progress += 1,
                TaskState::Waiting => totals.waiting += 1,
                _ => {}
            }
        }
        let name = lane_of(task);
        if !landed && retired.contains(name.as_str()) {
            continue;
        }
        let rounds = tx.attempts_of_task(project, &task.id)?.len();
        // The stage the graph gives this task, and its words (T-347): one
        // reading for the views and the lane lines.
        let stage = graph::stage_of(tx, project, task)?;
        let stage_words = graph::Stage::words_of(stage, &task.deps, rounds, None);
        // The latest attempt's round, for the new-session recovery mark
        // (T-481).
        let recovery = match tx.attempts_of_task(project, &task.id)?.last() {
            Some(attempt) => tx
                .session_recovery(project, &task.id, attempt.n)?
                .is_some(),
            None => false,
        };
        let line = TaskBoard {
            id: task.id.clone(),
            rank: task.rank,
            state: task.state,
            stage,
            stage_words,
            recovery,
            title: tx
                .get_contract(project, &task.id, task.contract_version)?
                .map(|contract| contract.goal)
                .unwrap_or_default(),
            est_min: task.estimate_min,
            actual_min: actual_min_of(tx, project, &task.id, now)?,
            rounds,
            stored_rounds: tx.rounds_of_task(project, &task.id)?.len(),
            tokens: tokens_of(tx, project, &task.id)?,
            cache_read: cache_read_of(tx, project, &task.id)?,
            cost_usd: cost_of(tx, project, &task.id)?,
            dev_tokens: side_tokens_of(tx, project, &task.id, Side::Development)?,
            review_tokens: side_tokens_of(tx, project, &task.id, Side::Review)?,
            dev_cost: side_cost_of(tx, project, &task.id, Side::Development)?,
            review_cost: side_cost_of(tx, project, &task.id, Side::Review)?,
            deps: task.deps.clone(),
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
                helping: None,
                done_count: 0,
                left_count: 0,
                left_min: 0,
            }),
        }
    }
    // One lane order for every view that has a row or a column per lane
    // (T-363, the captain's decision for Tasks made once for all): the lanes
    // by their first task in plan order, a lane with no task after them in its
    // present (seat) order. It is the prototype's order, and the lane colours
    // follow it. The sort is stable, so the lanes without a task keep their
    // order.
    lanes.sort_by_key(|lane| {
        tasks
            .iter()
            .position(|task| lane_of(task) == lane.lane)
            .unwrap_or(usize::MAX)
    });
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

    // Why each lane is idle (design §8.2), what it helps on (T-323) and what
    // it has left.
    for lane in &mut lanes {
        // A lane of a landed voyage retired at the landing (T-350): the words
        // stay those of the landing, not an idle reason of today.
        if !landed {
            let reason = idle::idle_reason(tx, project, &lane.lane, now)?;
            lane.idle_text = reason
                .as_ref()
                .map(|reason| idle::describe(reason, tx, project));
            lane.idle = reason;
            lane.helping = match tx.get_seat(&format!("{project}:{}", lane.lane))? {
                Some(seat) => idle::helping(tx, project, &seat, now)?.map(|help| help.text()),
                None => None,
            };
        }
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

    // The work as a graph (design §8.1), of the tasks the board shows (T-246).
    let graphed: std::collections::BTreeSet<&str> =
        tasks.iter().map(|task| task.id.as_str()).collect();
    let graph: Vec<graph::GraphTask> = graph::tasks(tx, project, now)?
        .into_iter()
        .filter(|task| graphed.contains(task.id.as_str()))
        .collect();
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

    // The deliveries in flight: a Deliver op that is not settled yet, and only
    // while the seat it names is still the live incumbent (T-233). An op of a
    // retired seat, or of an older generation of a live one, is history: the
    // home page showed it as a lost message.
    let mut unconfirmed_deliveries = 0;
    let mut unconfirmed: Vec<Unconfirmed> = Vec::new();
    for state in [
        OpState::Intended,
        OpState::Executing,
        OpState::Submitted,
        OpState::Unknown,
    ] {
        for op in tx.ops_in_state(project, state)? {
            if op.kind != OpKind::Deliver {
                continue;
            }
            let seat = tx.get_seat(&op.seat)?;
            let live = seat.as_ref().is_some_and(|seat| {
                matches!(
                    seat.state,
                    SeatState::Active | SeatState::Standby | SeatState::Starting
                ) && seat.generation == op.generation
            });
            if live {
                unconfirmed_deliveries += 1;
                unconfirmed.push(Unconfirmed {
                    op: op.id.clone(),
                    seat: seat.map(|seat| seat.name).unwrap_or_default(),
                    task: op.task.clone(),
                    age_s: (now - op.created).max(0),
                });
            }
        }
    }
    // The oldest first: the delivery that waits longest is the one that may be
    // a lane that never started.
    unconfirmed.sort_by(|left, right| {
        right
            .age_s
            .cmp(&left.age_s)
            .then_with(|| left.op.cmp(&right.op))
    });

    // What the shown tasks wait on, in plan order (T-360): the Now view lists
    // it under the lanes, each item with who acts. A task in review names the
    // reviewer holding it and how long it has held it; a task that waits for
    // the captain names what he owes.
    let mut ordered: Vec<(&str, &TaskBoard)> = Vec::new();
    for lane in &lanes {
        for task in &lane.tasks {
            ordered.push((lane.lane.as_str(), task));
        }
    }
    ordered.sort_by(|(_, left), (_, right)| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut waiting_on = Vec::new();
    for (lane, task) in ordered {
        match task.stage {
            graph::Stage::Review => {
                let holder = temps_seats.iter().find(|seat| {
                    seat.state == SeatState::Active
                        && seat.task_id.as_deref() == Some(task.id.as_str())
                });
                let name = holder.map(|seat| seat.name.as_str()).unwrap_or("-");
                // The reviewer's open round is when it took the task up; a
                // seat without one counts from its state.
                let since = match holder {
                    Some(seat) => match tx.open_round_of_seat(&seat.id)? {
                        Some(round) => round.start_at,
                        None => seat.state_since,
                    },
                    None => now,
                };
                waiting_on.push(format!(
                    "{} in review by {name}, {} min in",
                    task.id,
                    (now - since).max(0) / 60
                ));
            }
            graph::Stage::Asking => {
                waiting_on.push(format!("{} {lane} waits for the captain's answer", task.id))
            }
            graph::Stage::Verdict => {
                // The review is in and the verdict is the captain's (T-460):
                // name the item that waits in his inbox, so a finished review
                // is not read as a stall. A passed review leaves no open item;
                // then the plain words stay.
                waiting_on.push(
                    verdict_wait_of(tx, project, &task.id, now)?
                        .unwrap_or_else(|| format!("{} waits for the captain's verdict", task.id)),
                )
            }
            graph::Stage::MergeFailed => {
                waiting_on.push(format!("{} merge failed, waits for the captain", task.id))
            }
            _ => {}
        }
    }

    let voyages = voyage::reports(tx, project, now)?;
    // The decisions belong to the voyage the board measures (F3).
    let decisions = decisions_of(
        tx,
        project,
        workload.as_ref().map(|voyage| voyage.milestone.as_str()),
    )?;
    // The tasks that left the shown voyage (T-351): its `move` decisions.
    let moved_out = moved_out_of(&decisions);
    // The confirmed goal changes are the measured voyage's own (T-335).
    let goal_changes = goal_changes_of(
        tx,
        project,
        workload.as_ref().map(|voyage| voyage.milestone.as_str()),
    )?;
    // The latest of them is the line under the goal box (T-358).
    let goal_changed = goal_changed_of(&goal_changes);
    // The goal check of the shown voyage (T-354): the plan's own clauses, else
    // the clauses of its done sentence, unmet.
    let goal_check = match shown.as_ref() {
        Some(entry) => goal_check_of(tx, project, &entry.milestone, entry.done.as_deref())?,
        None => Vec::new(),
    };
    // The friction reports are the ones of the measured voyage too (T-263).
    let frictions = frictions_of(
        tx,
        project,
        workload.as_ref().map(|voyage| voyage.milestone.as_str()),
    )?;
    // The stations of the measured voyage (T-334): reached, now, next.
    let stations = match &workload {
        Some(voyage) => stations::progress(tx, project, &voyage.milestone)?,
        None => Vec::new(),
    };
    // Each lane's queue played forward (T-349): the measured voyage's forecast,
    // while it has open work. The jobs come from the lanes, which are in plan
    // order; the graph's order is for drawing and is not the queue's.
    let forecast = match &workload {
        Some(voyage)
            if lanes
                .iter()
                .any(|lane| lane.tasks.iter().any(|task| !task.state.is_terminal())) =>
        {
            let jobs: Vec<forecast::Job> = lanes
                .iter()
                .flat_map(|lane| {
                    lane.tasks.iter().map(|task| forecast::Job {
                        id: task.id.clone(),
                        lane: lane.lane.clone(),
                        stage: task.stage,
                        est_min: task.est_min.unwrap_or(0),
                        actual_min: task.actual_min.unwrap_or(0),
                        deps: task.deps.clone(),
                    })
                })
                .collect();
            let names: Vec<String> = lanes.iter().map(|lane| lane.lane.clone()).collect();
            let review_merge = forecast::review_merge_min(tx, project, &voyage.milestone)?;
            forecast::forecast(&jobs, &names, review_merge)
        }
        _ => Vec::new(),
    };
    Ok(ProjectBoard {
        voyages,
        workload,
        project: project.to_string(),
        captain_quota: tx
            .captain_quota(project)?
            .filter(|state| !state.resumed)
            .map(|state| CaptainQuotaBoard {
                reset_at: state.reset_at,
                remaining_s: (state.reset_at - now).max(0),
                awaiting_view: state.wake_op.is_some(),
            }),
        totals,
        lanes,
        deps,
        graph,
        graph_lanes,
        quota: quota_readings(tx, project)?,
        decisions,
        goal_changes,
        goal_changed,
        goal_check,
        frictions,
        // The overlaps with the integration branch (T-188): information
        // lines, never items that wait for the captain.
        overlaps: integration_overlaps_of(tx, project)?,
        waiting,
        temps,
        unconfirmed_deliveries,
        unconfirmed,
        stations,
        forecast,
        waiting_on,
        voyage_list,
        shown,
        moved_out,
    })
}

/// The captain's open item about a task whose review is in and waits for the
/// captain's verdict (T-460): `waits for your verdict (M-<id>)`, with the id of
/// the item that waits in the captain's inbox.
///
/// None when no such item waits — a passed review is information and leaves no
/// item — and when the project has no captain.
pub fn verdict_wait_of(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    now: Ts,
) -> GearResult<Option<String>> {
    let Some(captain) = tx.captain_of(project)? else {
        return Ok(None);
    };
    // The newest open item about the task: the verdict a reviewer handed in
    // (kind `review`), or the engine's word that no reviewer will settle it
    // (`verdict:<task>:<attempt>`, `review-stall:<task>:<attempt>`).
    let item = tx
        .actionable_for(project, &captain.id, now)?
        .into_iter()
        .rev()
        .find(|item| item.task.as_deref() == Some(task) && is_verdict_item(item));
    Ok(item.map(|item| format!("{task} waits for your verdict ({})", item.id)))
}

/// Whether an open item means "the verdict of this attempt is the captain's".
fn is_verdict_item(item: &MessageRow) -> bool {
    item.kind == MsgKind::Review
        || item.thread.starts_with("verdict:")
        || item.thread.starts_with("review-stall:")
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
        let Some(rest) = key.strip_prefix(&prefix) else {
            continue;
        };
        let Some((repo, task)) = split_overlap_key(rest) else {
            continue;
        };
        let Some(raw) = tx.get_meta(&key)? else {
            continue;
        };
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
    overlaps.sort_by(|left, right| (&left.task, &left.repo).cmp(&(&right.task, &right.repo)));
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
                .map(|files| {
                    files
                        .iter()
                        .filter_map(|file| file.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let integration = value
                .get("integration")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            (files, integration)
        }
        Ok(serde_json::Value::Array(files)) => (
            files
                .iter()
                .filter_map(|file| file.as_str().map(str::to_string))
                .collect(),
            None,
        ),
        Ok(serde_json::Value::String(file)) => (vec![file], None),
        // A plain value: the shared files, separated by commas.
        _ => (
            raw.split(',')
                .map(|file| file.trim().to_string())
                .filter(|file| !file.is_empty())
                .collect(),
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
    tx: &Tx<'_>,
    seat: &gear_store::SeatRow,
    agents: Option<&[AgentInfo]>,
    backoff: &[String],
) -> GearResult<&'static str> {
    if backoff.iter().any(|waiting| waiting == &seat.id) {
        return Ok("provider back-off");
    }
    let Some(agents) = agents else {
        return Ok("unknown");
    };
    let Some(binding) = tx
        .get_binding(&seat.id, seat.generation)?
        .filter(|binding| binding.status == BindingStatus::Active)
    else {
        return Ok("unknown");
    };
    let Some(agent) = agents
        .iter()
        .find(|agent| agent.pane_id == binding.pane_id && agent.agent.is_some())
    else {
        return Ok("missing");
    };
    if agent.agent.as_deref() != Some(binding.kind.as_str())
        || binding.session_ref.as_deref().is_some_and(|expected| {
            agent
                .agent_session
                .as_ref()
                .map(|session| session.value.as_str())
                != Some(expected)
        })
    {
        return Ok("unknown");
    }
    Ok(match agent.agent_status {
        AgentStatus::Idle => "idle",
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    })
}

/// Shared snapshot/board wording: agent state and task state have separate labels.
///
/// The current task is the first task of the lane that is on its way between
/// its delivery and its merge, named in the stage words the board's views use
/// (T-347). A cancelled task is never the lane's current task, even when its
/// last attempt was rejected and the classifier still calls that stage open
/// (F1 of the review): the graph leaves a cancelled task out, so the lane line
/// names the lane's next open task instead.
pub fn lane_line(lane: &LaneBoard) -> String {
    let current = lane
        .tasks
        .iter()
        .filter(|task| task.state != TaskState::Cancelled)
        .find(|task| task.stage.is_open());
    let current = current
        .map(|task| {
            let mut text = format!("{} {}", task.id, task.stage_words);
            // A round that could not continue the earlier conversation because
            // its CLI changed (T-481, design/10 §resume rules): the board says
            // so, as the seat's home page does.
            if task.recovery {
                text.push_str(" · new-session recovery");
            }
            text
        })
        .unwrap_or_else(|| match &lane.helping {
            // A lane that helps says what it helps on (T-323).
            Some(helping) => helping.clone(),
            None => "no current task".into(),
        });
    let queued = lane
        .tasks
        .iter()
        .filter(|task| task.state == TaskState::Ready)
        .count();
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
        None => format!(
            "{} · agent {} · {} · {} queued",
            lane.lane, lane.agent_state, current, queued
        ),
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
        lines.push(format!(
            "Captain quota: {status}; new tasks paused, current work/reviews/merges continue"
        ));
    }
    match view {
        View::Lanes => {
            if !project.lanes.is_empty() {
                lines.push(String::new());
                for lane in &project.lanes {
                    lines.push(format!(
                        "Lane {}{}",
                        lane_line(lane),
                        lane_measures(&lane.rework)
                    ));
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

/// The seconds a lane worked on a task (§5.1): the `active_s` of its dev and
/// rework rounds. None when the task has no such round, so a task that was
/// never worked on is not a measured zero (F5).
fn worked_s_of(rounds: &[RoundRow]) -> Option<i64> {
    let mut total = 0;
    let mut worked = false;
    for round in rounds {
        if round.role == "dev" || round.role == "rework" {
            total += round.active_s;
            worked = true;
        }
    }
    worked.then_some(total)
}

/// The minutes the lane worked on a task (§5.1): the sum of the `active_s` of
/// its dev and rework rounds. A task with none of them is measured from its
/// first attempt to its merge, or until now when it is not merged yet.
fn actual_min_of(tx: &Tx<'_>, project: &str, task: &str, now: Ts) -> GearResult<Option<i64>> {
    let rounds = tx.rounds_of_task(project, task)?;
    if let Some(worked) = worked_s_of(&rounds) {
        return Ok(Some(worked / 60));
    }
    let times = tx.attempt_times_of_task(project, task)?;
    let start = times.iter().filter_map(|(_, times)| times.started).min();
    let merged = times.iter().filter_map(|(_, times)| times.merged).max();
    let end = merged.or(Some(now));
    Ok(match (start, end) {
        (Some(start), Some(end)) if end >= start => Some((end - start) / 60),
        _ => None,
    })
}

/// The voyage the Workload view measures: the active one, else the latest
/// departed voyage that is finished (F5).
///
/// The finished voyages are not in `voyages` (the home page and the station
/// watch the active ones only), but their measurements are still true: a
/// landed voyage keeps its burn-up and its lanes.
/// The tokens of a task's rounds (§4.3): the fresh tokens only — input,
/// output and cache write. The cache reads are counted apart
/// ([`cache_read_of`]): a task that re-reads its context for twenty minutes
/// shows millions of them, and a sum that mixed them would say nothing about
/// the work (the user, 2026-10-28). None when a round counted none of them,
/// so a sum is never a total that hides an unknown part (F5).
fn tokens_of(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<Option<i64>> {
    let rounds = tx.rounds_of_task(project, task)?;
    Ok(tokens_of_rounds(rounds.iter()))
}

/// The fresh tokens of the rounds a filter keeps: the body of [`tokens_of`].
fn tokens_of_rounds<'a>(rounds: impl Iterator<Item = &'a RoundRow>) -> Option<i64> {
    let mut total = 0;
    let mut any = false;
    for round in rounds {
        any = true;
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
            return None;
        }
        total +=
            round.input.unwrap_or(0) + round.output.unwrap_or(0) + round.cache_write.unwrap_or(0);
    }
    any.then_some(total)
}

/// The side of the work a round counts for (T-363): a rework is development.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Development,
    Review,
}

/// The side a round's role counts for, when it counts for one.
fn side_of(role: &str) -> Option<Side> {
    match role {
        "dev" | "rework" => Some(Side::Development),
        "review" => Some(Side::Review),
        _ => None,
    }
}

/// The fresh tokens of one side of the task's work (T-363): the rules of
/// [`tokens_of`] over its development rounds or its review rounds only.
fn side_tokens_of(tx: &Tx<'_>, project: &str, task: &str, side: Side) -> GearResult<Option<i64>> {
    let rounds = tx.rounds_of_task(project, task)?;
    Ok(tokens_of_rounds(
        rounds
            .iter()
            .filter(|round| side_of(&round.role) == Some(side)),
    ))
}

/// The cache reads of a task's rounds (§4.3), counted apart from the fresh
/// tokens: None when a round did not count its cache reads, so the sum is
/// never a total that hides an unknown part (F5).
fn cache_read_of(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<Option<i64>> {
    let rounds = tx.rounds_of_task(project, task)?;
    let mut total = 0;
    for round in &rounds {
        // A round that counted no cache reads is unknown, not a measured
        // zero: the field's presence says what was counted (F5).
        let Some(cache_read) = round.cache_read else {
            return Ok(None);
        };
        total += cache_read;
    }
    Ok((!rounds.is_empty()).then_some(total))
}

/// The cost of a task's rounds (§4.4): None when a round priced none of them,
/// so a sum is never a total that hides an unknown part (F5).
fn cost_of(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<Option<f64>> {
    let rounds = tx.rounds_of_task(project, task)?;
    Ok(cost_of_rounds(rounds.iter()))
}

/// The cost of the rounds a filter keeps: the body of [`cost_of`].
fn cost_of_rounds<'a>(rounds: impl Iterator<Item = &'a RoundRow>) -> Option<f64> {
    let mut total = 0.0;
    let mut any = false;
    for round in rounds {
        any = true;
        // A round that was never priced is unknown, not a price of zero:
        // the sum that hides it is not a total (F5 of the T-166 review).
        let Some(cost) = round.cost_usd else {
            return None;
        };
        total += cost;
    }
    any.then_some(total)
}

/// The cost of one side of the task's work (T-363): the rules of [`cost_of`]
/// over its development rounds or its review rounds only.
fn side_cost_of(tx: &Tx<'_>, project: &str, task: &str, side: Side) -> GearResult<Option<f64>> {
    let rounds = tx.rounds_of_task(project, task)?;
    Ok(cost_of_rounds(
        rounds
            .iter()
            .filter(|round| side_of(&round.role) == Some(side)),
    ))
}

/// The captain's decisions of the voyage, as the board shows them (§7.1).
fn decisions_of(
    tx: &Tx<'_>,
    project: &str,
    voyage: Option<&str>,
) -> GearResult<Vec<DecisionBoard>> {
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

/// The tasks that left the shown voyage for another (T-351): the voyage's
/// decisions of kind `move`, whose text is "<id> moved to <milestone>", in the
/// order they were recorded.
fn moved_out_of(decisions: &[DecisionBoard]) -> Vec<(String, String)> {
    decisions
        .iter()
        .filter(|decision| decision.kind == "move")
        .filter_map(|decision| {
            let (task, to) = decision.what.split_once(" moved to ")?;
            Some((task.to_string(), to.to_string()))
        })
        .collect()
}

/// The confirmed goal changes of the voyage (T-335): the decisions of kind
/// `user` with source `gear` (T-410, T-331), oldest first.
fn goal_changes_of(
    tx: &Tx<'_>,
    project: &str,
    voyage: Option<&str>,
) -> GearResult<Vec<GoalChangeBoard>> {
    let Some(voyage) = voyage else {
        return Ok(Vec::new());
    };
    Ok(tx
        .decisions_of(project, voyage)?
        .into_iter()
        .filter(|decision| decision.kind == "user" && decision.source == "gear")
        .map(|decision| GoalChangeBoard {
            at: decision.at,
            what: decision.what,
            why: decision.why,
        })
        .collect())
}

/// The latest confirmed goal change of the voyage (T-358), as (time, before,
/// after): its words come from the decision T-410 (C6) records,
/// `<field> of <M> changed from "<old>" to "<new>"`. `None` without a goal
/// change, or when its words are not that record's.
fn goal_changed_of(changes: &[GoalChangeBoard]) -> Option<(String, String, String)> {
    let change = changes.last()?;
    let (_, rest) = change.what.split_once(" changed from \"")?;
    let (before, rest) = rest.split_once("\" to \"")?;
    let (after, _) = rest.split_once('"')?;
    Some((change.at.clone(), before.to_string(), after.to_string()))
}

/// The goal check of a voyage (T-354): the meta `goal-check:<project>:<milestone>`
/// holds the plan's clauses as a JSON array of `{"met": bool, "text": str}`.
/// Without it the clauses of the voyage's done sentence stand in, every one
/// unmet: the store that keeps no check knows of no clause that is met.
fn goal_check_of(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
    done: Option<&str>,
) -> GearResult<Vec<(bool, String)>> {
    if let Some(raw) = tx.get_meta(&format!("goal-check:{project}:{milestone}"))? {
        if let Some(check) = parse_goal_check(&raw) {
            return Ok(check);
        }
    }
    Ok(done_clauses(done)
        .into_iter()
        .map(|text| (false, text))
        .collect())
}

/// The clauses of a goal check written in the meta: a JSON array of
/// `{"met": bool, "text": str}`. `None` when the text is not one.
fn parse_goal_check(raw: &str) -> Option<Vec<(bool, String)>> {
    let clauses: Vec<serde_json::Value> = serde_json::from_str(raw).ok()?;
    let mut check = Vec::with_capacity(clauses.len());
    for clause in clauses {
        let met = clause.get("met")?.as_bool()?;
        let text = clause.get("text")?.as_str()?.to_string();
        check.push((met, text));
    }
    Some(check)
}

/// The clauses of a done sentence: split on `;`, trimmed, empty ones left out.
pub(crate) fn done_clauses(done: Option<&str>) -> Vec<String> {
    let Some(done) = done else {
        return Vec::new();
    };
    done.split(';')
        .map(|clause| clause.trim().to_string())
        .filter(|clause| !clause.is_empty())
        .collect()
}

/// The friction reports of the voyage, as the board shows them (§6.1): the
/// open ones and the closed ones, oldest first.
///
/// A friction belongs to the voyage of the task it names: with no measured
/// voyage, none (T-263).
fn frictions_of(
    tx: &Tx<'_>,
    project: &str,
    voyage: Option<&str>,
) -> GearResult<Vec<FrictionBoard>> {
    let Some(voyage) = voyage else {
        return Ok(Vec::new());
    };
    let mut frictions = Vec::new();
    for friction in tx.frictions_of_project(project)? {
        // The task of the report: a friction of a task of another voyage is
        // history, not the measured voyage's.
        let Some(task) = tx.get_task(project, &friction.task)? else {
            continue;
        };
        if task.milestone.as_deref() != Some(voyage) {
            continue;
        }
        frictions.push(FrictionBoard {
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
        });
    }
    Ok(frictions)
}

/// One row per voyage of the project (T-350): its milestone, its title,
/// whether it still sails, and — once landed — when its last task was merged.
fn voyage_entries(tx: &Tx<'_>, project: &str, tasks: &[TaskRow]) -> GearResult<Vec<VoyageEntry>> {
    let mut entries = Vec::new();
    for voyage in tx.voyages_of_project(project)? {
        let of_voyage = || {
            tasks
                .iter()
                .filter(|task| task.milestone.as_deref() == Some(voyage.milestone.as_str()))
        };
        // Sailing while some task of it is not settled: a done, failed or
        // cancelled task is no work left.
        let sailing = of_voyage().any(|task| {
            !matches!(
                task.state,
                TaskState::Done | TaskState::Failed | TaskState::Cancelled
            )
        });
        // A landed voyage ended at the last merge of its tasks.
        let mut ended: Option<Ts> = None;
        if !sailing {
            for task in of_voyage() {
                for (_, times) in tx.attempt_times_of_task(project, &task.id)? {
                    if let Some(merged) = times.merged {
                        ended = Some(ended.map_or(merged, |last| last.max(merged)));
                    }
                }
            }
        }
        entries.push(VoyageEntry {
            milestone: voyage.milestone,
            title: voyage.title,
            done: voyage.done,
            bar: voyage.bar,
            sailing,
            departure: voyage.departure,
            ended,
        });
    }
    Ok(entries)
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
    let mut finished: Vec<voyage::VoyageReport> =
        voyage::reports_including_finished(tx, project, now)?
            .into_iter()
            .filter(|report| !active.iter().any(|one| one.milestone == report.milestone))
            // A departed voyage: its elapsed time is known.
            .filter(|report| report.elapsed_min.is_some())
            .collect();
    // The latest departure is the one with the least elapsed time.
    finished.sort_by_key(|report| report.elapsed_min.unwrap_or(i64::MAX));
    // With no voyage sailing, the board shows the latest voyage the plan
    // described (T-441): a milestone the plan never described has no done
    // sentence, and showing it would show a voyage with no goal. A project
    // with only such milestones still shows one: today's choice, the latest
    // departure.
    let described = finished.iter().find(|report| {
        report
            .done
            .as_deref()
            .is_some_and(|done| !done.trim().is_empty())
    });
    Ok(match described {
        Some(report) => Some(report.clone()),
        None => finished.into_iter().next(),
    })
}

/// The quota readings of the providers (§9.1): the meta keys
/// `quota.reading.<provider>`.
///
/// Only the providers the project reads now are shown (T-383): a reading left
/// in the meta by an earlier config is not one of them. A config that does
/// not parse does not filter: the readings are shown rather than hidden.
fn quota_readings(tx: &Tx<'_>, project: &str) -> GearResult<Vec<QuotaReading>> {
    let wanted = quota_providers(tx, project)?;
    let mut readings = Vec::new();
    for key in tx.meta_keys_with_prefix("quota.reading.")? {
        let Some(raw) = tx.get_meta(&key)? else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let provider = value
            .get("provider")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| key.trim_start_matches("quota.reading."))
            .to_string();
        if let Some(wanted) = &wanted {
            if !wanted.contains(&provider) {
                continue;
            }
        }
        let mut windows = Vec::new();
        for window in value
            .get("windows")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&Vec::new())
        {
            let Some(name) = window.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
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
            provider,
            windows,
            read_at: value
                .get("read_at")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
        });
    }
    Ok(readings)
}

/// The providers the project's stored config reads (§9.1): the `provider` of
/// every `[[quota.sources]]`.
///
/// None when the config does not parse or the project is not there: the
/// filter is skipped then, and every reading stays.
fn quota_providers(tx: &Tx<'_>, project: &str) -> GearResult<Option<Vec<String>>> {
    let Some(row) = tx.get_project(project)? else {
        return Ok(None);
    };
    let Ok(config) = row.config_toml.parse::<toml::Value>() else {
        return Ok(None);
    };
    let mut providers = Vec::new();
    for source in config
        .get("quota")
        .and_then(|quota| quota.get("sources"))
        .and_then(toml::Value::as_array)
        .unwrap_or(&Vec::new())
    {
        if let Some(provider) = source.get("provider").and_then(toml::Value::as_str) {
            providers.push(provider.to_string());
        }
    }
    Ok(Some(providers))
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
        None => format!(
            " · rework {}/{} · no task yet",
            rework.rejected, rework.handed_in
        ),
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
                        "tokens": task.tokens,
                        "cache_read": task.cache_read,
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
        GearError::new(
            ErrorCode::BadFormat,
            format!("cannot serialize the board: {e}"),
        )
    })
}

/// Show the board until the user quits (`q`).
///
/// Without a terminal — a pipe, a test — the board is printed once and the
/// command returns: a full-screen view must not hang a caller that cannot
/// see it. [`run_with`] draws the frames.
pub fn run(
    store: &mut Store,
    opts: &Options,
    out: &mut dyn Write,
    mut herdr: Option<&mut dyn HerdrRead>,
) -> GearResult<()> {
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
/// `Enter` opens the panel of the selected row. The mouse wheel moves the
/// selection three picks a notch and the page keys a screenful (T-411); the
/// viewport follows the selection in each case.
///
/// A frame is built only when something it shows can have changed (T-384): the
/// store's `data_version` moved, Herdr's agent list moved, or the kept board's
/// clock got old ([`REBUILD_AFTER`]). Every other frame redraws the board that
/// was built last, and Herdr is asked for its agents at most every
/// [`AGENTS_EVERY`]. A frame that reads like the last one written is not
/// written at all (T-403): the screen keeps it.
pub fn run_with(
    store: &mut Store,
    opts: &Options,
    out: &mut dyn Write,
    keys: &mut dyn Keys,
    mut herdr: Option<&mut dyn HerdrRead>,
) -> GearResult<()> {
    let mut screen = frame::Screen {
        tab: tab_of(opts.view),
        selected: 0,
        detail: false,
        opened: Vec::new(),
        size: keys.size(),
    };
    let mut panel: Option<Vec<style::Line>> = None;
    // The project the board shows when it was opened without one (T-317):
    // the store is the session's, so its projects are the session's, and `p`
    // walks through them.
    let mut shown = 0usize;
    let mut first = true;
    // The voyage the whole board shows (T-357): the one the options name, else
    // the measured one. `[`, `]` and a click on a label pick another, and every
    // frame is built with the pick.
    let mut milestone: Option<String> = opts.milestone.clone();
    // The board that was built last, and what it was built from (T-384). The
    // whole board is kept: a refresh that finds nothing changed redraws it
    // instead of building it again, and a key that only moves the selection,
    // the tab or the scrolling costs a redraw and nothing else.
    let mut kept: Option<Board> = None;
    // The store's `data_version` the kept board was built on. It moves when
    // another connection commits, and nothing else of the store can change
    // what the board shows without moving it.
    let mut kept_version: Option<i64> = None;
    let mut built_at = std::time::Instant::now();
    // Herdr's agents, asked for at most every [`AGENTS_EVERY`] and kept in
    // between.
    let mut agents: Option<Vec<AgentInfo>> = None;
    let mut agents_at: Option<std::time::Instant> = None;
    // The frame that was written last (T-403). A frame that says what that one
    // said is not written at all: the board redraws in place, and rewriting
    // the same text only makes the page blink.
    let mut written: Option<String> = None;
    loop {
        let at = std::time::Instant::now();
        let mut agents_changed = false;
        if let Some(herdr) = herdr.as_mut() {
            if agents_at.is_none_or(|asked| at.duration_since(asked) >= hook::agents_every()) {
                let asked = herdr.agents().ok();
                agents_at = Some(at);
                agents_changed = asked != agents;
                agents = asked;
            }
        }
        // One cheap query decides whether anything the board shows can have
        // changed: the store moved, the agent list moved, or the kept board's
        // clock and its age/deadline texts are too old to be trusted.
        let version = store.read(|tx| tx.data_version()).ok();
        let stale = kept.is_none()
            || agents_changed
            || version != kept_version
            || at.duration_since(built_at) >= hook::stale_after();
        if stale {
            let shown_opts = Options {
                milestone: milestone.clone(),
                ..opts.clone()
            };
            match build_with_agents(store, &shown_opts, now_ts(), agents.as_deref()) {
                Ok(mut board) => {
                    // The frame draws the shown project: the project the board
                    // was opened with is the only one then, and every other
                    // project is one `p` away. `shown` is the index into the
                    // project list, kept as one (T-384 rework, F1): a rebuild
                    // against another count starts from it, so a cycle back to
                    // the first project stays there.
                    if opts.project.is_none() && !board.projects.is_empty() {
                        let count = board.projects.len();
                        shown %= count;
                        board.projects.rotate_left(shown);
                    }
                    kept = Some(board);
                    kept_version = version;
                    built_at = std::time::Instant::now();
                    hook::built();
                }
                Err(e) => {
                    // The first frame decides whether the board can start at all.
                    kept = None;
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
                    written = Some(text);
                    let deadline = std::time::Instant::now() + opts.refresh;
                    let _ = keys
                        .next_event(deadline.saturating_duration_since(std::time::Instant::now()));
                    continue;
                }
            }
        }
        // A resize has thrown away what the screen showed (T-403): the frame
        // is drawn once, over a full clear, even when it says what the last
        // one said. Nothing else clears the screen.
        let size = keys.size();
        let resized = size != screen.size;
        screen.size = size;
        // The selection moves over the rows the view paints as picks, and
        // never leaves them (F4): a view that shrank, or a key pressed at its
        // end, does not lose the selection.
        // The head of the board says where the tabs and the view are (T-353):
        // the box of the goal and the rows of the header grow and shrink with
        // the voyage's words.
        let layout = {
            let board = kept.as_ref().expect("the board was just built or kept");
            frame::layout(board, &screen)
        };
        let mut tall = frame::Canvas::new(screen.size.0, 1000);
        let picks = {
            let board = kept.as_ref().expect("the board was just built or kept");
            frame::view(board, &screen, &mut tall, layout.top)
        };
        screen.selected = screen.selected.min(picks.len().saturating_sub(1));
        let text = {
            let board = kept.as_ref().expect("the board was just built or kept");
            frame::canvas(board, &screen, panel.as_deref()).ansi(opts.color)
        };
        first = false;
        if resized {
            out.write_all(b"\x1b[2J").map_err(write_error)?;
            written = None;
        }
        if written.as_deref() != Some(text.as_str()) {
            screen::draw(out, &text).map_err(write_error)?;
            written = Some(text);
        }
        let deadline = std::time::Instant::now() + opts.refresh;
        // The picks one page holds (T-411): the rows of the view the screen
        // shows, the header and the footer's rows taken out, and never less
        // than one.
        let page = screen.size.1.saturating_sub(layout.top + 2).max(1);
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            let Some(event) = keys.next_event(deadline - now) else {
                continue;
            };
            if panel.is_some() {
                // Any key closes the panel (the prototype): the board's own
                // keys come after it.
                panel = None;
                break;
            }
            match event {
                Event::Key('q') | Event::Key('Q') | Event::Key('\u{3}') => return Ok(()),
                Event::Right => {
                    screen.tab = screen.tab.next();
                    screen.selected = 0;
                    panel = None;
                    break;
                }
                Event::Left => {
                    screen.tab = screen.tab.previous();
                    screen.selected = 0;
                    panel = None;
                    break;
                }
                Event::Key('\t') => {
                    screen.tab = screen.tab.next();
                    screen.selected = 0;
                    panel = None;
                    break;
                }
                Event::BackTab => {
                    screen.tab = screen.tab.previous();
                    screen.selected = 0;
                    panel = None;
                    break;
                }
                Event::Key(character) if character.is_ascii_digit() => {
                    if let Some(picked) = Tab::of_number(character as u8 - b'0') {
                        screen.tab = picked;
                        screen.selected = 0;
                        panel = None;
                        break;
                    }
                }
                Event::Key('p') | Event::Key('P') => {
                    // The next project of the session (T-317): the selection
                    // and the panel belong to the project that was shown. The
                    // kept board is walked on, so the next project is shown
                    // without building the board again.
                    shown += 1;
                    if let Some(board) = kept.as_mut() {
                        let count = board.projects.len();
                        if count > 1 {
                            board.projects.rotate_left(1);
                        }
                        // `shown` names the project shown, not the presses
                        // since the board opened (T-384 rework, F1): kept
                        // unnormalized, a full cycle would move the board to
                        // another project when the count grows.
                        if count > 0 {
                            shown %= count;
                        }
                    }
                    screen.selected = 0;
                    panel = None;
                    break;
                }
                // `[` and `]` pick the voyage the whole board shows (T-357):
                // the list is newest first, so `[` goes one entry older and `]`
                // one newer, and the oldest and the newest voyage stay where
                // they are.
                Event::Key(character @ ('[' | ']')) => {
                    let picked = kept.as_ref().and_then(|board| {
                        let project = board.projects.first()?;
                        let shown = project.shown.as_ref()?.milestone.as_str();
                        let index = project
                            .voyage_list
                            .iter()
                            .position(|entry| entry.milestone == shown)?;
                        let next = if character == '[' {
                            (index + 1).min(project.voyage_list.len() - 1)
                        } else {
                            index.saturating_sub(1)
                        };
                        project
                            .voyage_list
                            .get(next)
                            .map(|entry| entry.milestone.clone())
                    });
                    if let Some(picked) = picked {
                        milestone = Some(picked);
                        screen.selected = 0;
                        panel = None;
                        // The kept board shows another voyage: the next frame
                        // is built for the pick.
                        kept = None;
                        break;
                    }
                }
                // The `d` key unfolds the header and folds it again (T-354):
                // the tab and the selection stay where they were.
                Event::Key('d') | Event::Key('D') => {
                    screen.detail = !screen.detail;
                    panel = None;
                    break;
                }
                Event::Up | Event::Key('k') => {
                    screen.selected = screen.selected.saturating_sub(1);
                    panel = None;
                    break;
                }
                Event::Down | Event::Key('j') => {
                    screen.selected = (screen.selected + 1).min(picks.len().saturating_sub(1));
                    panel = None;
                    break;
                }
                // The mouse wheel (T-411): three picks a notch, and never a
                // click. It closes the panel as an arrow does.
                Event::Mouse { button: 64, .. } => {
                    screen.selected = screen.selected.saturating_sub(WHEEL_PICKS);
                    panel = None;
                    break;
                }
                Event::Mouse { button: 65, .. } => {
                    screen.selected =
                        (screen.selected + WHEEL_PICKS).min(picks.len().saturating_sub(1));
                    panel = None;
                    break;
                }
                // The page keys move the selection by a screenful (T-411).
                // The viewport follows it as it follows an arrow's.
                Event::PageUp => {
                    screen.selected = screen.selected.saturating_sub(page);
                    panel = None;
                    break;
                }
                Event::PageDown => {
                    screen.selected = (screen.selected + page).min(picks.len().saturating_sub(1));
                    panel = None;
                    break;
                }
                Event::Key('\r') | Event::Key('\n') => {
                    let board = kept.as_ref().expect("the board was just built or kept");
                    // The Cost view opens a lane's tasks instead of a panel
                    // (T-363): `Enter` on a lane shows them under it, and on
                    // the same lane hides them again.
                    if screen.tab == frame::Tab::Cost {
                        if let Some(lane) = board
                            .projects
                            .first()
                            .and_then(|p| p.lanes.get(screen.selected))
                        {
                            match screen.opened.iter().position(|open| *open == lane.lane) {
                                Some(index) => {
                                    screen.opened.remove(index);
                                }
                                None => screen.opened.push(lane.lane.clone()),
                            }
                        }
                        panel = None;
                        break;
                    }
                    panel = open_panel(board, store, screen.tab, screen.selected, screen.size.0)?;
                    break;
                }
                Event::Mouse { button: 0, x, y } => {
                    // A click on the tab row picks a view; a click on a row of
                    // the view picks that row. The rows are the ones the
                    // frame draws: the tabs on their own row, the data below
                    // the heading. A click at the terminal's (x, y), counted
                    // from 1, is the screen's column x - 1 and row y - 1
                    // (T-353).
                    let column = x.saturating_sub(1) as usize;
                    let row = y.saturating_sub(1) as usize;
                    if row == 0 {
                        // A click on a voyage's label on the first row picks
                        // that voyage (T-357): the whole board shows it.
                        let picked = layout
                            .voyages
                            .iter()
                            .find(|(first, last, _)| column >= *first && column <= *last)
                            .map(|(_, _, named)| named.clone());
                        if let Some(picked) = picked {
                            milestone = Some(picked);
                            screen.selected = 0;
                            panel = None;
                            // The kept board shows another voyage: the next
                            // frame is built for the pick.
                            kept = None;
                            break;
                        }
                    } else if row == layout.tab_row {
                        let picked = layout
                            .tabs
                            .iter()
                            .find(|(first, last, _)| column >= *first && column <= *last)
                            .map(|(_, _, tab)| *tab);
                        if let Some(picked) = picked {
                            screen.tab = picked;
                            screen.selected = 0;
                            panel = None;
                            break;
                        }
                    } else {
                        // Only the rows the frame draws are rows of the view
                        // (F8): the click is on the screen row, the spacer
                        // below the data and the footer's row select nothing.
                        // The view's first row is a row of the view (T-342's
                        // rework): scrolling can put a pick there.
                        let last = frame::view_bottom(screen.tab, screen.size.1);
                        if row >= layout.top && row < last {
                            // The rows above the viewport are scrolled away:
                            // the clicked row is the one the frame drew.
                            let offset =
                                frame::offset_of(&picks, screen.selected, layout.top, last);
                            // A click selects the pick drawn on that row; a
                            // row without a pick selects nothing.
                            if let Some(index) = picks.iter().position(|pick| *pick == row + offset)
                            {
                                screen.selected = index;
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
        View::Deps | View::Graph => frame::Tab::Tasks,
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
    width: usize,
) -> GearResult<Option<Vec<style::Line>>> {
    let Some(project) = board.projects.first() else {
        return Ok(None);
    };
    match tab {
        frame::Tab::Now => Ok(project.lanes.get(selected).map(|lane| {
            frame::styled_lane_panel(
                lane,
                &project.graph,
                frame::lane_color_of(project, &lane.lane),
            )
        })),
        frame::Tab::Tasks => {
            // The task of the selected pick: the row the graph draws for it.
            let picks = frame::task_picks(project, board.now, width);
            let Some(task) = picks.get(selected) else {
                return Ok(None);
            };
            let rows =
                store.read(|tx| graph::styled_panel(tx, &project.project, task, now_ts()))?;
            Ok(Some(rows))
        }
        frame::Tab::Cost => {
            // The Cost view opens no panel (T-363): `Enter` on a lane shows
            // its tasks under the lane's row.
            Ok(None)
        }
        frame::Tab::Log => Ok(None),
    }
}

/// Print the board once: the JSON or the text, depending on the options.
///
/// The text is one static frame: nothing can be pressed after it, so it is the
/// frame of one project even when the store holds several — the project
/// switch of the interactive board (`p`, T-317) is not mentioned, exactly as
/// before the board could switch at all (review of T-317, F1).
fn print_once(board: &Board, opts: &Options, out: &mut dyn Write) -> GearResult<()> {
    if opts.json {
        writeln!(out, "{}", to_json(board)?).map_err(write_error)?;
    } else {
        let size = (screen::DEFAULT_WIDTH, screen::DEFAULT_ROWS);
        // Every view of the frame reads the first project, so a board of one
        // project draws what it always drew.
        let mut one = board.clone();
        one.projects.truncate(1);
        writeln!(
            out,
            "{}",
            frame::frame(
                &one,
                &frame::Screen {
                    tab: tab_of(opts.view),
                    selected: 0,
                    detail: false,
                    opened: Vec::new(),
                    size,
                },
                None
            )
        )
        .map_err(write_error)?;
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
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.try_lock().unwrap();
        assert_eq!(EngineState::probe(Some(&db)), EngineState::Running);
    }

    #[test]
    fn a_missing_database_path_means_the_engine_is_down() {
        assert_eq!(EngineState::probe(None), EngineState::Down);
    }
}
