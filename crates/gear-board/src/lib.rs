//! The GEAR plan board (PRD §15).
//!
//! The board is what the user and the captain look at: **the plan, not the
//! agents**. It shows the totals, every lane's queue with the state of each
//! task, the items waiting for the captain and the temp slots — and one
//! warning line, only when the engine is down.
//!
//! It says nothing about liveness: Herdr's own panel shows whether an agent
//! is working or idle, so the board has no agent state and no heartbeat
//! (PRD §15). It also never writes: the plan is read, not touched.
//!
//! This crate is a library. It builds the board from the store and draws it
//! on a terminal; `gear-cli` calls it from the `gear board` subcommand.

mod screen;

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, TaskState, Ts};
use gear_store::{Store, Tx};

pub use screen::Keys;

/// How long a frame stays on the screen before the board redraws it.
pub const REFRESH: Duration = Duration::from_secs(2);

/// The lane name of a task that belongs to no lane.
const UNASSIGNED: &str = "unassigned";

/// The one warning line the board may print.
const ENGINE_DOWN: &str = "! engine down — the next gear call starts it";

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
    /// How long a frame stays on the screen.
    pub refresh: Duration,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            project: None,
            db: None,
            json: false,
            refresh: REFRESH,
        }
    }
}

/// The board: the plan of every project that is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    /// The engine's liveness; the board warns only when it is down.
    pub engine: EngineState,
    /// One block per project.
    pub projects: Vec<ProjectBoard>,
}

/// The plan of one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectBoard {
    pub project: String,
    pub totals: Totals,
    /// The lanes that have work, in seat order; a lane a task names but no
    /// seat carries comes last.
    pub lanes: Vec<LaneBoard>,
    /// The items waiting for the captain.
    pub waiting: Waiting,
    /// The temp slots of the project.
    pub temps: Temps,
}

/// The counts of the whole plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub tasks: usize,
    pub done: usize,
    pub in_progress: usize,
    pub waiting: usize,
}

/// One lane and its queue, in plan order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneBoard {
    pub lane: String,
    pub tasks: Vec<TaskBoard>,
}

/// One task of a lane's queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBoard {
    pub id: String,
    pub state: TaskState,
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
    let engine = EngineState::probe(opts.db.as_deref());
    let projects = store.read(|tx| {
        let ids = match &opts.project {
            Some(id) => vec![id.clone()],
            None => tx.project_ids()?,
        };
        ids.iter().map(|id| project_board(tx, id, now)).collect()
    })?;
    Ok(Board { engine, projects })
}

/// The plan of one project, as the board shows it.
fn project_board(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<ProjectBoard> {
    let row = tx.get_project(project)?.ok_or_else(|| {
        GearError::new(ErrorCode::NoTarget, format!("project {project} is not registered"))
    })?;
    let seats = tx.seats_of_project(project)?;
    let tasks = tx.tasks_of_project(project)?;

    // The lanes: the registered ones first (seat order), then a lane that a
    // task names but no seat carries.
    let mut order: Vec<String> = Vec::new();
    for seat in &seats {
        if seat.role == SeatRole::Lane && !order.contains(&seat.name) {
            order.push(seat.name.clone());
        }
    }
    let mut lanes: Vec<LaneBoard> = order
        .into_iter()
        .map(|lane| LaneBoard { lane, tasks: Vec::new() })
        .collect();

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
        let line = TaskBoard { id: task.id.clone(), state: task.state };
        match lanes.iter_mut().find(|lane| lane.lane == name) {
            Some(lane) => lane.tasks.push(line),
            None => lanes.push(LaneBoard { lane: name, tasks: vec![line] }),
        }
    }
    // A lane without a task has nothing to show: the board shows the plan.
    lanes.retain(|lane| !lane.tasks.is_empty());

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

    let temps = Temps {
        in_use: seats
            .iter()
            .filter(|seat| seat.role == SeatRole::Temp && holds_temp_slot(seat.state))
            .count(),
        quota: row.quota_temps.max(0) as usize,
    };

    Ok(ProjectBoard { project: project.to_string(), totals, lanes, waiting, temps })
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
pub fn render(board: &Board) -> String {
    let mut head = vec!["GEAR board".to_string()];
    if board.engine == EngineState::Down {
        head.push(ENGINE_DOWN.to_string());
    }
    let mut blocks: Vec<Vec<String>> = vec![head];
    for project in &board.projects {
        blocks.push(project_lines(project));
    }
    blocks
        .iter()
        .map(|block| block.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The lines of one project block.
fn project_lines(project: &ProjectBoard) -> Vec<String> {
    let mut lines = vec![
        format!("project {}", project.project),
        format!(
            "Totals: {} tasks · {} done · {} in progress · {} waiting",
            project.totals.tasks,
            project.totals.done,
            project.totals.in_progress,
            project.totals.waiting
        ),
    ];
    if !project.lanes.is_empty() {
        lines.push(String::new());
        for lane in &project.lanes {
            lines.push(format!("Lane {}", lane.lane));
            for task in &lane.tasks {
                lines.push(format!("  {} {}", task.id, state_word(task.state)));
            }
        }
    }
    lines.push(String::new());
    lines.push(waiting_line(&project.waiting));
    lines.push(format!(
        "Temp slots: {}/{} in use",
        project.temps.in_use, project.temps.quota
    ));
    lines
}

/// The line about the captain's inbox.
fn waiting_line(waiting: &Waiting) -> String {
    match waiting.oldest_age_s {
        Some(age) => format!(
            "Waiting for the captain: {} items · oldest {}",
            waiting.count,
            human_age(age)
        ),
        None => format!("Waiting for the captain: {} items", waiting.count),
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

/// The board's word for a task state (PRD §15): the states of the plan, plus
/// the two ways a task can end without a result.
fn state_word(state: TaskState) -> &'static str {
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
                "totals": {
                    "tasks": project.totals.tasks,
                    "done": project.totals.done,
                    "in_progress": project.totals.in_progress,
                    "waiting": project.totals.waiting,
                },
                "lanes": project.lanes.iter().map(|lane| serde_json::json!({
                    "lane": lane.lane,
                    "tasks": lane.tasks.iter().map(|task| serde_json::json!({
                        "id": task.id,
                        "state": state_word(task.state),
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "captain_waiting": {
                    "count": project.waiting.count,
                    "oldest_age_s": project.waiting.oldest_age_s,
                },
                "temps": {
                    "in_use": project.temps.in_use,
                    "quota": project.temps.quota,
                },
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
/// The board is a full-screen view: it redraws every [`Options::refresh`]
/// seconds and `q` leaves it. Without a terminal — a pipe, a test — the
/// board is printed once and the command returns: a full-screen view must not
/// hang a caller that cannot see it.
pub fn run(store: &mut Store, opts: &Options, out: &mut dyn Write) -> GearResult<()> {
    if opts.json || !screen::is_terminal() {
        let board = build(store, opts, now_ts())?;
        return print_once(&board, opts, out);
    }
    let mut keys = screen::Terminal::new();
    screen::enter(out).map_err(write_error)?;
    let result = run_with(store, opts, out, &mut keys);
    // The terminal is given back even when the board failed.
    let _ = screen::leave(out);
    result
}

/// The frames over injected keys: the board is redrawn every
/// [`Options::refresh`], and `q` leaves it.
pub fn run_with(
    store: &mut Store,
    opts: &Options,
    out: &mut dyn Write,
    keys: &mut dyn Keys,
) -> GearResult<()> {
    let mut first = true;
    loop {
        let text = match build(store, opts, now_ts()) {
            Ok(board) => render(&board),
            Err(e) => {
                // The first frame decides whether the board can start at all.
                if first {
                    return Err(e);
                }
                // A frame that cannot be read is shown as the failure it is:
                // an observation failure is not an all-clear (17 §5).
                format!("GEAR board\n\n! the plan could not be read: {}: {}", e.code, e.detail)
            }
        };
        first = false;
        screen::draw(out, &text).map_err(write_error)?;
        let deadline = std::time::Instant::now() + opts.refresh;
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            if let Some(key) = keys.next_key(deadline - now) {
                if is_quit(key) {
                    return Ok(());
                }
            }
        }
    }
}

/// `q` leaves the board. Ctrl-C leaves it too: in raw mode there is no
/// SIGINT, so the key itself has to end the view.
fn is_quit(key: char) -> bool {
    key == 'q' || key == 'Q' || key == '\u{3}'
}

/// Print the board once: the JSON or the text, depending on the options.
fn print_once(board: &Board, opts: &Options, out: &mut dyn Write) -> GearResult<()> {
    if opts.json {
        writeln!(out, "{}", to_json(board)?).map_err(write_error)?;
    } else {
        writeln!(out, "{}", render(board)).map_err(write_error)?;
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
