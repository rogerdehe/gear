//! GEAR SQLite store (design/02).
//!
//! Responsibilities: open the single per-host database, apply the schema,
//! run short `BEGIN IMMEDIATE` transactions, allocate ids, replay requests,
//! and read/write rows. **No business rules live here**: callers (gear-app,
//! gear-engine) decide what to write and check transitions with gear-model.
//!
//! Every method on [`Tx`] runs inside the caller's transaction. Never call
//! Herdr or git while holding a [`Tx`].

use gear_model::{
    AttemptState, ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, OpState, SeatRole, SeatState, TaskState, Ts,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::path::Path;
use std::time::{Duration, Instant};

mod migrate;

pub const SCHEMA_SQL: &str = include_str!("schema.sql");
pub use migrate::{OLDEST_VERSION, SCHEMA_VERSION, STEPS};
/// SQLite busy timeout for every connection. A caller waits for the store
/// rather than failing with `Locked` (T-379, F-129): a `gear` call queues
/// behind a slow writer instead of giving up after five seconds.
pub const BUSY_TIMEOUT_MS: u64 = 30_000;

/// A write that holds the store longer than this prints one line to stderr,
/// naming the caller that held it (T-379). The engine's stderr is its log.
pub const SLOW_WRITE_MS: u64 = 1_000;

/// How many prepared statements one connection keeps (T-385). A tick or a
/// board frame runs the same queries again and again; the cache holds their
/// statements so SQLite does not parse the text every time.
pub const STATEMENT_CACHE_CAPACITY: usize = 256;

/// The line [`Store::write`] prints when it held the store over
/// [`SLOW_WRITE_MS`]: the elapsed time and the caller's location.
pub fn slow_write_line(ms: u128, caller: &'static std::panic::Location<'static>) -> String {
    format!("gear-store: a write held the store for {ms} ms ({caller})")
}

/// The budget of a round whose task has no estimate (§5.1). The design
/// names the estimate for a dev round and the measured median for a review
/// round; a task without an estimate has none to read, so the store falls
/// back to the same default a review round without measurements gets.
pub const DEFAULT_ROUND_BUDGET_MIN: i64 = 30;

/// The budget of a review round with fewer than three finished review rounds
/// of the same model to measure (§5.1).
pub const DEFAULT_REVIEW_BUDGET_MIN: i64 = 30;

pub struct Store {
    conn: rusqlite::Connection,
}

/// A write (or read) transaction. Dropped without `commit` => rolled back.
pub struct Tx<'s> {
    tx: rusqlite::Transaction<'s>,
}

// ---------------------------------------------------------------- rows

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: String,
    pub herdr_session: String,
    pub config_toml: String,
    pub config_version: i64,
    pub quota_lanes: i64,
    pub quota_temps: i64,
    pub quota_includes_captain: bool,
    pub plan_version: i64,
    pub created: Ts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatRow {
    /// `<project>:<name>`
    pub id: String,
    pub project: String,
    pub role: SeatRole,
    pub name: String,
    pub profile: String,
    /// Current active generation; 0 = never onboarded.
    pub generation: i64,
    pub state: SeatState,
    pub state_since: Ts,
    pub state_reason: Option<String>,
    /// The task a temp seat is bound to for life (None for every other seat).
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingStatus {
    Pending,
    Active,
    Void,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingRow {
    pub seat: String,
    pub generation: i64,
    pub session: String,
    pub pane_id: String,
    /// Herdr agent kind, e.g. `opencode`.
    pub kind: String,
    pub session_ref: Option<String>,
    pub status: BindingStatus,
    pub handover_id: Option<String>,
    pub created: Ts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    pub project: String,
    pub thread: String,
    pub task: Option<String>,
    pub attempt: Option<i64>,
    pub sender: String,
    pub sender_gen: i64,
    pub recipient: String,
    pub kind: MsgKind,
    pub urgent: bool,
    pub body: String,
    pub reply_to: Option<String>,
    pub needs_ack: bool,
    pub state: MsgState,
    pub created: Ts,
    /// v6 (§5.2): the structured fields of a progress answer, JSON
    /// `{"left": <min>, "blocker": "<text>"}`.
    pub meta: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub project: String,
    /// `M-<n>`
    pub id: String,
    pub thread: String,
    pub task: Option<String>,
    pub attempt: Option<i64>,
    pub sender: String,
    pub sender_gen: i64,
    pub recipient: String,
    pub kind: MsgKind,
    pub urgent: bool,
    pub body: String,
    pub reply_to: Option<String>,
    pub needs_ack: bool,
    pub state: MsgState,
    pub wait_on: Option<String>,
    pub wait_until: Option<Ts>,
    pub result: Option<String>,
    pub created: Ts,
    pub resolved: Option<Ts>,
    /// v6 (§5.2): the structured fields of a progress answer, JSON
    /// `{"left": <min>, "blocker": "<text>"}`.
    pub meta: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOp {
    pub project: String,
    pub seat: String,
    /// Generation fixed at intent time (design/08); checked again before execute.
    pub generation: i64,
    pub kind: OpKind,
    pub task: Option<String>,
    pub attempt: Option<i64>,
    pub ref_msg: Option<String>,
    pub plan_version: Option<i64>,
    /// JSON object with kind-specific fields (e.g. prompt text, message ids covered).
    pub payload: serde_json::Value,
    pub created: Ts,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpRow {
    pub project: String,
    /// `O-<n>`
    pub id: String,
    pub seat: String,
    pub generation: i64,
    pub kind: OpKind,
    pub task: Option<String>,
    pub attempt: Option<i64>,
    pub ref_msg: Option<String>,
    pub plan_version: Option<i64>,
    pub payload: serde_json::Value,
    pub state: OpState,
    pub result: Option<String>,
    pub check_budget: i64,
    pub created: Ts,
    pub updated: Ts,
}

/// One task (design/06). `deps` is stored as a JSON array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub project: String,
    /// `T-<n>`
    pub id: String,
    /// queue position, ascending
    pub rank: i64,
    pub state: TaskState,
    /// `now|must|should|could|wont`
    pub priority: String,
    pub lane: Option<String>,
    pub milestone: Option<String>,
    pub required: bool,
    /// current contract version
    pub contract_version: i64,
    pub deps: Vec<String>,
    pub followup_of: Option<String>,
    pub wait_on: Option<String>,
    pub wait_cond: Option<String>,
    pub rework_note: Option<String>,
    /// The planned time of the task in minutes (T-38): the plan's `estimate:`
    /// field. None for a task that has none.
    pub estimate_min: Option<i64>,
    /// The GitHub issue the task came from (T-79): the plan's `issue: #<n>`.
    pub issue: Option<i64>,
    /// v6 (§1.1): the repo the task works in, copied from the lane's config
    /// when it was created. None for a task of before v6; its reader falls
    /// back to the lane's current config.
    pub repo: Option<String>,
    /// v6 (§1.1): the integration branch the task is merged into.
    pub base: Option<String>,
    /// v6 (§1.1): the gate the task's merge runs.
    pub gate: Option<String>,
    /// v6 (§5.1): the estimate the fail verdict asked for with
    /// `estimate=<min>`; the budget of the rework round.
    pub rework_estimate_min: Option<i64>,
    pub updated: Ts,
}

/// One contract version of a task. `evidence_files` is stored as a JSON array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractRow {
    pub project: String,
    pub task: String,
    pub version: i64,
    pub goal: String,
    pub accept: String,
    pub evidence_files: Vec<String>,
    pub evidence_due_min: Option<i64>,
    pub body: String,
    pub created: Ts,
    /// v7 (T-272): the reason the task is mechanical — the merge gate alone
    /// proves the change, so the review is skipped. None: reviewed as before.
    pub mechanical: Option<String>,
}

/// One attempt of a task (design/06).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptRow {
    pub project: String,
    pub task: String,
    pub n: i64,
    pub seat: String,
    pub generation: i64,
    /// contract version current when the attempt started
    pub contract_version: i64,
    /// contract version the attempt works to (only rises)
    pub adopted_version: i64,
    pub pending_update: Option<i64>,
    pub state: AttemptState,
    pub candidate: Option<String>,
    pub report: Option<String>,
    pub safe_point: Option<String>,
    pub started: Ts,
    pub updated: Ts,
    /// v6 (§3): the session the attempt ran in (Herdr session at check-in,
    /// refreshed at hand-in).
    pub session_ref: Option<String>,
    /// v6 (§3): the agent kind that ran the attempt (pi, codex, ...).
    pub session_cli: Option<String>,
    /// v6 (§3.2): what the engine did before the packet: keep | new |
    /// compact | resume | fresh | new-unconfirmed | compact-fallback |
    /// resume-fallback.
    pub session_action: Option<String>,
    /// v6 (§3.2): when the packet was actually sent through Herdr.
    pub sent_at: Option<Ts>,
}

/// One step of an attempt (T-38). Each step has its own time; a step writes
/// its own column and never another's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptStep {
    /// The attempt was handed to the lane.
    Delivered,
    /// The lane confirmed the delivery and began to work.
    Started,
    /// The lane handed the candidate in.
    HandedIn,
    /// The reviewer handed in the verdict.
    Verdict,
    /// The attempt was settled (the captain decided).
    Settled,
    /// The candidate was merged into the integration branch.
    Merged,
}

impl AttemptStep {
    /// Every step, in the order they happen.
    pub const ALL: &'static [AttemptStep] = &[
        AttemptStep::Delivered,
        AttemptStep::Started,
        AttemptStep::HandedIn,
        AttemptStep::Verdict,
        AttemptStep::Settled,
        AttemptStep::Merged,
    ];

    /// The column the step's time lives in.
    pub fn column(self) -> &'static str {
        match self {
            AttemptStep::Delivered => "delivered_at",
            AttemptStep::Started => "started_at",
            AttemptStep::HandedIn => "handed_in_at",
            AttemptStep::Verdict => "verdict_at",
            AttemptStep::Settled => "settled_at",
            AttemptStep::Merged => "merged_at",
        }
    }
}

/// The times of the steps of one attempt (T-38). A step that has not happened
/// yet is None; the times are written by the step itself, never by a row
/// replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AttemptTimes {
    pub delivered: Option<Ts>,
    pub started: Option<Ts>,
    pub handed_in: Option<Ts>,
    pub verdict: Option<Ts>,
    pub settled: Option<Ts>,
    pub merged: Option<Ts>,
}

impl AttemptTimes {
    /// The time of one step.
    pub fn get(&self, step: AttemptStep) -> Option<Ts> {
        match step {
            AttemptStep::Delivered => self.delivered,
            AttemptStep::Started => self.started,
            AttemptStep::HandedIn => self.handed_in,
            AttemptStep::Verdict => self.verdict,
            AttemptStep::Settled => self.settled,
            AttemptStep::Merged => self.merged,
        }
    }

    /// The time of the last step that happened (the steps in their order).
    pub fn last(&self) -> Option<Ts> {
        self.merged
            .or(self.settled)
            .or(self.verdict)
            .or(self.handed_in)
            .or(self.started)
            .or(self.delivered)
    }
}

/// One acceptance verdict (append-only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictRow {
    pub project: String,
    pub task: String,
    pub attempt: i64,
    pub contract_version: i64,
    pub candidate: String,
    pub passed: bool,
    pub evidence: Option<String>,
    pub by_seat: String,
    pub created: Ts,
}

/// The review verdict a temp reviewer handed in for one attempt (design/08,
/// conventions §8). One record per (project, task, attempt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewVerdictRow {
    pub project: String,
    pub task: String,
    pub attempt: i64,
    /// `true` = pass, `false` = fail.
    pub passed: bool,
    /// The findings: a path or the text itself.
    pub findings: String,
    /// The temp seat that handed the verdict in.
    pub by_seat: String,
    pub created: Ts,
}

/// The key of a round (v6, §1): the project, the task, the attempt and the
/// role of the round. One round per key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundKey {
    pub project: String,
    pub task: String,
    pub attempt: i64,
    /// `dev` | `rework` | `review`
    pub role: String,
}

/// A round to insert (v6, §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRound {
    pub key: RoundKey,
    pub seat: String,
    pub generation: i64,
    pub cli: String,
    pub model: Option<String>,
    /// The engine's first input of this round (§4.1).
    pub start_at: Ts,
}

/// The tokens and the cost of a round (v6, §4.3-4.4).
#[derive(Debug, Clone, PartialEq)]
pub struct RoundTokens {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_write: Option<i64>,
    pub reasoning: Option<i64>,
    pub cost_usd: Option<f64>,
    /// `agent` | `prices` | `none`
    pub cost_source: String,
}

/// One round of work by one seat (v6, §1): one field per column.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundRow {
    pub project: String,
    pub task: String,
    pub attempt: i64,
    /// `dev` | `rework` | `review`
    pub role: String,
    pub seat: String,
    pub generation: i64,
    pub cli: String,
    pub model: Option<String>,
    pub start_at: Ts,
    /// The next round's start on this seat, or the seat's release (§4.1).
    pub end_at: Option<Ts>,
    /// Every `session_ref` the seat's binding had during the window.
    pub sessions: Vec<String>,
    /// Time on the clock, pauses excluded (§5.1).
    pub active_s: i64,
    /// Time the seat could not work (§5.1).
    pub paused_s: i64,
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_write: Option<i64>,
    pub reasoning: Option<i64>,
    pub cost_usd: Option<f64>,
    pub cost_source: Option<String>,
    /// When the tokens were last read.
    pub counted_at: Option<Ts>,
}

/// A friction report to insert (v6, §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFriction {
    pub project: String,
    pub task: String,
    pub attempt: i64,
    /// `dev` | `rework` | `review`
    pub role: String,
    pub seat: String,
    pub body: String,
    /// True when the round passed its budget, so the report was required (§6.2).
    pub required: bool,
}

/// One friction report (v6, §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrictionRow {
    pub project: String,
    /// `F-<n>`
    pub id: String,
    pub task: String,
    pub attempt: i64,
    pub role: String,
    pub seat: String,
    pub body: String,
    pub required: bool,
    pub created: Ts,
    /// When the captain closed it (§6.4).
    pub closed_at: Option<Ts>,
    /// What the captain did about it: fixed (what), lesson (which), not worth
    /// it (why).
    pub outcome: Option<String>,
}

/// A decision to log (v6, §7). `key` is the hash of the normalized line, so
/// an apply is idempotent (§7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDecision {
    pub project: String,
    pub key: String,
    pub voyage: String,
    /// The date written on the line, or the apply time.
    pub at: String,
    /// The kind of the decision (§7.2).
    pub kind: String,
    pub what: String,
    pub why: Option<String>,
    /// The alternative not taken.
    pub alt: Option<String>,
    /// What it changed: scope, time, quality, risk.
    pub changes: Option<String>,
    /// True when the decision is worth a look at landing (§7.2).
    pub star: bool,
    /// `plan` | `gear`
    pub source: String,
    /// The plan line, verbatim (source = plan).
    pub line: Option<String>,
    pub plan_version: Option<i64>,
}

/// One decision of the log (v6, §7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRow {
    pub project: String,
    pub key: String,
    /// The order of first appearance.
    pub seq: i64,
    pub voyage: String,
    pub at: String,
    pub kind: String,
    pub what: String,
    pub why: Option<String>,
    pub alt: Option<String>,
    pub changes: Option<String>,
    pub star: bool,
    pub source: String,
    pub line: Option<String>,
    pub plan_version: Option<i64>,
    pub created: Ts,
}

/// One stretch a seat could not work (v6, §5.4): a quota park or a declared
/// wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PauseRow {
    pub project: String,
    pub seat: String,
    /// `quota` | `wait`
    pub kind: String,
    /// The provider and window, or the task waited for.
    pub reason: Option<String>,
    pub start_at: Ts,
    /// None while the pause lasts.
    pub end_at: Option<Ts>,
}

/// A milestone as a voyage (T-22): its one-sentence `done`, the quality `bar`
/// of its phase, the planned `eta`, and where it departed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoyageRow {
    pub project: String,
    /// The milestone id the plan uses (`M1`, `M2`, …).
    pub milestone: String,
    pub title: String,
    /// None until the plan describes the voyage (a milestone whose tasks
    /// predate its first block has a departure and no words yet).
    pub done: Option<String>,
    pub bar: Option<String>,
    pub eta: Option<String>,
    /// When the milestone's first task was created; None while it has none.
    pub departure: Option<Ts>,
    /// How many tasks the milestone had at departure.
    pub departure_tasks: Option<i64>,
    /// Total estimate at departure; unknown for unmeasured historical scope.
    pub departure_estimate_min: Option<i64>,
    pub updated: Ts,
}

/// One station of a voyage (M5 S3): its id as the plan names it (`S1`, `S2`,
/// …) and what reaching it needs, in the user's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoyageStation {
    pub id: String,
    pub needs: String,
}

/// One applied plan version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRow {
    pub project: String,
    pub version: i64,
    pub draft: String,
    pub applied: Ts,
    pub by_seat: Option<String>,
    pub by_gen: Option<i64>,
}

/// Durable captain quota episode (T-37), shared by engine policy and board.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CaptainQuota {
    pub generation: i64,
    pub detected_at: Ts,
    pub reset_at: Ts,
    pub bar: String,
    pub wake_op: Option<String>,
    pub views_before_wake: usize,
    pub resumed: bool,
}

/// What one `gear` call actually displayed (design/13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewRecord {
    pub project: String,
    pub seat: String,
    pub generation: i64,
    pub ts: Ts,
    pub shown_msgs: Vec<String>,
    pub shown_ops: Vec<String>,
    pub shown_task: Option<String>,
    pub shown_attempt: Option<i64>,
    pub shown_contract: Option<i64>,
}

/// Result of [`Tx::run_request`].
#[derive(Debug, Clone, PartialEq)]
pub enum RequestOutcome {
    /// First execution; the closure ran and its result was stored.
    Executed(serde_json::Value),
    /// Same id + same args seen before; stored result returned, closure not run.
    Replayed(serde_json::Value),
}

// ---------------------------------------------------------------- store

impl Store {
    /// Open (creating if needed) the database at `path`: WAL mode, busy timeout,
    /// foreign keys on, the schema applied.
    ///
    /// A store of an older version is migrated up (one step per transaction).
    /// A store of a newer version is refused for writing with
    /// [`ErrorCode::SchemaTooNew`] before anything is written: the schema only
    /// migrates up, never down.
    pub fn open(path: &Path) -> GearResult<Store> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| {
                    GearError::new(
                        ErrorCode::BadFormat,
                        format!("cannot create directory {}: {e}", dir.display()),
                    )
                })?;
            }
        }
        let conn = Connection::open(path).map_err(map_sqlite)?;
        let mut store = Store { conn };
        // The version is read before the pragmas: a store of a newer version
        // is refused before a single byte of it is changed.
        let version = Self::stored_version(&store.conn)?;
        if let Version::Stamped(version) = version {
            if version > SCHEMA_VERSION {
                return Err(GearError::new(
                    ErrorCode::SchemaTooNew,
                    format!(
                        "the store at {} has schema version {version}, but this binary writes version \
                         {SCHEMA_VERSION}; the schema only migrates up — upgrade GEAR and try again",
                        path.display()
                    ),
                ));
            }
        }
        store.prepare_connection()?;
        store.apply_schema()?;
        Ok(store)
    }

    /// Open an **existing** database for reading only (T-256): `gear admin
    /// init --check` reads the other projects' configs to see whether two
    /// projects would cut the same branch names, and that check writes
    /// nothing — it creates no file, applies no schema and migrates nothing.
    /// A database of another schema version is read as it is.
    pub fn open_read_only(path: &Path) -> GearResult<Store> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(map_sqlite)?;
        let store = Store { conn };
        store
            .conn
            .set_prepared_statement_cache_capacity(STATEMENT_CACHE_CAPACITY);
        store
            .conn
            .busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))
            .map_err(map_sqlite)?;
        Ok(store)
    }

    /// In-memory database for tests (schema applied).
    pub fn open_in_memory() -> GearResult<Store> {
        let conn = Connection::open_in_memory().map_err(map_sqlite)?;
        let mut store = Store { conn };
        store.prepare_connection()?;
        store.apply_schema()?;
        Ok(store)
    }

    /// Run `f` inside `BEGIN IMMEDIATE`. Commit on `Ok`, roll back on `Err`.
    /// Lock timeout maps to `ErrorCode::Locked` with nothing written.
    ///
    /// The time from `BEGIN IMMEDIATE` to the commit or rollback is measured:
    /// a write over [`SLOW_WRITE_MS`] prints one line to stderr naming the
    /// caller ([`slow_write_line`]), so a writer that held the store shows.
    ///
    /// The clock starts **after** the transaction is acquired (F1 of the
    /// T-379 review): a caller that queued for the lock is waiting, not
    /// holding the store, and the line must not blame it for the wait.
    #[track_caller]
    pub fn write<T>(&mut self, f: impl FnOnce(&Tx<'_>) -> GearResult<T>) -> GearResult<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        let started = Instant::now();
        let tx = Tx { tx };
        let result = match f(&tx) {
            Ok(value) => tx.tx.commit().map_err(map_sqlite).map(|()| value),
            Err(e) => {
                drop(tx); // rolls back, nothing written
                Err(e)
            }
        };
        let elapsed = started.elapsed();
        if elapsed.as_millis() > u128::from(SLOW_WRITE_MS) {
            eprintln!(
                "{}",
                slow_write_line(elapsed.as_millis(), std::panic::Location::caller())
            );
        }
        result
    }

    /// Run `f` in a deferred (read) transaction for a consistent snapshot.
    pub fn read<T>(&mut self, f: impl FnOnce(&Tx<'_>) -> GearResult<T>) -> GearResult<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(map_sqlite)?;
        let tx = Tx { tx };
        match f(&tx) {
            Ok(value) => {
                tx.tx.commit().map_err(map_sqlite)?;
                Ok(value)
            }
            Err(e) => {
                drop(tx); // rolls back
                Err(e)
            }
        }
    }

    /// Connection pragmas: WAL journal, busy timeout, foreign keys.
    fn prepare_connection(&self) -> GearResult<()> {
        // Queries are prepared through the statement cache ([`prepare_cached`]):
        // a tick or a board frame reuses its statements instead of re-parsing
        // every query.
        self.conn
            .set_prepared_statement_cache_capacity(STATEMENT_CACHE_CAPACITY);
        // An in-memory database keeps the "memory" journal mode; the value is
        // informational, only a failure matters.
        let _mode: String = self
            .conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(map_sqlite)?;
        self.conn
            .busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))
            .map_err(map_sqlite)?;
        self.conn
            .execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// Select each creation/migration step under its write lock. Another
    /// opener may have advanced the version while this connection waited.
    /// Schema changes and their stamp commit together, one step at a time.
    fn apply_schema(&mut self) -> GearResult<()> {
        loop {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite)?;
            match Self::stored_version(&tx)? {
                Version::Fresh => {
                    tx.execute_batch(SCHEMA_SQL).map_err(map_sqlite)?;
                    set_version(&tx, SCHEMA_VERSION)?;
                    tx.commit().map_err(map_sqlite)?;
                    return Ok(());
                }
                Version::Stamped(version) if version > SCHEMA_VERSION => {
                    return Err(GearError::new(
                        ErrorCode::SchemaTooNew,
                        format!(
                            "the store has schema version {version}, but this binary writes version \
                             {SCHEMA_VERSION}; the schema only migrates up — upgrade GEAR and try again"
                        ),
                    ));
                }
                Version::Stamped(version) if version == SCHEMA_VERSION => return Ok(()),
                version => {
                    let from = match version {
                        Version::Stamped(version) => version.max(OLDEST_VERSION),
                        Version::Unstamped => OLDEST_VERSION,
                        Version::Fresh => unreachable!(),
                    };
                    let step = STEPS.iter().find(|step| step.from == from).ok_or_else(|| {
                        GearError::new(
                            ErrorCode::BadFormat,
                            format!("no schema migration from version {from}"),
                        )
                    })?;
                    (step.apply)(&tx)?;
                    set_version(&tx, step.from + 1)?;
                    tx.commit().map_err(map_sqlite)?;
                }
            }
        }
    }

    /// What the store says about its schema version. Read before anything
    /// writes.
    fn stored_version(conn: &Connection) -> GearResult<Version> {
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .map_err(map_sqlite)?;
        if tables == 0 {
            return Ok(Version::Fresh);
        }
        let meta: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='meta'",
                [],
                |row| row.get(0),
            )
            .map_err(map_sqlite)?;
        if meta == 0 {
            return Ok(Version::Unstamped);
        }
        let stored: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(map_sqlite)?;
        match stored {
            None => Ok(Version::Unstamped),
            Some(raw) => {
                let version: i64 = raw.trim().parse().map_err(|_| {
                    GearError::new(
                        ErrorCode::BadFormat,
                        format!("meta.schema_version is not a number: {raw}"),
                    )
                })?;
                Ok(Version::Stamped(version))
            }
        }
    }
}

/// What a store says about its schema version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    /// The store has no tables: it is new.
    Fresh,
    /// The store has a version.
    Stamped(i64),
    /// The store has tables but no version: it predates the stamp.
    Unstamped,
}

/// The version stamp in `meta`.
fn set_version(tx: &rusqlite::Transaction<'_>, version: i64) -> GearResult<()> {
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![version.to_string()],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

// ---------------------------------------------------------------- tx

impl<'s> Tx<'s> {
    // ---- infrastructure

    /// SQLite's `PRAGMA data_version` (T-384): a number that moves when
    /// **another** connection commits a write to the database, and stays put
    /// for this connection's own writes (SQLite's documented behaviour). One
    /// cheap query is all it costs, so a reader that shows the database can ask
    /// it every second and rebuild only when it moved.
    ///
    /// The numbers are only comparable with the numbers of the same
    /// connection: a caller keeps the last one it read and compares.
    pub fn data_version(&self) -> GearResult<i64> {
        self.tx
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .map_err(map_sqlite)
    }

    /// Next id for `kind` in `project`: `T-1`, `T-2`, …; kinds: T M R V O. Never reused.
    pub fn next_id(&self, project: &str, kind: char) -> GearResult<String> {
        let kind = kind.to_string();
        // The row is created with the first number; every later call bumps it.
        self.tx
            .execute(
                "INSERT INTO id_seq (project, kind, next) VALUES (?1, ?2, 1)
                 ON CONFLICT(project, kind) DO UPDATE SET next = id_seq.next + 1",
                params![project, kind],
            )
            .map_err(map_sqlite)?;
        let n: i64 = self
            .tx
            .query_row(
                "SELECT next FROM id_seq WHERE project=?1 AND kind=?2",
                params![project, kind],
                |row| row.get(0),
            )
            .map_err(map_sqlite)?;
        Ok(format!("{kind}-{n}"))
    }

    /// Request replay (design/02). `args_key` must already include project,
    /// caller identity (with generation), action and normalized args.
    /// - unseen id: run `f`, store its JSON result, return `Executed`;
    /// - same id, same args_key hash: return `Replayed(stored)` without running `f`;
    /// - same id, different hash: `RequestConflict`.
    pub fn run_request(
        &self,
        request_id: &str,
        args_key: &str,
        now: Ts,
        f: impl FnOnce(&Tx<'s>) -> GearResult<serde_json::Value>,
    ) -> GearResult<RequestOutcome> {
        let hash = fnv1a_hex(args_key.as_bytes());
        let stored: Option<(String, String)> = {
            let mut stmt = self
                .tx
                .prepare_cached("SELECT args_hash, result FROM requests WHERE id=?1")
                .map_err(map_sqlite)?;
            let mut rows = stmt.query(params![request_id]).map_err(map_sqlite)?;
            match rows.next().map_err(map_sqlite)? {
                Some(row) => Some((row.get(0).map_err(map_sqlite)?, row.get(1).map_err(map_sqlite)?)),
                None => None,
            }
        };
        if let Some((stored_hash, stored_result)) = stored {
            if stored_hash == hash {
                return Ok(RequestOutcome::Replayed(
                    json_from_text(&stored_result).map_err(map_sqlite)?,
                ));
            }
            return Err(GearError::new(
                ErrorCode::RequestConflict,
                format!("request {request_id} was already used with different arguments"),
            ));
        }
        let result = f(self)?;
        let text = json_to_text(&result).map_err(map_sqlite)?;
        self.tx
            .execute(
                "INSERT INTO requests (id, args_hash, result, created) VALUES (?1, ?2, ?3, ?4)",
                params![request_id, hash, text, now],
            )
            .map_err(map_sqlite)?;
        Ok(RequestOutcome::Executed(result))
    }

    /// The stored result of a request id, with the hash of its arguments:
    /// `(args_hash, result json)`. A caller that runs a side effect before its
    /// transaction (T-19: the hand-in's git check) can replay a committed
    /// request's receipt without repeating the side effect.
    pub fn get_request(&self, request_id: &str) -> GearResult<Option<(String, String)>> {
        let stored: Option<(String, String)> = self
            .tx
            .query_row(
                "SELECT args_hash, result FROM requests WHERE id=?1",
                params![request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(stored)
    }

    pub fn get_meta(&self, key: &str) -> GearResult<Option<String>> {
        let value: Option<String> = self
            .tx
            .query_row("SELECT value FROM meta WHERE key=?1", params![key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(map_sqlite)?;
        Ok(value)
    }

    /// The meta keys that start with `prefix`, in order.
    ///
    /// The board finds the quota readings of the providers this way (§9.1):
    /// they live in `meta` under `quota.reading.<provider>`.
    pub fn meta_keys_with_prefix(&self, prefix: &str) -> GearResult<Vec<String>> {
        let mut stmt = self
            .tx
            .prepare_cached("SELECT key FROM meta ORDER BY key")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite)?;
        let mut keys = Vec::new();
        for row in rows {
            let key = row.map_err(map_sqlite)?;
            if key.starts_with(prefix) {
                keys.push(key);
            }
        }
        Ok(keys)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    pub fn audit(
        &self,
        now: Ts,
        project: Option<&str>,
        actor: &str,
        action: &str,
        detail: &str,
    ) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO audit (ts, project, actor, action, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![now, project, actor, action, detail],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    // ---- projects

    pub fn insert_project(&self, row: &ProjectRow) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO projects
                   (id, herdr_session, config_toml, config_version, quota_lanes, quota_temps,
                    quota_includes_captain, plan_version, goal, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9)",
                params![
                    row.id,
                    row.herdr_session,
                    row.config_toml,
                    row.config_version,
                    row.quota_lanes,
                    row.quota_temps,
                    row.quota_includes_captain as i64,
                    row.plan_version,
                    row.created,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// Ids of every project in the store, ascending. The engine iterates these
    /// for recovery on start and for each tick.
    pub fn project_ids(&self) -> GearResult<Vec<String>> {
        let mut stmt = self
            .tx
            .prepare_cached("SELECT id FROM projects ORDER BY id")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(map_sqlite)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
    }

    /// Replace the mutable fields of a registered project (raw write): the
    /// config text, its version and the quotas. `id` and `created` stay.
    /// Missing → NoTarget.
    pub fn update_project(&self, row: &ProjectRow) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE projects
                 SET herdr_session=?2, config_toml=?3, config_version=?4, quota_lanes=?5,
                     quota_temps=?6, quota_includes_captain=?7, plan_version=?8
                 WHERE id=?1",
                params![
                    row.id,
                    row.herdr_session,
                    row.config_toml,
                    row.config_version,
                    row.quota_lanes,
                    row.quota_temps,
                    row.quota_includes_captain as i64,
                    row.plan_version,
                ],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "project", &row.id)
    }

    pub fn get_project(&self, id: &str) -> GearResult<Option<ProjectRow>> {
        let row: Option<ProjectRow> = self
            .tx
            .query_row(
                "SELECT id, herdr_session, config_toml, config_version, quota_lanes, quota_temps,
                        quota_includes_captain, plan_version, created
                 FROM projects WHERE id=?1",
                params![id],
                project_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    // ---- seats & bindings

    pub fn insert_seat(&self, row: &SeatRow) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO seats
                   (id, project, role, name, profile, generation, state, state_since, state_reason, task_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    row.id,
                    row.project,
                    row.role.as_str(),
                    row.name,
                    row.profile,
                    row.generation,
                    row.state.as_str(),
                    row.state_since,
                    row.state_reason,
                    row.task_id,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    pub fn get_seat(&self, seat_id: &str) -> GearResult<Option<SeatRow>> {
        let row: Option<SeatRow> = self
            .tx
            .query_row(&seat_select(" WHERE id=?1"), params![seat_id], seat_from_row)
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    pub fn seats_of_project(&self, project: &str) -> GearResult<Vec<SeatRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&seat_select(" WHERE project=?1 ORDER BY id"))
            .map_err(map_sqlite)?;
        let rows = stmt.query_map(params![project], seat_from_row).map_err(map_sqlite)?;
        let mut seats = Vec::new();
        for row in rows {
            seats.push(row.map_err(map_sqlite)?);
        }
        Ok(seats)
    }

    /// The seat with role Captain in `project`, if any.
    pub fn captain_of(&self, project: &str) -> GearResult<Option<SeatRow>> {
        let row: Option<SeatRow> = self
            .tx
            .query_row(
                &seat_select(" WHERE project=?1 AND role=?2 ORDER BY id LIMIT 1"),
                params![project, SeatRole::Captain.as_str()],
                seat_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    /// Raw state write. Caller must have checked `SeatState::can_transition`.
    pub fn set_seat_state(
        &self,
        seat_id: &str,
        state: SeatState,
        reason: Option<&str>,
        now: Ts,
    ) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE seats SET state=?2, state_since=?3, state_reason=?4 WHERE id=?1",
                params![seat_id, state.as_str(), now, reason],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "seat", seat_id)
    }

    pub fn set_seat_generation(&self, seat_id: &str, generation: i64) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE seats SET generation=?2 WHERE id=?1",
                params![seat_id, generation],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "seat", seat_id)
    }

    /// The profile a seat is observed and onboarded with (the project config
    /// owns it). Raw write: the caller checks the seat.
    pub fn set_seat_profile(&self, seat_id: &str, profile: &str) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE seats SET profile=?2 WHERE id=?1",
                params![seat_id, profile],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "seat", seat_id)
    }

    /// Bind a seat to the one task it works on (a temp seat for life).
    /// Raw write: the caller checks that the seat is a temp seat.
    pub fn set_seat_task(&self, seat_id: &str, task: &str) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE seats SET task_id=?2 WHERE id=?1",
                params![seat_id, task],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "seat", seat_id)
    }

    /// End the job binding of a seat: a reviewer in `Standby` has no job
    /// (design §3.6 — its verdict is in and it waits for the next one).
    /// Raw write: the caller checks that the seat is a temp seat.
    pub fn clear_seat_task(&self, seat_id: &str) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE seats SET task_id=NULL WHERE id=?1",
                params![seat_id],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "seat", seat_id)
    }

    pub fn insert_binding(&self, row: &BindingRow) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO bindings
                   (seat, generation, session, pane_id, kind, session_ref, status, handover_id, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    row.seat,
                    row.generation,
                    row.session,
                    row.pane_id,
                    row.kind,
                    row.session_ref,
                    binding_status_str(row.status),
                    row.handover_id,
                    row.created,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    pub fn get_binding(&self, seat_id: &str, generation: i64) -> GearResult<Option<BindingRow>> {
        let row: Option<BindingRow> = self
            .tx
            .query_row(
                "SELECT seat, generation, session, pane_id, kind, session_ref, status, handover_id, created
                 FROM bindings WHERE seat=?1 AND generation=?2",
                params![seat_id, generation],
                binding_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    pub fn set_binding_status(
        &self,
        seat_id: &str,
        generation: i64,
        status: BindingStatus,
    ) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE bindings SET status=?3 WHERE seat=?1 AND generation=?2",
                params![seat_id, generation, binding_status_str(status)],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "binding", &format!("{seat_id} generation {generation}"))
    }

    /// Raw write of the binding's session ref: the agent session Herdr
    /// reported for this binding (design/03). The binding must exist.
    pub fn set_binding_session_ref(
        &self,
        seat_id: &str,
        generation: i64,
        session_ref: &str,
    ) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE bindings SET session_ref=?3 WHERE seat=?1 AND generation=?2",
                params![seat_id, generation, session_ref],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "binding", &format!("{seat_id} generation {generation}"))
    }

    // ---- messages

    /// Allocates `M-<n>` via `next_id` and inserts. Returns the stored row.
    pub fn insert_message(&self, msg: &NewMessage) -> GearResult<MessageRow> {
        let id = self.next_id(&msg.project, 'M')?;
        self.tx
            .execute(
                "INSERT INTO messages
                   (project, id, thread, task, attempt, sender, sender_gen, recipient, kind, urgent,
                    body, reply_to, needs_ack, state, wait_on, wait_until, result, created, resolved,
                    meta)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, NULL, NULL, NULL, ?15, NULL, ?16)",
                params![
                    msg.project,
                    id,
                    msg.thread,
                    msg.task,
                    msg.attempt,
                    msg.sender,
                    msg.sender_gen,
                    msg.recipient,
                    msg.kind.as_str(),
                    msg.urgent as i64,
                    msg.body,
                    msg.reply_to,
                    msg.needs_ack as i64,
                    msg.state.as_str(),
                    msg.created,
                    msg.meta,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(MessageRow {
            project: msg.project.clone(),
            id,
            thread: msg.thread.clone(),
            task: msg.task.clone(),
            attempt: msg.attempt,
            sender: msg.sender.clone(),
            sender_gen: msg.sender_gen,
            recipient: msg.recipient.clone(),
            kind: msg.kind,
            urgent: msg.urgent,
            body: msg.body.clone(),
            reply_to: msg.reply_to.clone(),
            needs_ack: msg.needs_ack,
            state: msg.state,
            wait_on: None,
            wait_until: None,
            result: None,
            created: msg.created,
            resolved: None,
            meta: msg.meta.clone(),
        })
    }

    pub fn get_message(&self, project: &str, id: &str) -> GearResult<Option<MessageRow>> {
        let row: Option<MessageRow> = self
            .tx
            .query_row(
                &message_select(" WHERE project=?1 AND id=?2"),
                params![project, id],
                message_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    /// Raw state write (+ result/resolved/wait fields). Caller checks `MsgState::can_transition`.
    pub fn update_message_state(
        &self,
        project: &str,
        id: &str,
        state: MsgState,
        result: Option<&str>,
        wait_on: Option<&str>,
        wait_until: Option<Ts>,
        now: Ts,
    ) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE messages
                 SET state=?3, result=?4, wait_on=?5, wait_until=?6,
                     resolved=CASE WHEN ?3='resolved' THEN ?7 ELSE resolved END
                 WHERE project=?1 AND id=?2",
                params![project, id, state.as_str(), result, wait_on, wait_until, now],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "message", &format!("{id} in project {project}"))
    }

    /// Replace a message's body. The engine updates its own open item while
    /// a condition lasts (T-7); a caller keeps every other field.
    pub fn update_message_body(&self, project: &str, id: &str, body: &str) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE messages SET body=?3 WHERE project=?1 AND id=?2",
                params![project, id, body],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "message", &format!("{id} in project {project}"))
    }

    /// Promote an open action item without changing its body or lifecycle.
    pub fn mark_message_urgent(&self, project: &str, id: &str) -> GearResult<()> {
        let changed = self.tx.execute(
            "UPDATE messages SET urgent=1 WHERE project=?1 AND id=?2",
            params![project, id],
        ).map_err(map_sqlite)?;
        require_written(changed, "message", &format!("{id} in project {project}"))
    }

    /// Set the current urgency of an existing item.
    pub fn set_message_urgent(&self, project: &str, id: &str, urgent: bool) -> GearResult<()> {
        let changed = self.tx.execute("UPDATE messages SET urgent=?3 WHERE project=?1 AND id=?2", params![project, id, urgent]).map_err(map_sqlite)?;
        require_written(changed, "message", &format!("{id} in project {project}"))
    }

    /// Progress messages for one attempt, in their recorded order.
    pub fn progress_of_attempt(&self, project: &str, task: &str, attempt: i64) -> GearResult<Vec<MessageRow>> {
        let mut stmt = self.tx.prepare_cached(&message_select(" WHERE project=?1 AND task=?2 AND attempt=?3 AND kind='progress' ORDER BY created, id")).map_err(map_sqlite)?;
        let rows = stmt.query_map(params![project, task, attempt], message_from_row).map_err(map_sqlite)?;
        let mut result = Vec::new();
        for row in rows { result.push(row.map_err(map_sqlite)?); }
        Ok(result)
    }

    /// Messages to `recipient` in state Open, or Waiting with `wait_until <= now`.
    /// Ordered by created ascending.
    pub fn actionable_for(
        &self,
        project: &str,
        recipient: &str,
        now: Ts,
    ) -> GearResult<Vec<MessageRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&message_select(
                " WHERE project=?1 AND recipient=?2
                   AND (state='open' OR (state='waiting' AND wait_until IS NOT NULL AND wait_until<=?3))
                 ORDER BY created, id",
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project, recipient, now], message_from_row)
            .map_err(map_sqlite)?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(map_sqlite)?);
        }
        Ok(messages)
    }

    /// Reply/Instruction messages to `recipient` created after `since` (for "unshown" checks).
    ///
    /// "Info" messages are the answers the captain sent (kind `reply`, stored in
    /// state `info`); progress is also stored in state `info` but must not ring
    /// per item (design/07), so the kind list is what decides.
    pub fn messages_to_since(
        &self,
        project: &str,
        recipient: &str,
        since: Ts,
    ) -> GearResult<Vec<MessageRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&message_select(
                " WHERE project=?1 AND recipient=?2 AND created>?3
                   AND kind IN ('reply', 'instruction')
                 ORDER BY created, id",
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project, recipient, since], message_from_row)
            .map_err(map_sqlite)?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(map_sqlite)?);
        }
        Ok(messages)
    }

    /// The messages `sender` sent since `since` (T-227), in their recorded
    /// order. Every kind counts: a progress answer, a question, a blocked
    /// report — anything the seat said is the seat's own word.
    pub fn messages_from_since(
        &self,
        project: &str,
        sender: &str,
        since: Ts,
    ) -> GearResult<Vec<MessageRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&message_select(
                " WHERE project=?1 AND sender=?2 AND created>=?3
                 ORDER BY created, id",
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project, sender, since], message_from_row)
            .map_err(map_sqlite)?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(map_sqlite)?);
        }
        Ok(messages)
    }

    pub fn thread_messages(&self, project: &str, thread: &str) -> GearResult<Vec<MessageRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&message_select(
                " WHERE project=?1 AND thread=?2 ORDER BY created, id",
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project, thread], message_from_row)
            .map_err(map_sqlite)?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(map_sqlite)?);
        }
        Ok(messages)
    }

    /// Direction results for one task attempt.
    pub fn direction_results_of_attempt(&self, project: &str, task: &str, attempt: i64) -> GearResult<Vec<MessageRow>> {
        let mut stmt = self.tx.prepare_cached(&message_select(" WHERE project=?1 AND task=?2 AND attempt=?3 AND kind='result' AND CASE WHEN json_valid(meta) THEN json_extract(meta, '$.direction') = 1 ELSE 0 END ORDER BY created, id"))
            .map_err(map_sqlite)?;
        let rows = stmt.query_map(params![project, task, attempt], message_from_row).map_err(map_sqlite)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
    }

    /// The captain quota episode, kept in meta so recovery needs no migration.
    pub fn captain_quota(&self, project: &str) -> GearResult<Option<CaptainQuota>> {
        let raw = self.get_meta(&format!("engine.captain_quota.{project}"))?;
        match raw.filter(|raw| !raw.is_empty()) {
            Some(raw) => serde_json::from_str(&raw).map(Some).map_err(|error| {
                GearError::new(ErrorCode::BadFormat, format!("captain quota state: {error}"))
            }),
            None => Ok(None),
        }
    }

    pub fn set_captain_quota(&self, project: &str, state: Option<&CaptainQuota>) -> GearResult<()> {
        let raw = state.map(serde_json::to_string).transpose()
            .map_err(|error| GearError::new(ErrorCode::BadFormat, error.to_string()))?
            .unwrap_or_default();
        self.set_meta(&format!("engine.captain_quota.{project}"), &raw)
    }

    pub fn captain_quota_paused(&self, project: &str) -> GearResult<bool> {
        Ok(self.captain_quota(project)?.is_some_and(|state| !state.resumed))
    }

    /// The seats of the project that wait for their provider (T-75): their
    /// screen shows a provider back-off. Kept in meta, so it survives a
    /// restart and needs no migration.
    pub fn seats_in_backoff(&self, project: &str) -> GearResult<Vec<String>> {
        let raw = self.get_meta(&format!("engine.backoff.{project}"))?;
        match raw.filter(|raw| !raw.is_empty()) {
            Some(raw) => serde_json::from_str(&raw).map_err(|error| {
                GearError::new(ErrorCode::BadFormat, format!("the back-off seats: {error}"))
            }),
            None => Ok(Vec::new()),
        }
    }

    /// Keep the seats that wait for their provider (T-75). Nothing is written
    /// when the set is the same as the stored one.
    pub fn set_seats_in_backoff(&self, project: &str, seats: &[String]) -> GearResult<()> {
        if self.seats_in_backoff(project)? == seats {
            return Ok(());
        }
        let raw = serde_json::to_string(seats)
            .map_err(|error| GearError::new(ErrorCode::BadFormat, error.to_string()))?;
        self.set_meta(&format!("engine.backoff.{project}"), &raw)
    }

    // ---- views

    pub fn insert_view(&self, v: &ViewRecord) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO views
                   (project, seat, generation, ts, shown_msgs, shown_ops, shown_task, shown_attempt,
                    shown_contract)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    v.project,
                    v.seat,
                    v.generation,
                    v.ts,
                    json_strings_to_text(&v.shown_msgs).map_err(map_sqlite)?,
                    json_strings_to_text(&v.shown_ops).map_err(map_sqlite)?,
                    v.shown_task,
                    v.shown_attempt,
                    v.shown_contract,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// Views by `seat` at `generation` with `ts >= since`, oldest first.
    pub fn views_since(&self, seat: &str, generation: i64, since: Ts) -> GearResult<Vec<ViewRecord>> {
        let mut stmt = self
            .tx
            .prepare_cached(
                "SELECT project, seat, generation, ts, shown_msgs, shown_ops, shown_task,
                        shown_attempt, shown_contract
                 FROM views WHERE seat=?1 AND generation=?2 AND ts>=?3 ORDER BY ts, rowid",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![seat, generation, since], view_from_row)
            .map_err(map_sqlite)?;
        let mut views = Vec::new();
        for row in rows {
            views.push(row.map_err(map_sqlite)?);
        }
        Ok(views)
    }

    // ---- operations

    /// Allocates `O-<n>`, inserts in state Intended. Returns the stored row.
    pub fn insert_op(&self, op: &NewOp) -> GearResult<OpRow> {
        let id = self.next_id(&op.project, 'O')?;
        self.tx
            .execute(
                "INSERT INTO operations
                   (project, id, seat, generation, kind, task, attempt, ref_msg, plan_version, payload,
                    state, result, check_budget, created, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, 0, ?12, ?12)",
                params![
                    op.project,
                    id,
                    op.seat,
                    op.generation,
                    op.kind.as_str(),
                    op.task,
                    op.attempt,
                    op.ref_msg,
                    op.plan_version,
                    json_to_text(&op.payload).map_err(map_sqlite)?,
                    OpState::Intended.as_str(),
                    op.created,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(OpRow {
            project: op.project.clone(),
            id,
            seat: op.seat.clone(),
            generation: op.generation,
            kind: op.kind,
            task: op.task.clone(),
            attempt: op.attempt,
            ref_msg: op.ref_msg.clone(),
            plan_version: op.plan_version,
            payload: op.payload.clone(),
            state: OpState::Intended,
            result: None,
            check_budget: 0,
            created: op.created,
            updated: op.created,
        })
    }

    pub fn get_op(&self, project: &str, id: &str) -> GearResult<Option<OpRow>> {
        let row: Option<OpRow> = self
            .tx
            .query_row(
                &op_select(" WHERE project=?1 AND id=?2"),
                params![project, id],
                op_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    /// Raw state write. Caller checks `OpState::can_transition`.
    pub fn set_op_state(
        &self,
        project: &str,
        id: &str,
        state: OpState,
        result: Option<&str>,
        now: Ts,
    ) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE operations SET state=?3, result=?4, updated=?5 WHERE project=?1 AND id=?2",
                params![project, id, state.as_str(), result, now],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "operation", &format!("{id} in project {project}"))
    }

    pub fn bump_op_check_budget(&self, project: &str, id: &str, now: Ts) -> GearResult<i64> {
        self.tx
            .execute(
                "UPDATE operations SET check_budget=check_budget+1, updated=?3
                 WHERE project=?1 AND id=?2",
                params![project, id, now],
            )
            .map_err(map_sqlite)?;
        let budget: Option<i64> = self
            .tx
            .query_row(
                "SELECT check_budget FROM operations WHERE project=?1 AND id=?2",
                params![project, id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        budget.ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!("no operation {id} in project {project}"),
            )
        })
    }

    /// Deliver/Doorbell ops of `seat` whose state holds the delivery slot.
    pub fn slot_ops(&self, seat: &str) -> GearResult<Vec<OpRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&op_select(
                " WHERE seat=?1
                   AND kind IN ('deliver', 'doorbell')
                   AND state IN ('intended', 'executing', 'submitted', 'unknown')
                 ORDER BY created, id",
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![seat], op_from_row)
            .map_err(map_sqlite)?;
        let mut ops = Vec::new();
        for row in rows {
            ops.push(row.map_err(map_sqlite)?);
        }
        Ok(ops)
    }

    pub fn ops_in_state(&self, project: &str, state: OpState) -> GearResult<Vec<OpRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&op_select(" WHERE project=?1 AND state=?2 ORDER BY created, id"))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project, state.as_str()], op_from_row)
            .map_err(map_sqlite)?;
        let mut ops = Vec::new();
        for row in rows {
            ops.push(row.map_err(map_sqlite)?);
        }
        Ok(ops)
    }

    /// Most recent op of `kind` for `seat` (any state), for cooldowns.
    /// Every op of `kind` for `seat`, oldest first.
    pub fn ops_of_seat_kind(&self, seat: &str, kind: OpKind) -> GearResult<Vec<OpRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&op_select(
                " WHERE seat=?1 AND kind=?2
                 ORDER BY created, CAST(SUBSTR(id, INSTR(id, '-')+1) AS INTEGER)",
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![seat, kind.as_str()], op_from_row)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }

    pub fn last_op(&self, seat: &str, kind: OpKind) -> GearResult<Option<OpRow>> {
        let row: Option<OpRow> = self
            .tx
            .query_row(
                &op_select(
                    " WHERE seat=?1 AND kind=?2
                     ORDER BY created DESC, CAST(SUBSTR(id, INSTR(id, '-')+1) AS INTEGER) DESC
                     LIMIT 1",
                ),
                params![seat, kind.as_str()],
                op_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }
}

impl<'s> Tx<'s> {
    // ---- plans, tasks, contracts (slice 2; T-33)

    /// Insert a plan version. Duplicate (project, version) → BadFormat (constraint violation, as every insert).
    pub fn insert_plan(&self, row: &PlanRow) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO plans (project, version, draft, applied, by_seat, by_gen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    row.project,
                    row.version,
                    row.draft,
                    row.applied,
                    row.by_seat,
                    row.by_gen
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// Highest applied plan version of the project; 0 when none.
    pub fn plan_version(&self, project: &str) -> GearResult<i64> {
        let version: Option<i64> = self
            .tx
            .query_row(
                "SELECT MAX(version) FROM plans WHERE project=?1",
                params![project],
                |row| row.get(0),
            )
            .map_err(map_sqlite)?;
        Ok(version.unwrap_or(0))
    }

    pub fn get_plan(&self, project: &str, version: i64) -> GearResult<Option<PlanRow>> {
        let row: Option<PlanRow> = self
            .tx
            .query_row(
                "SELECT project, version, draft, applied, by_seat, by_gen
                 FROM plans WHERE project=?1 AND version=?2",
                params![project, version],
                plan_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    /// Insert a task. Duplicate (project, id) → BadFormat (constraint violation, as every insert).
    pub fn insert_task(&self, row: &TaskRow) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO tasks
                   (project, id, rank, state, priority, lane, milestone, required, contract_version,
                    deps, followup_of, wait_on, wait_cond, rework_note, estimate_min, issue,
                    repo, base, gate, rework_estimate_min, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                         ?17, ?18, ?19, ?20, ?21)",
                params![
                    row.project,
                    row.id,
                    row.rank,
                    row.state.as_str(),
                    row.priority,
                    row.lane,
                    row.milestone,
                    row.required as i64,
                    row.contract_version,
                    json_strings_to_text(&row.deps).map_err(map_sqlite)?,
                    row.followup_of,
                    row.wait_on,
                    row.wait_cond,
                    row.rework_note,
                    row.estimate_min,
                    row.issue,
                    row.repo,
                    row.base,
                    row.gate,
                    row.rework_estimate_min,
                    row.updated,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    pub fn get_task(&self, project: &str, id: &str) -> GearResult<Option<TaskRow>> {
        let row: Option<TaskRow> = self
            .tx
            .query_row(
                &task_select(" WHERE project=?1 AND id=?2"),
                params![project, id],
                task_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    /// All tasks of the project ordered by rank, then id.
    pub fn tasks_of_project(&self, project: &str) -> GearResult<Vec<TaskRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&task_select(" WHERE project=?1 ORDER BY rank, id"))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project], task_from_row)
            .map_err(map_sqlite)?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row.map_err(map_sqlite)?);
        }
        Ok(tasks)
    }

    /// Replace every column of an existing task (raw write, no transition
    /// check — callers use `require_transition`). Missing → NoTarget.
    pub fn update_task(&self, row: &TaskRow) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE tasks
                 SET rank=?3, state=?4, priority=?5, lane=?6, milestone=?7, required=?8,
                     contract_version=?9, deps=?10, followup_of=?11, wait_on=?12, wait_cond=?13,
                     rework_note=?14, estimate_min=?15, issue=?16, repo=?17, base=?18, gate=?19,
                     rework_estimate_min=?20, updated=?21
                 WHERE project=?1 AND id=?2",
                params![
                    row.project,
                    row.id,
                    row.rank,
                    row.state.as_str(),
                    row.priority,
                    row.lane,
                    row.milestone,
                    row.required as i64,
                    row.contract_version,
                    json_strings_to_text(&row.deps).map_err(map_sqlite)?,
                    row.followup_of,
                    row.wait_on,
                    row.wait_cond,
                    row.rework_note,
                    row.estimate_min,
                    row.issue,
                    row.repo,
                    row.base,
                    row.gate,
                    row.rework_estimate_min,
                    row.updated,
                ],
            )
            .map_err(map_sqlite)?;
        require_written(
            changed,
            "task",
            &format!("{} in project {}", row.id, row.project),
        )
    }

    /// Insert a contract version. Duplicate (project, task, version) → BadFormat (constraint violation, as every insert).
    pub fn insert_contract(&self, row: &ContractRow) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO contracts
                   (project, task, version, goal, accept, evidence_files, evidence_due_min, body, created,
                    mechanical)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    row.project,
                    row.task,
                    row.version,
                    row.goal,
                    row.accept,
                    json_strings_to_text(&row.evidence_files).map_err(map_sqlite)?,
                    row.evidence_due_min,
                    row.body,
                    row.created,
                    row.mechanical,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    pub fn get_contract(&self, project: &str, task: &str, version: i64) -> GearResult<Option<ContractRow>> {
        let row: Option<ContractRow> = self
            .tx
            .query_row(
                "SELECT project, task, version, goal, accept, evidence_files, evidence_due_min, body, created,
                        mechanical
                 FROM contracts WHERE project=?1 AND task=?2 AND version=?3",
                params![project, task, version],
                contract_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    // ---- attempts, verdicts (slice 2; T-34)

    /// Insert an attempt. Duplicate (project, task, n) → BadFormat (constraint violation, as every insert).
    pub fn insert_attempt(&self, row: &AttemptRow) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO attempts
                   (project, task, n, seat, generation, contract_version, adopted_version,
                    pending_update, state, candidate, report, safe_point, started, updated,
                    session_ref, session_cli, session_action, sent_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                params![
                    row.project,
                    row.task,
                    row.n,
                    row.seat,
                    row.generation,
                    row.contract_version,
                    row.adopted_version,
                    row.pending_update,
                    row.state.as_str(),
                    row.candidate,
                    row.report,
                    row.safe_point,
                    row.started,
                    row.updated,
                    row.session_ref,
                    row.session_cli,
                    row.session_action,
                    row.sent_at,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    pub fn get_attempt(&self, project: &str, task: &str, n: i64) -> GearResult<Option<AttemptRow>> {
        let row: Option<AttemptRow> = self
            .tx
            .query_row(
                &attempt_select(" WHERE project=?1 AND task=?2 AND n=?3"),
                params![project, task, n],
                attempt_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    /// All attempts of a task ordered by n.
    pub fn attempts_of_task(&self, project: &str, task: &str) -> GearResult<Vec<AttemptRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&attempt_select(" WHERE project=?1 AND task=?2 ORDER BY n"))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project, task], attempt_from_row)
            .map_err(map_sqlite)?;
        let mut attempts = Vec::new();
        for row in rows {
            attempts.push(row.map_err(map_sqlite)?);
        }
        Ok(attempts)
    }

    /// The seat's attempts in Delivered/Started/Stopped/Unknown (design/06 "in progress"),
    /// ordered by (task, n).
    pub fn in_progress_attempts_of_seat(&self, seat: &str) -> GearResult<Vec<AttemptRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&attempt_select(
                " WHERE seat=?1 AND state IN ('delivered', 'started', 'stopped', 'unknown')
                 ORDER BY task, n",
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![seat], attempt_from_row)
            .map_err(map_sqlite)?;
        let mut attempts = Vec::new();
        for row in rows {
            attempts.push(row.map_err(map_sqlite)?);
        }
        Ok(attempts)
    }

    /// The seat's non-terminal attempts — in progress plus Submitted
    /// (Delivered/Started/Stopped/Unknown/Submitted) — ordered by (task, n).
    /// The engine's lane view (design/09) reads the current attempt from here.
    /// The seat's open attempts, the one being worked on first: handed-in
    /// (Submitted) attempts wait for review in the background and come last
    /// (never-stop, L-45).
    pub fn open_attempts_of_seat(&self, seat: &str) -> GearResult<Vec<AttemptRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&attempt_select(
                " WHERE seat=?1
                   AND state IN ('delivered', 'started', 'stopped', 'unknown', 'submitted')
                 ORDER BY state = 'submitted', task, n",
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![seat], attempt_from_row)
            .map_err(map_sqlite)?;
        let mut attempts = Vec::new();
        for row in rows {
            attempts.push(row.map_err(map_sqlite)?);
        }
        Ok(attempts)
    }

    /// Replace every column of an existing attempt (raw write). Missing → NoTarget.
    ///
    /// The step times are **not** written here: they belong to the step that
    /// happened, and a row replacement must never move or clear them.
    pub fn update_attempt(&self, row: &AttemptRow) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE attempts
                 SET seat=?4, generation=?5, contract_version=?6, adopted_version=?7,
                     pending_update=?8, state=?9, candidate=?10, report=?11, safe_point=?12,
                     started=?13, updated=?14
                 WHERE project=?1 AND task=?2 AND n=?3",
                params![
                    row.project,
                    row.task,
                    row.n,
                    row.seat,
                    row.generation,
                    row.contract_version,
                    row.adopted_version,
                    row.pending_update,
                    row.state.as_str(),
                    row.candidate,
                    row.report,
                    row.safe_point,
                    row.started,
                    row.updated,
                ],
            )
            .map_err(map_sqlite)?;
        require_written(
            changed,
            "attempt",
            &format!("{} {} in project {}", row.task, row.n, row.project),
        )
    }

    /// Write the time of one step of an attempt (T-38).
    ///
    /// Only the step's own column is written, so no step can overwrite
    /// another's time. The first time wins: a step that is written twice
    /// (a repeated delivery, a replayed settle) keeps the first reading.
    /// The attempt must exist → else `NoTarget`.
    pub fn set_attempt_time(
        &self,
        project: &str,
        task: &str,
        n: i64,
        step: AttemptStep,
        now: Ts,
    ) -> GearResult<()> {
        let column = step.column();
        let changed = self
            .tx
            .execute(
                &format!(
                    "UPDATE attempts SET {column}=COALESCE({column}, ?4)
                     WHERE project=?1 AND task=?2 AND n=?3"
                ),
                params![project, task, n, now],
            )
            .map_err(map_sqlite)?;
        require_written(
            changed,
            "attempt",
            &format!("{task} attempt {n} in project {project}"),
        )
    }

    /// The step times of one attempt. A step that has not happened is None.
    pub fn attempt_times(&self, project: &str, task: &str, n: i64) -> GearResult<AttemptTimes> {
        let row: Option<(Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>)> =
            self.tx
                .query_row(
                    "SELECT delivered_at, started_at, handed_in_at, verdict_at, settled_at, merged_at
                     FROM attempts WHERE project=?1 AND task=?2 AND n=?3",
                    params![project, task, n],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(map_sqlite)?;
        Ok(row.map_or_else(AttemptTimes::default, |t| AttemptTimes {
            delivered: t.0,
            started: t.1,
            handed_in: t.2,
            verdict: t.3,
            settled: t.4,
            merged: t.5,
        }))
    }

    /// The step times of every attempt of a task, ordered by attempt number.
    /// An attempt with no row (never created) is not listed.
    pub fn attempt_times_of_task(
        &self,
        project: &str,
        task: &str,
    ) -> GearResult<Vec<(i64, AttemptTimes)>> {
        let mut stmt = self
            .tx
            .prepare_cached(
                "SELECT n, delivered_at, started_at, handed_in_at, verdict_at, settled_at, merged_at
                 FROM attempts WHERE project=?1 AND task=?2 ORDER BY n",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project, task], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    AttemptTimes {
                        delivered: row.get(1)?,
                        started: row.get(2)?,
                        handed_in: row.get(3)?,
                        verdict: row.get(4)?,
                        settled: row.get(5)?,
                        merged: row.get(6)?,
                    },
                ))
            })
            .map_err(map_sqlite)?;
        let mut times = Vec::new();
        for row in rows {
            times.push(row.map_err(map_sqlite)?);
        }
        Ok(times)
    }

    /// Append a verdict (no uniqueness: history is kept).
    pub fn insert_verdict(&self, row: &VerdictRow) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO verdicts
                   (project, task, attempt, contract_version, candidate, passed, evidence, by_seat, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    row.project,
                    row.task,
                    row.attempt,
                    row.contract_version,
                    row.candidate,
                    row.passed as i64,
                    row.evidence,
                    row.by_seat,
                    row.created,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// Verdicts of a task ordered by (created, attempt).
    pub fn verdicts_of_task(&self, project: &str, task: &str) -> GearResult<Vec<VerdictRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(
                "SELECT project, task, attempt, contract_version, candidate, passed, evidence,
                        by_seat, created
                 FROM verdicts WHERE project=?1 AND task=?2 ORDER BY created, attempt",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project, task], verdict_from_row)
            .map_err(map_sqlite)?;
        let mut verdicts = Vec::new();
        for row in rows {
            verdicts.push(row.map_err(map_sqlite)?);
        }
        Ok(verdicts)
    }

    // ---- voyages (T-22)

    /// Store or update a milestone as a voyage. The departure is fixed once: a
    /// later apply may change the fields but never moves the departure.
    pub fn upsert_voyage(&self, row: &VoyageRow) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO voyages
                   (project, milestone, title, done, bar, eta, departure, departure_tasks, updated, departure_estimate_min)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(project, milestone) DO UPDATE SET
                   title=excluded.title, done=excluded.done, bar=excluded.bar, eta=excluded.eta,
                   departure=COALESCE(voyages.departure, excluded.departure),
                   departure_tasks=COALESCE(voyages.departure_tasks, excluded.departure_tasks),
                   departure_estimate_min=COALESCE(voyages.departure_estimate_min, excluded.departure_estimate_min),
                   updated=excluded.updated",
                params![
                    row.project,
                    row.milestone,
                    row.title,
                    row.done,
                    row.bar,
                    row.eta,
                    row.departure,
                    row.departure_tasks,
                    row.updated,
                    row.departure_estimate_min,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// Fix the departure of a milestone whose first tasks are created now: the
    /// row is created (words NULL) when the plan has no block for it yet, and
    /// an existing departure is never moved (design of T-22, review R1/R2).
    pub fn set_voyage_departure(
        &self,
        project: &str,
        milestone: &str,
        departure: Ts,
        tasks: i64,
        now: Ts,
    ) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO voyages
                   (project, milestone, title, done, bar, eta, departure, departure_tasks, updated)
                 VALUES (?1, ?2, '', NULL, NULL, NULL, ?3, ?4, ?5)
                 ON CONFLICT(project, milestone) DO UPDATE SET
                   departure=COALESCE(voyages.departure, excluded.departure),
                   departure_tasks=COALESCE(voyages.departure_tasks, excluded.departure_tasks),
                   updated=excluded.updated",
                params![project, milestone, departure, tasks, now],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// Preserve the first known departure estimate. Callers recover historical
    /// values from the departure plan, never from today's edited scope.
    pub fn set_voyage_departure_estimate(
        &self, project: &str, milestone: &str, minutes: i64,
    ) -> GearResult<()> {
        self.tx.execute(
            "UPDATE voyages SET departure_estimate_min=?3
             WHERE project=?1 AND milestone=?2 AND departure_estimate_min IS NULL",
            params![project, milestone, minutes],
        ).map_err(map_sqlite)?;
        Ok(())
    }

    /// Empty a voyage's departure: a milestone that lost every task no longer
    /// has one, so a reused milestone id starts its clock when its new tasks
    /// come (T-239). `upsert_voyage` cannot do this: it COALESCEs the columns.
    pub fn clear_voyage_departure(
        &self, project: &str, milestone: &str,
    ) -> GearResult<()> {
        self.tx.execute(
            "UPDATE voyages
             SET departure=NULL, departure_tasks=NULL, departure_estimate_min=NULL
             WHERE project=?1 AND milestone=?2",
            params![project, milestone],
        ).map_err(map_sqlite)?;
        Ok(())
    }

    /// One milestone's voyage, if it has one.
    pub fn get_voyage(&self, project: &str, milestone: &str) -> GearResult<Option<VoyageRow>> {
        let row: Option<VoyageRow> = self
            .tx
            .query_row(
                "SELECT project, milestone, title, done, bar, eta, departure, departure_tasks, updated, departure_estimate_min
                 FROM voyages WHERE project=?1 AND milestone=?2",
                params![project, milestone],
                voyage_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    /// The voyages of a project, in milestone-id order.
    pub fn voyages_of_project(&self, project: &str) -> GearResult<Vec<VoyageRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(
                "SELECT project, milestone, title, done, bar, eta, departure, departure_tasks, updated, departure_estimate_min
                 FROM voyages WHERE project=?1 ORDER BY milestone",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project], voyage_from_row)
            .map_err(map_sqlite)?;
        let mut voyages = Vec::new();
        for row in rows {
            voyages.push(row.map_err(map_sqlite)?);
        }
        Ok(voyages)
    }

    // ---- review blocks (T-23)

    /// Record the blocking findings of a review (the ids the reviewer named).
    pub fn set_review_blocking(
        &self,
        project: &str,
        task: &str,
        attempt: i64,
        ids: &str,
        now: Ts,
    ) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO review_blocks (project, task, attempt, ids, created)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(project, task, attempt) DO UPDATE SET ids=excluded.ids",
                params![project, task, attempt, ids, now],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// The blocking findings of a review, if it named any.
    pub fn review_blocking(
        &self,
        project: &str,
        task: &str,
        attempt: i64,
    ) -> GearResult<Option<String>> {
        let ids: Option<String> = self
            .tx
            .query_row(
                "SELECT ids FROM review_blocks WHERE project=?1 AND task=?2 AND attempt=?3",
                params![project, task, attempt],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(ids)
    }

    // ---- review verdicts (T-2)

    /// Store the review verdict of one attempt. One record per (project, task,
    /// attempt): a second one for the same attempt is a constraint violation.
    pub fn insert_review_verdict(&self, row: &ReviewVerdictRow) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO review_verdicts
                   (project, task, attempt, passed, findings, by_seat, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    row.project,
                    row.task,
                    row.attempt,
                    row.passed as i64,
                    row.findings,
                    row.by_seat,
                    row.created,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// The review verdict of one attempt, if there is one.
    pub fn get_review_verdict(
        &self,
        project: &str,
        task: &str,
        attempt: i64,
    ) -> GearResult<Option<ReviewVerdictRow>> {
        let row: Option<ReviewVerdictRow> = self
            .tx
            .query_row(
                "SELECT project, task, attempt, passed, findings, by_seat, created
                 FROM review_verdicts WHERE project=?1 AND task=?2 AND attempt=?3",
                params![project, task, attempt],
                review_verdict_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    /// The review verdicts of a task, ordered by attempt.
    pub fn review_verdicts_of_task(
        &self,
        project: &str,
        task: &str,
    ) -> GearResult<Vec<ReviewVerdictRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(
                "SELECT project, task, attempt, passed, findings, by_seat, created
                 FROM review_verdicts WHERE project=?1 AND task=?2 ORDER BY attempt",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project, task], review_verdict_from_row)
            .map_err(map_sqlite)?;
        let mut verdicts = Vec::new();
        for row in rows {
            verdicts.push(row.map_err(map_sqlite)?);
        }
        Ok(verdicts)
    }

    /// The review verdicts of a project whose attempt is still handed in
    /// (Submitted) — the ones a settle call still has to act on (T-13), oldest
    /// first. A verdict whose attempt was already settled is not listed again;
    /// one whose task finished meanwhile is consumed once (the attempt is
    /// closed there) and then drops out too (review R-67, P1).
    pub fn reviews_to_settle(&self, project: &str) -> GearResult<Vec<ReviewVerdictRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(
                "SELECT r.project, r.task, r.attempt, r.passed, r.findings, r.by_seat, r.created
                 FROM review_verdicts r
                 JOIN attempts a ON a.project=r.project AND a.task=r.task AND a.n=r.attempt
                 WHERE r.project=?1 AND a.state='submitted'
                 ORDER BY r.created, r.task, r.attempt",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project], review_verdict_from_row)
            .map_err(map_sqlite)?;
        let mut verdicts = Vec::new();
        for row in rows {
            verdicts.push(row.map_err(map_sqlite)?);
        }
        Ok(verdicts)
    }

    // ---- v6: the M4 store API (§1 and §13.1 of gear/voyages/M4/design.md)

    /// The session the attempt ran in (§3): written at check-in and again at
    /// hand-in, when the agent may have compacted or been restarted.
    pub fn set_attempt_session(
        &self,
        project: &str,
        task: &str,
        n: i64,
        session_ref: &str,
        cli: &str,
    ) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE attempts SET session_ref=?4, session_cli=?5
                 WHERE project=?1 AND task=?2 AND n=?3",
                params![project, task, n, session_ref, cli],
            )
            .map_err(map_sqlite)?;
        require_written(
            changed,
            "attempt",
            &format!("{task} attempt {n} in project {project}"),
        )
    }

    /// Refresh the attempt's session from its current binding, including an
    /// unknown session after a replacement has not reported one yet.
    pub fn refresh_attempt_session(
        &self, project: &str, task: &str, n: i64, session_ref: Option<&str>, cli: &str,
    ) -> GearResult<()> {
        let changed = self.tx.execute(
            "UPDATE attempts SET session_ref=?4, session_cli=?5 WHERE project=?1 AND task=?2 AND n=?3",
            params![project, task, n, session_ref, cli],
        ).map_err(map_sqlite)?;
        require_written(changed, "attempt", &format!("{task} attempt {n} in project {project}"))
    }

    /// What the engine did before the packet (§3.2): keep | new | compact |
    /// resume | fresh | new-unconfirmed | compact-fallback | resume-fallback.
    pub fn set_attempt_action(
        &self,
        project: &str,
        task: &str,
        n: i64,
        action: &str,
    ) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE attempts SET session_action=?4
                 WHERE project=?1 AND task=?2 AND n=?3",
                params![project, task, n, action],
            )
            .map_err(map_sqlite)?;
        require_written(
            changed,
            "attempt",
            &format!("{task} attempt {n} in project {project}"),
        )
    }

    /// When the packet was actually sent through Herdr (§3.2).
    pub fn set_attempt_sent(&self, project: &str, task: &str, n: i64, at: Ts) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE attempts SET sent_at=?4 WHERE project=?1 AND task=?2 AND n=?3",
                params![project, task, n, at],
            )
            .map_err(map_sqlite)?;
        require_written(
            changed,
            "attempt",
            &format!("{task} attempt {n} in project {project}"),
        )
    }

    /// Insert a round (§4). A key that exists → BadFormat: one round per
    /// (project, task, attempt, role).
    pub fn insert_round(&self, r: &NewRound) -> GearResult<()> {
        self.tx
            .execute(
                "INSERT INTO rounds
                   (project, task, attempt, role, seat, generation, cli, model, start_at, end_at,
                    sessions, active_s, paused_s, input, output, cache_read, cache_write, reasoning,
                    cost_usd, cost_source, counted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, '[]', 0, 0,
                         NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL)",
                params![
                    r.key.project,
                    r.key.task,
                    r.key.attempt,
                    r.key.role,
                    r.seat,
                    r.generation,
                    r.cli,
                    r.model,
                    r.start_at,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// The open round of a seat (`end_at IS NULL`): the one it is working on.
    pub fn open_round_of_seat(&self, seat: &str) -> GearResult<Option<RoundRow>> {
        let row: Option<RoundRow> = self
            .tx
            .query_row(
                &round_select(" WHERE seat=?1 AND end_at IS NULL ORDER BY start_at DESC LIMIT 1"),
                params![seat],
                round_from_row,
            )
            .optional()
            .map_err(map_sqlite)?;
        Ok(row)
    }

    /// Close a round: the next round's start on the seat, or the seat's
    /// release (§4.1).
    pub fn close_round(&self, key: &RoundKey, end_at: Ts) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE rounds SET end_at=?5
                 WHERE project=?1 AND task=?2 AND attempt=?3 AND role=?4",
                params![key.project, key.task, key.attempt, key.role, end_at],
            )
            .map_err(map_sqlite)?;
        require_written(
            changed,
            "round",
            &format!(
                "{} attempt {} {} in project {}",
                key.task, key.attempt, key.role, key.project
            ),
        )
    }

    /// Add a session the seat ran in during the round's window (§4.3). A
    /// session that is already listed is not added twice.
    pub fn add_round_session(&self, key: &RoundKey, session_ref: &str) -> GearResult<()> {
        let stored: Option<String> = self
            .tx
            .query_row(
                "SELECT sessions FROM rounds
                 WHERE project=?1 AND task=?2 AND attempt=?3 AND role=?4",
                params![key.project, key.task, key.attempt, key.role],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        let mut sessions = match stored {
            Some(text) => serde_json::from_str::<Vec<String>>(&text).map_err(|error| {
                GearError::new(ErrorCode::BadFormat, format!("round sessions: {error}"))
            })?,
            None => {
                return Err(GearError::new(
                    ErrorCode::NoTarget,
                    format!(
                        "no round {} attempt {} {} in project {}",
                        key.task, key.attempt, key.role, key.project
                    ),
                ))
            }
        };
        if sessions.iter().any(|session| session == session_ref) {
            return Ok(());
        }
        sessions.push(session_ref.to_string());
        let text = serde_json::to_string(&sessions)
            .map_err(|error| GearError::new(ErrorCode::BadFormat, error.to_string()))?;
        self.tx
            .execute(
                "UPDATE rounds SET sessions=?5
                 WHERE project=?1 AND task=?2 AND attempt=?3 AND role=?4",
                params![key.project, key.task, key.attempt, key.role, text],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// Add to the round's paused time (§5.1): the clock pauses while the seat
    /// is quota-parked or the task is in a declared wait.
    pub fn add_round_pause(&self, key: &RoundKey, secs: i64) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE rounds SET paused_s = paused_s + ?5
                 WHERE project=?1 AND task=?2 AND attempt=?3 AND role=?4",
                params![key.project, key.task, key.attempt, key.role, secs],
            )
            .map_err(map_sqlite)?;
        require_written(
            changed,
            "round",
            &format!(
                "{} attempt {} {} in project {}",
                key.task, key.attempt, key.role, key.project
            ),
        )
    }

    /// Write the round's active time (the clock without its pauses).
    pub fn set_round_active(&self, key: &RoundKey, active_s: i64) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE rounds SET active_s=?5
                 WHERE project=?1 AND task=?2 AND attempt=?3 AND role=?4",
                params![key.project, key.task, key.attempt, key.role, active_s],
            )
            .map_err(map_sqlite)?;
        require_written(
            changed,
            "round",
            &format!(
                "{} attempt {} {} in project {}",
                key.task, key.attempt, key.role, key.project
            ),
        )
    }

    /// Write the round's tokens and cost, and the time they were read (§4.3).
    pub fn set_round_tokens(&self, key: &RoundKey, t: &RoundTokens, at: Ts) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE rounds
                 SET input=?5, output=?6, cache_read=?7, cache_write=?8, reasoning=?9,
                     cost_usd=?10, cost_source=?11, counted_at=?12
                 WHERE project=?1 AND task=?2 AND attempt=?3 AND role=?4",
                params![
                    key.project,
                    key.task,
                    key.attempt,
                    key.role,
                    t.input,
                    t.output,
                    t.cache_read,
                    t.cache_write,
                    t.reasoning,
                    t.cost_usd,
                    t.cost_source,
                    at,
                ],
            )
            .map_err(map_sqlite)?;
        require_written(
            changed,
            "round",
            &format!(
                "{} attempt {} {} in project {}",
                key.task, key.attempt, key.role, key.project
            ),
        )
    }

    /// The rounds of a task, oldest first.
    pub fn rounds_of_task(&self, project: &str, task: &str) -> GearResult<Vec<RoundRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&round_select(" WHERE project=?1 AND task=?2 ORDER BY start_at, role"))
            .map_err(map_sqlite)?;
        let rows = stmt.query_map(params![project, task], round_from_row).map_err(map_sqlite)?;
        let mut rounds = Vec::new();
        for row in rows {
            rounds.push(row.map_err(map_sqlite)?);
        }
        Ok(rounds)
    }

    /// The rounds of a project, oldest first.
    pub fn rounds_of_project(&self, project: &str) -> GearResult<Vec<RoundRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&round_select(" WHERE project=?1 ORDER BY start_at, task, role"))
            .map_err(map_sqlite)?;
        let rows = stmt.query_map(params![project], round_from_row).map_err(map_sqlite)?;
        let mut rounds = Vec::new();
        for row in rows {
            rounds.push(row.map_err(map_sqlite)?);
        }
        Ok(rounds)
    }

    /// The rounds a project's overrun step still has to look at (T-406): the
    /// open ones (no `end_at`) and the ended ones that still owe an answer on
    /// their `overrun:<task>:<role>` thread (T-381's resolution). A settled
    /// round's item is resolved on the tick that looks at it, so the round is
    /// no longer selected after that.
    pub fn rounds_to_process(&self, project: &str) -> GearResult<Vec<RoundRow>> {
        let mut stmt = self
            .tx
            .prepare(&round_select(
                " WHERE project=?1
                    AND (end_at IS NULL OR EXISTS (
                      SELECT 1 FROM messages m
                       WHERE m.project=rounds.project
                         AND m.thread='overrun:'||rounds.task||':'||rounds.role
                         AND m.attempt=rounds.attempt AND m.kind='system'
                         AND m.state IN ('open', 'waiting')))
                  ORDER BY start_at, task, role",
            ))
            .map_err(map_sqlite)?;
        let rows = stmt.query_map(params![project], round_from_row).map_err(map_sqlite)?;
        let mut rounds = Vec::new();
        for row in rows {
            rounds.push(row.map_err(map_sqlite)?);
        }
        Ok(rounds)
    }

    /// Store a friction report (§6). Returns its id, `F-<n>`.
    ///
    /// The time of the report is the store's own: the input carries the
    /// report, not the moment it was written (§13.1).
    pub fn insert_friction(&self, f: &NewFriction) -> GearResult<String> {
        let id = self.next_id(&f.project, 'F')?;
        self.tx
            .execute(
                "INSERT INTO frictions
                   (project, id, task, attempt, role, seat, body, required, created,
                    closed_at, outcome)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                         CAST(strftime('%s', 'now') AS INTEGER), NULL, NULL)",
                params![
                    f.project,
                    id,
                    f.task,
                    f.attempt,
                    f.role,
                    f.seat,
                    f.body,
                    f.required as i64,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(id)
    }

    /// The friction reports that are not closed yet (§6.4).
    pub fn open_frictions(&self, project: &str) -> GearResult<Vec<FrictionRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&friction_select(" WHERE project=?1 AND closed_at IS NULL ORDER BY created, id"))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project], friction_from_row)
            .map_err(map_sqlite)?;
        let mut frictions = Vec::new();
        for row in rows {
            frictions.push(row.map_err(map_sqlite)?);
        }
        Ok(frictions)
    }

    /// Every friction report of the project, closed ones too.
    pub fn frictions_of_project(&self, project: &str) -> GearResult<Vec<FrictionRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&friction_select(" WHERE project=?1 ORDER BY created, id"))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project], friction_from_row)
            .map_err(map_sqlite)?;
        let mut frictions = Vec::new();
        for row in rows {
            frictions.push(row.map_err(map_sqlite)?);
        }
        Ok(frictions)
    }

    /// Close a friction report with what the captain did about it (§6.4).
    pub fn close_friction(
        &self,
        project: &str,
        id: &str,
        outcome: &str,
        at: Ts,
    ) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE frictions SET closed_at=?3, outcome=?4 WHERE project=?1 AND id=?2",
                params![project, id, at, outcome],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "friction", &format!("{id} in project {project}"))
    }

    /// Log a decision (§7). A key that is already there is skipped: false says
    /// the decision was already logged, true says it was written now. The
    /// table is append-only (§7.3).
    ///
    /// The time of the entry is the store's own: the input carries the
    /// decision, not the moment it was written (§13.1).
    pub fn insert_decision_if_new(&self, d: &NewDecision) -> GearResult<bool> {
        let changed = self
            .tx
            .execute(
                "INSERT INTO decisions
                   (project, key, seq, voyage, at, kind, what, why, alt, changes, star, source,
                    line, plan_version, created)
                 VALUES (?1, ?2,
                         (SELECT COALESCE(MAX(seq), 0) + 1 FROM decisions WHERE project=?1),
                         ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                         CAST(strftime('%s', 'now') AS INTEGER))
                 ON CONFLICT(project, key) DO NOTHING",
                params![
                    d.project,
                    d.key,
                    d.voyage,
                    d.at,
                    d.kind,
                    d.what,
                    d.why,
                    d.alt,
                    d.changes,
                    d.star as i64,
                    d.source,
                    d.line,
                    d.plan_version,
                ],
            )
            .map_err(map_sqlite)?;
        Ok(changed > 0)
    }

    /// The decisions of a voyage, in the order they appeared (§7).
    pub fn decisions_of(&self, project: &str, voyage: &str) -> GearResult<Vec<DecisionRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(&decision_select(" WHERE project=?1 AND voyage=?2 ORDER BY seq"))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project, voyage], decision_from_row)
            .map_err(map_sqlite)?;
        let mut decisions = Vec::new();
        for row in rows {
            decisions.push(row.map_err(map_sqlite)?);
        }
        Ok(decisions)
    }

    /// Open a pause: the seat cannot work from `at` on (§5.4).
    ///
    /// The stretches of one kind do not overlap: a pause of that kind that is
    /// already open stays. A pause of **another** kind is independent — a
    /// quota park and a declared wait may be there at once — so it neither
    /// stops this one nor is dropped by it. The row key is
    /// (project, seat, kind, start_at).
    pub fn open_pause(
        &self,
        project: &str,
        seat: &str,
        kind: &str,
        reason: &str,
        at: Ts,
    ) -> GearResult<()> {
        // A pause of this kind that is already open stays: the stretches of
        // one kind do not overlap (§5.4). A pause of **another** kind is a
        // different thing — a quota park and a declared wait are independent
        // and may be there at once — so it does not stop this one.
        let open: Option<i64> = self
            .tx
            .query_row(
                "SELECT count(*) FROM seat_pauses
                 WHERE project=?1 AND seat=?2 AND kind=?3 AND end_at IS NULL",
                params![project, seat, kind],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        if open.unwrap_or(0) > 0 {
            return Ok(());
        }
        // The row key is (project, seat, kind, start_at): a pause of this kind
        // that starts in a second one of them already starts in is that row
        // again — it is reopened, not a second row with a time of its own.
        let existing: Option<i64> = self
            .tx
            .query_row(
                "SELECT count(*) FROM seat_pauses
                 WHERE project=?1 AND seat=?2 AND kind=?3 AND start_at=?4",
                params![project, seat, kind, at],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        if existing.unwrap_or(0) > 0 {
            self.tx
                .execute(
                    "UPDATE seat_pauses SET reason=?5, end_at=NULL
                     WHERE project=?1 AND seat=?2 AND kind=?3 AND start_at=?4",
                    params![project, seat, kind, at, reason],
                )
                .map_err(map_sqlite)?;
            return Ok(());
        }
        self.tx
            .execute(
                "INSERT INTO seat_pauses (project, seat, kind, reason, start_at, end_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
                params![project, seat, kind, reason, at],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// Close the open pause of that kind: the seat can work again (§5.4).
    ///
    /// The pause never ends before it began: the end is the later of the two
    /// times.
    pub fn close_pause(&self, project: &str, seat: &str, kind: &str, at: Ts) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE seat_pauses SET end_at=max(?4, start_at)
                 WHERE project=?1 AND seat=?2 AND kind=?3 AND end_at IS NULL",
                params![project, seat, kind, at],
            )
            .map_err(map_sqlite)?;
        require_written(
            changed,
            "pause",
            &format!("{kind} of seat {seat} in project {project}"),
        )
    }

    /// The pauses of a project, oldest first.
    pub fn pauses_of_project(&self, project: &str) -> GearResult<Vec<PauseRow>> {
        let mut stmt = self
            .tx
            .prepare_cached(
                "SELECT project, seat, kind, reason, start_at, end_at
                 FROM seat_pauses WHERE project=?1 ORDER BY start_at, seat",
            )
            .map_err(map_sqlite)?;
        let rows = stmt.query_map(params![project], pause_from_row).map_err(map_sqlite)?;
        let mut pauses = Vec::new();
        for row in rows {
            pauses.push(row.map_err(map_sqlite)?);
        }
        Ok(pauses)
    }

    /// The estimate the fail verdict asked for (§5.1): the budget of the
    /// rework round.
    pub fn set_task_rework_estimate(
        &self,
        project: &str,
        task: &str,
        min: Option<i64>,
    ) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE tasks SET rework_estimate_min=?3 WHERE project=?1 AND id=?2",
                params![project, task, min],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "task", &format!("{task} in project {project}"))
    }

    /// A declared wait (§5.3): the task cannot finish until `on` is merged.
    pub fn set_task_wait(&self, project: &str, task: &str, on: &str, cond: &str) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE tasks SET wait_on=?3, wait_cond=?4 WHERE project=?1 AND id=?2",
                params![project, task, on, cond],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "task", &format!("{task} in project {project}"))
    }

    /// End a declared wait (§5.3).
    pub fn clear_task_wait(&self, project: &str, task: &str) -> GearResult<()> {
        let changed = self
            .tx
            .execute(
                "UPDATE tasks SET wait_on=NULL, wait_cond=NULL WHERE project=?1 AND id=?2",
                params![project, task],
            )
            .map_err(map_sqlite)?;
        require_written(changed, "task", &format!("{task} in project {project}"))
    }

    /// The budget of a round in minutes (§5.1), in one place: the overrun
    /// question, the friction check and the packet's budget line all read it.
    ///
    /// - a dev round (attempt 1): the task's `estimate_min`;
    /// - a rework round (attempt n >= 2): the estimate the fail verdict
    ///   asked for, or `max(10, ceil(estimate_min / 2))`;
    /// - a review round: 1.5 × the median active time of the last 10
    ///   finished review rounds of the same model, at least 10 min; with
    ///   fewer than 3 such rounds, 30 min. Only rounds that measured
    ///   activity count: a round with `active_s` 0 says nothing about how
    ///   long a review takes, so it is not part of the sample.
    pub fn round_budget_min(
        &self,
        project: &str,
        task: &str,
        attempt: i64,
        role: &str,
        model: Option<&str>,
    ) -> GearResult<i64> {
        if role == "review" {
            return self.review_budget_min(project, model);
        }
        let estimate: Option<i64> = self
            .tx
            .query_row(
                "SELECT estimate_min FROM tasks WHERE project=?1 AND id=?2",
                params![project, task],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?
            .flatten();
        if attempt >= 2 {
            let rework: Option<i64> = self
                .tx
                .query_row(
                    "SELECT rework_estimate_min FROM tasks WHERE project=?1 AND id=?2",
                    params![project, task],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_sqlite)?
                .flatten();
            if let Some(minutes) = rework {
                return Ok(minutes.max(1));
            }
            let half = estimate.map(|minutes| (minutes + 1) / 2).unwrap_or(0);
            return Ok(half.max(10));
        }
        Ok(estimate.unwrap_or(DEFAULT_ROUND_BUDGET_MIN))
    }

    /// The measured budget of a review round (§5.1).
    ///
    /// The median is taken over the last 10 ended review rounds that
    /// measured some activity (`active_s > 0`). A round whose usage was
    /// never read carries 0 and would drag the median to the floor; it says
    /// nothing about how long a review takes, so it is left out of the
    /// sample instead.
    fn review_budget_min(&self, project: &str, model: Option<&str>) -> GearResult<i64> {
        let mut stmt = self
            .tx
            .prepare_cached(
                "SELECT active_s FROM rounds
                 WHERE project=?1 AND role='review' AND end_at IS NOT NULL AND model IS ?2
                   AND active_s > 0
                 ORDER BY end_at DESC LIMIT 10",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![project, model], |row| row.get::<_, i64>(0))
            .map_err(map_sqlite)?;
        let mut minutes = Vec::new();
        for row in rows {
            minutes.push(row.map_err(map_sqlite)? as f64 / 60.0);
        }
        if minutes.len() < 3 {
            return Ok(DEFAULT_REVIEW_BUDGET_MIN);
        }
        minutes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = if minutes.len() % 2 == 1 {
            minutes[minutes.len() / 2]
        } else {
            let upper = minutes.len() / 2;
            (minutes[upper - 1] + minutes[upper]) / 2.0
        };
        let budget = (median * 1.5).ceil() as i64;
        Ok(budget.max(10))
    }

    // ---- stations (M5 S3)

    /// Write the voyage's stations, in plan order.
    ///
    /// They are kept in `meta` under `voyage-stations:<project>:<milestone>`,
    /// one `<id>: <needs>` line per station, joined by "\n". An empty list
    /// writes "".
    pub fn set_voyage_stations(
        &self,
        project: &str,
        milestone: &str,
        stations: &[VoyageStation],
    ) -> GearResult<()> {
        let text = stations
            .iter()
            .map(|station| format!("{}: {}", station.id, station.needs))
            .collect::<Vec<_>>()
            .join("\n");
        self.set_meta(&format!("voyage-stations:{project}:{milestone}"), &text)
    }

    /// The voyage's stations, in plan order. A missing or empty value is an
    /// empty list; each line is split at its first `": "`.
    pub fn voyage_stations(
        &self,
        project: &str,
        milestone: &str,
    ) -> GearResult<Vec<VoyageStation>> {
        let raw = self
            .get_meta(&format!("voyage-stations:{project}:{milestone}"))?
            .unwrap_or_default();
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        Ok(raw
            .lines()
            .filter_map(|line| {
                let (id, needs) = line.split_once(": ")?;
                Some(VoyageStation {
                    id: id.to_string(),
                    needs: needs.to_string(),
                })
            })
            .collect())
    }

    /// Write the station a task belongs to, under
    /// `task-station:<project>:<task>`; `None` writes "".
    pub fn set_task_station(
        &self,
        project: &str,
        task: &str,
        station: Option<&str>,
    ) -> GearResult<()> {
        self.set_meta(
            &format!("task-station:{project}:{task}"),
            station.unwrap_or(""),
        )
    }

    /// The station a task belongs to; a missing or empty value is `None`.
    pub fn task_station(&self, project: &str, task: &str) -> GearResult<Option<String>> {
        let raw = self
            .get_meta(&format!("task-station:{project}:{task}"))?
            .unwrap_or_default();
        Ok((!raw.is_empty()).then_some(raw))
    }
}

/// Test helpers (enable with feature-less `cfg(test)` in this crate or via
/// `gear_store::testing` from other crates' tests).
pub mod testing {
    use super::*;

    /// Insert an onboarded seat (state Active, generation 1, `profile = kind`) with an
    /// Active binding `(seat, 1, project session, pane_id, kind)`. The project must exist.
    /// Production code never does this: real onboarding goes through the engine.
    pub fn seed_active_seat(
        tx: &Tx<'_>,
        project: &str,
        name: &str,
        role: SeatRole,
        pane_id: &str,
        kind: &str,
        now: Ts,
    ) -> GearResult<SeatRow> {
        let project_row = tx.get_project(project)?.ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!("project {project} does not exist"),
            )
        })?;
        let seat = SeatRow {
            id: format!("{project}:{name}"),
            project: project.to_string(),
            role,
            name: name.to_string(),
            profile: kind.to_string(),
            generation: 1,
            state: SeatState::Active,
            state_since: now,
            state_reason: None,
            task_id: None,
        };
        tx.insert_seat(&seat)?;
        let binding = BindingRow {
            seat: seat.id.clone(),
            generation: 1,
            session: project_row.herdr_session,
            pane_id: pane_id.to_string(),
            kind: kind.to_string(),
            session_ref: None,
            status: BindingStatus::Active,
            handover_id: None,
            created: now,
        };
        tx.insert_binding(&binding)?;
        Ok(seat)
    }

    /// Insert a temp seat bound to `task` (the one task it works on for life),
    /// Active at generation 1 with an Active binding. The project must exist.
    pub fn seed_temp_seat(
        tx: &Tx<'_>,
        project: &str,
        name: &str,
        task: &str,
        pane_id: &str,
        kind: &str,
        now: Ts,
    ) -> GearResult<SeatRow> {
        let mut seat = seed_active_seat(tx, project, name, SeatRole::Temp, pane_id, kind, now)?;
        tx.set_seat_task(&seat.id, task)?;
        seat.task_id = Some(task.to_string());
        Ok(seat)
    }

    /// Insert a project with quota lanes=4 temps=2, captain excluded, config_toml "", version 1.
    pub fn seed_project(
        tx: &Tx<'_>,
        project: &str,
        session: &str,
        now: Ts,
    ) -> GearResult<ProjectRow> {
        let row = ProjectRow {
            id: project.to_string(),
            herdr_session: session.to_string(),
            config_toml: String::new(),
            config_version: 1,
            quota_lanes: 4,
            quota_temps: 2,
            quota_includes_captain: false,
            plan_version: 0,
            created: now,
        };
        tx.insert_project(&row)?;
        Ok(row)
    }
}

// ---------------------------------------------------------------- helpers

/// A raw state write must have moved the row it names: `Ok` means the row
/// exists and was written, `NoTarget` names the missing target.
fn require_written(changed: usize, what: &str, target: &str) -> GearResult<()> {
    if changed == 0 {
        return Err(GearError::new(
            ErrorCode::NoTarget,
            format!("no {what} {target}"),
        ));
    }
    Ok(())
}

/// One conversion for every rusqlite error. Lock contention is `Locked`
/// (safe to retry, nothing written); everything else is a data problem.
pub(crate) fn map_sqlite(e: rusqlite::Error) -> GearError {
    match e {
        rusqlite::Error::ToSqlConversionFailure(inner) => match inner.downcast::<GearError>() {
            Ok(gear) => *gear,
            Err(other) => GearError::new(ErrorCode::BadFormat, other.to_string()),
        },
        rusqlite::Error::SqliteFailure(err, msg) => {
            let code = match err.code {
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                    ErrorCode::Locked
                }
                _ => ErrorCode::BadFormat,
            };
            GearError::new(code, msg.unwrap_or_else(|| err.to_string()))
        }
        other => GearError::new(ErrorCode::BadFormat, other.to_string()),
    }
}

/// Carry a typed error through rusqlite's row closures; the outer conversion
/// unwraps it again, so the original code and detail survive.
fn as_sqlite_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
}

fn json_to_text(value: &serde_json::Value) -> rusqlite::Result<String> {
    serde_json::to_string(value).map_err(as_sqlite_error)
}

fn json_from_text(text: &str) -> rusqlite::Result<serde_json::Value> {
    serde_json::from_str(text).map_err(as_sqlite_error)
}

fn json_strings_from_text(text: &str) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(text).map_err(as_sqlite_error)
}

fn json_strings_to_text(values: &[String]) -> rusqlite::Result<String> {
    serde_json::to_string(values).map_err(as_sqlite_error)
}

/// FNV-1a 64 bit (not cryptographic: it only detects a changed argument set).
fn fnv1a_64(data: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn fnv1a_hex(data: &[u8]) -> String {
    format!("{:016x}", fnv1a_64(data))
}

/// The hash [`Tx::run_request`] stores for a request's arguments, so a caller
/// can recognise an identical committed request before its transaction (T-19).
pub fn args_hash(args_key: &str) -> String {
    fnv1a_hex(args_key.as_bytes())
}

fn binding_status_str(status: BindingStatus) -> &'static str {
    match status {
        BindingStatus::Pending => "pending",
        BindingStatus::Active => "active",
        BindingStatus::Void => "void",
    }
}

fn binding_status_from_str(s: &str) -> GearResult<BindingStatus> {
    match s {
        "pending" => Ok(BindingStatus::Pending),
        "active" => Ok(BindingStatus::Active),
        "void" => Ok(BindingStatus::Void),
        other => Err(GearError::new(
            ErrorCode::BadFormat,
            format!("unknown binding status: {other}"),
        )),
    }
}

fn seat_select(tail: &str) -> String {
    format!(
        "SELECT id, project, role, name, profile, generation, state, state_since, state_reason, task_id
         FROM seats{tail}"
    )
}

fn message_select(tail: &str) -> String {
    format!(
        "SELECT project, id, thread, task, attempt, sender, sender_gen, recipient, kind, urgent, body,
                reply_to, needs_ack, state, wait_on, wait_until, result, created, resolved, meta
         FROM messages{tail}"
    )
}

fn op_select(tail: &str) -> String {
    format!(
        "SELECT project, id, seat, generation, kind, task, attempt, ref_msg, plan_version, payload,
                state, result, check_budget, created, updated
         FROM operations{tail}"
    )
}

fn task_select(tail: &str) -> String {
    format!(
        "SELECT project, id, rank, state, priority, lane, milestone, required, contract_version,
                deps, followup_of, wait_on, wait_cond, rework_note, estimate_min, issue,
                repo, base, gate, rework_estimate_min, updated
         FROM tasks{tail}"
    )
}

fn attempt_select(tail: &str) -> String {
    format!(
        "SELECT project, task, n, seat, generation, contract_version, adopted_version,
                pending_update, state, candidate, report, safe_point, started, updated,
                session_ref, session_cli, session_action, sent_at
         FROM attempts{tail}"
    )
}

fn round_select(tail: &str) -> String {
    format!(
        "SELECT project, task, attempt, role, seat, generation, cli, model, start_at, end_at,
                sessions, active_s, paused_s, input, output, cache_read, cache_write, reasoning,
                cost_usd, cost_source, counted_at
         FROM rounds{tail}"
    )
}

fn friction_select(tail: &str) -> String {
    format!(
        "SELECT project, id, task, attempt, role, seat, body, required, created, closed_at, outcome
         FROM frictions{tail}"
    )
}

fn decision_select(tail: &str) -> String {
    format!(
        "SELECT project, key, seq, voyage, at, kind, what, why, alt, changes, star, source, line,
                plan_version, created
         FROM decisions{tail}"
    )
}

fn plan_from_row(row: &rusqlite::Row) -> rusqlite::Result<PlanRow> {
    Ok(PlanRow {
        project: row.get(0)?,
        version: row.get(1)?,
        draft: row.get(2)?,
        applied: row.get(3)?,
        by_seat: row.get(4)?,
        by_gen: row.get(5)?,
    })
}

fn task_from_row(row: &rusqlite::Row) -> rusqlite::Result<TaskRow> {
    let state: String = row.get(3)?;
    let deps: String = row.get(9)?;
    Ok(TaskRow {
        project: row.get(0)?,
        id: row.get(1)?,
        rank: row.get(2)?,
        state: state.parse::<TaskState>().map_err(as_sqlite_error)?,
        priority: row.get(4)?,
        lane: row.get(5)?,
        milestone: row.get(6)?,
        required: row.get::<_, i64>(7)? != 0,
        contract_version: row.get(8)?,
        deps: json_strings_from_text(&deps)?,
        followup_of: row.get(10)?,
        wait_on: row.get(11)?,
        wait_cond: row.get(12)?,
        rework_note: row.get(13)?,
        estimate_min: row.get(14)?,
        issue: row.get(15)?,
        repo: row.get(16)?,
        base: row.get(17)?,
        gate: row.get(18)?,
        rework_estimate_min: row.get(19)?,
        updated: row.get(20)?,
    })
}

fn contract_from_row(row: &rusqlite::Row) -> rusqlite::Result<ContractRow> {
    let evidence_files: String = row.get(5)?;
    Ok(ContractRow {
        project: row.get(0)?,
        task: row.get(1)?,
        version: row.get(2)?,
        goal: row.get(3)?,
        accept: row.get(4)?,
        evidence_files: json_strings_from_text(&evidence_files)?,
        evidence_due_min: row.get(6)?,
        body: row.get(7)?,
        created: row.get(8)?,
        mechanical: row.get(9)?,
    })
}

fn attempt_from_row(row: &rusqlite::Row) -> rusqlite::Result<AttemptRow> {
    let state: String = row.get(8)?;
    Ok(AttemptRow {
        project: row.get(0)?,
        task: row.get(1)?,
        n: row.get(2)?,
        seat: row.get(3)?,
        generation: row.get(4)?,
        contract_version: row.get(5)?,
        adopted_version: row.get(6)?,
        pending_update: row.get(7)?,
        state: state.parse::<AttemptState>().map_err(as_sqlite_error)?,
        candidate: row.get(9)?,
        report: row.get(10)?,
        safe_point: row.get(11)?,
        started: row.get(12)?,
        updated: row.get(13)?,
        session_ref: row.get(14)?,
        session_cli: row.get(15)?,
        session_action: row.get(16)?,
        sent_at: row.get(17)?,
    })
}

fn round_from_row(row: &rusqlite::Row) -> rusqlite::Result<RoundRow> {
    let sessions: String = row.get(10)?;
    Ok(RoundRow {
        project: row.get(0)?,
        task: row.get(1)?,
        attempt: row.get(2)?,
        role: row.get(3)?,
        seat: row.get(4)?,
        generation: row.get(5)?,
        cli: row.get(6)?,
        model: row.get(7)?,
        start_at: row.get(8)?,
        end_at: row.get(9)?,
        sessions: json_strings_from_text(&sessions)?,
        active_s: row.get(11)?,
        paused_s: row.get(12)?,
        input: row.get(13)?,
        output: row.get(14)?,
        cache_read: row.get(15)?,
        cache_write: row.get(16)?,
        reasoning: row.get(17)?,
        cost_usd: row.get(18)?,
        cost_source: row.get(19)?,
        counted_at: row.get(20)?,
    })
}

fn friction_from_row(row: &rusqlite::Row) -> rusqlite::Result<FrictionRow> {
    Ok(FrictionRow {
        project: row.get(0)?,
        id: row.get(1)?,
        task: row.get(2)?,
        attempt: row.get(3)?,
        role: row.get(4)?,
        seat: row.get(5)?,
        body: row.get(6)?,
        required: row.get::<_, i64>(7)? != 0,
        created: row.get(8)?,
        closed_at: row.get(9)?,
        outcome: row.get(10)?,
    })
}

fn decision_from_row(row: &rusqlite::Row) -> rusqlite::Result<DecisionRow> {
    Ok(DecisionRow {
        project: row.get(0)?,
        key: row.get(1)?,
        seq: row.get(2)?,
        voyage: row.get(3)?,
        at: row.get(4)?,
        kind: row.get(5)?,
        what: row.get(6)?,
        why: row.get(7)?,
        alt: row.get(8)?,
        changes: row.get(9)?,
        star: row.get::<_, i64>(10)? != 0,
        source: row.get(11)?,
        line: row.get(12)?,
        plan_version: row.get(13)?,
        created: row.get(14)?,
    })
}

fn pause_from_row(row: &rusqlite::Row) -> rusqlite::Result<PauseRow> {
    Ok(PauseRow {
        project: row.get(0)?,
        seat: row.get(1)?,
        kind: row.get(2)?,
        reason: row.get(3)?,
        start_at: row.get(4)?,
        end_at: row.get(5)?,
    })
}

fn voyage_from_row(row: &rusqlite::Row) -> rusqlite::Result<VoyageRow> {
    Ok(VoyageRow {
        project: row.get(0)?,
        milestone: row.get(1)?,
        title: row.get(2)?,
        done: row.get(3)?,
        bar: row.get(4)?,
        eta: row.get(5)?,
        departure: row.get(6)?,
        departure_tasks: row.get(7)?,
        departure_estimate_min: row.get(9)?,
        updated: row.get(8)?,
    })
}

fn verdict_from_row(row: &rusqlite::Row) -> rusqlite::Result<VerdictRow> {
    Ok(VerdictRow {
        project: row.get(0)?,
        task: row.get(1)?,
        attempt: row.get(2)?,
        contract_version: row.get(3)?,
        candidate: row.get(4)?,
        passed: row.get::<_, i64>(5)? != 0,
        evidence: row.get(6)?,
        by_seat: row.get(7)?,
        created: row.get(8)?,
    })
}

fn review_verdict_from_row(row: &rusqlite::Row) -> rusqlite::Result<ReviewVerdictRow> {
    Ok(ReviewVerdictRow {
        project: row.get(0)?,
        task: row.get(1)?,
        attempt: row.get(2)?,
        passed: row.get::<_, i64>(3)? != 0,
        findings: row.get(4)?,
        by_seat: row.get(5)?,
        created: row.get(6)?,
    })
}

fn project_from_row(row: &rusqlite::Row) -> rusqlite::Result<ProjectRow> {
    Ok(ProjectRow {
        id: row.get(0)?,
        herdr_session: row.get(1)?,
        config_toml: row.get(2)?,
        config_version: row.get(3)?,
        quota_lanes: row.get(4)?,
        quota_temps: row.get(5)?,
        quota_includes_captain: row.get::<_, i64>(6)? != 0,
        plan_version: row.get(7)?,
        created: row.get(8)?,
    })
}

fn seat_from_row(row: &rusqlite::Row) -> rusqlite::Result<SeatRow> {
    let role: String = row.get(2)?;
    let state: String = row.get(6)?;
    Ok(SeatRow {
        id: row.get(0)?,
        project: row.get(1)?,
        role: role.parse::<SeatRole>().map_err(as_sqlite_error)?,
        name: row.get(3)?,
        profile: row.get(4)?,
        generation: row.get(5)?,
        state: state.parse::<SeatState>().map_err(as_sqlite_error)?,
        state_since: row.get(7)?,
        state_reason: row.get(8)?,
        task_id: row.get(9)?,
    })
}

fn binding_from_row(row: &rusqlite::Row) -> rusqlite::Result<BindingRow> {
    let status: String = row.get(6)?;
    Ok(BindingRow {
        seat: row.get(0)?,
        generation: row.get(1)?,
        session: row.get(2)?,
        pane_id: row.get(3)?,
        kind: row.get(4)?,
        session_ref: row.get(5)?,
        status: binding_status_from_str(&status).map_err(as_sqlite_error)?,
        handover_id: row.get(7)?,
        created: row.get(8)?,
    })
}

fn message_from_row(row: &rusqlite::Row) -> rusqlite::Result<MessageRow> {
    let kind: String = row.get(8)?;
    let state: String = row.get(13)?;
    Ok(MessageRow {
        project: row.get(0)?,
        id: row.get(1)?,
        thread: row.get(2)?,
        task: row.get(3)?,
        attempt: row.get(4)?,
        sender: row.get(5)?,
        sender_gen: row.get(6)?,
        recipient: row.get(7)?,
        kind: kind.parse::<MsgKind>().map_err(as_sqlite_error)?,
        urgent: row.get::<_, i64>(9)? != 0,
        body: row.get(10)?,
        reply_to: row.get(11)?,
        needs_ack: row.get::<_, i64>(12)? != 0,
        state: state.parse::<MsgState>().map_err(as_sqlite_error)?,
        wait_on: row.get(14)?,
        wait_until: row.get(15)?,
        result: row.get(16)?,
        created: row.get(17)?,
        resolved: row.get(18)?,
        meta: row.get(19)?,
    })
}

fn op_from_row(row: &rusqlite::Row) -> rusqlite::Result<OpRow> {
    let kind: String = row.get(4)?;
    let state: String = row.get(10)?;
    let payload: String = row.get(9)?;
    Ok(OpRow {
        project: row.get(0)?,
        id: row.get(1)?,
        seat: row.get(2)?,
        generation: row.get(3)?,
        kind: kind.parse::<OpKind>().map_err(as_sqlite_error)?,
        task: row.get(5)?,
        attempt: row.get(6)?,
        ref_msg: row.get(7)?,
        plan_version: row.get(8)?,
        payload: json_from_text(&payload)?,
        state: state.parse::<OpState>().map_err(as_sqlite_error)?,
        result: row.get(11)?,
        check_budget: row.get(12)?,
        created: row.get(13)?,
        updated: row.get(14)?,
    })
}

fn view_from_row(row: &rusqlite::Row) -> rusqlite::Result<ViewRecord> {
    let shown_msgs: String = row.get(4)?;
    let shown_ops: String = row.get(5)?;
    Ok(ViewRecord {
        project: row.get(0)?,
        seat: row.get(1)?,
        generation: row.get(2)?,
        ts: row.get(3)?,
        shown_msgs: json_strings_from_text(&shown_msgs)?,
        shown_ops: json_strings_from_text(&shown_ops)?,
        shown_task: row.get(6)?,
        shown_attempt: row.get(7)?,
        shown_contract: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Both connections see the old schema before the writer commits. The
    // opener must choose its work from the version under BEGIN IMMEDIATE.
    fn competing_schema_writer(fresh: bool, committed_version: i64) {
        use std::sync::mpsc;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gear.db");
        let mut opener = Store {
            conn: Connection::open(&path).unwrap(),
        };
        opener.prepare_connection().unwrap();
        if !fresh {
            opener
                .conn
                .execute_batch(
                    "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta VALUES ('schema_version', '1');",
                )
                .unwrap();
        }
        let observed = Store::stored_version(&opener.conn).unwrap();
        assert_eq!(
            observed,
            if fresh {
                Version::Fresh
            } else {
                Version::Stamped(1)
            }
        );

        let mut writer = Connection::open(&path).unwrap();
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE slots (marker TEXT);
             INSERT INTO slots VALUES ('preserve me');",
        )
        .unwrap();
        set_version(&tx, committed_version).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = opener.apply_schema();
            done_tx.send(result.map_err(|e| e.code)).unwrap();
        });
        started_rx.recv().unwrap();
        // The write lock is held until the competing version is committed.
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        tx.commit().unwrap();
        let result = done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        worker.join().unwrap();
        if committed_version > SCHEMA_VERSION {
            assert_eq!(result, Err(ErrorCode::SchemaTooNew));
        } else {
            result.unwrap();
        }
        let version: String = writer
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, committed_version.to_string());
        let marker: String = writer
            .query_row("SELECT marker FROM slots", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            marker, "preserve me",
            "a completed migration must not run twice"
        );
        let tables: i64 = writer
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2, "the waiting opener must not change the schema");
    }

    #[test]
    fn migration_rechecks_a_newer_version_after_waiting_for_the_writer() {
        competing_schema_writer(false, SCHEMA_VERSION + 1);
    }

    #[test]
    fn migration_skips_a_step_already_committed_by_another_opener() {
        competing_schema_writer(false, SCHEMA_VERSION);
    }

    #[test]
    fn creation_rechecks_a_newer_version_after_waiting_for_the_writer() {
        competing_schema_writer(true, SCHEMA_VERSION + 1);
    }

    #[test]
    fn creation_skips_schema_already_committed_by_another_opener() {
        competing_schema_writer(true, SCHEMA_VERSION);
    }

    #[test]
    fn fnv1a_matches_the_reference_vectors() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
        assert_eq!(fnv1a_hex(b"a"), "af63dc4c8601ec8c");
    }

    #[test]
    fn binding_status_roundtrips() {
        for status in [
            BindingStatus::Pending,
            BindingStatus::Active,
            BindingStatus::Void,
        ] {
            let text = binding_status_str(status);
            assert_eq!(binding_status_from_str(text).unwrap(), status);
        }
        assert!(binding_status_from_str("bogus").is_err());
    }

    #[test]
    fn a_binding_session_ref_is_written_and_read_back() {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", 0)?;
            tx.insert_seat(&SeatRow {
                id: "p:heat".into(),
                project: "p".into(),
                role: SeatRole::Lane,
                name: "heat".into(),
                profile: "opencode".into(),
                generation: 1,
                state: SeatState::Starting,
                state_since: 0,
                state_reason: None,
                task_id: None,
            })?;
            tx.insert_binding(&BindingRow {
                seat: "p:heat".into(),
                generation: 1,
                session: "sess".into(),
                pane_id: "w1:p1".into(),
                kind: "opencode".into(),
                session_ref: None,
                status: BindingStatus::Pending,
                handover_id: None,
                created: 0,
            })?;
            Ok(())
        })
        .unwrap();

        s.write(|tx| tx.set_binding_session_ref("p:heat", 1, "ses_x"))
            .unwrap();
        let binding = s.read(|tx| tx.get_binding("p:heat", 1)).unwrap().unwrap();
        assert_eq!(binding.session_ref.as_deref(), Some("ses_x"));

        // A binding that does not exist is NoTarget, not a silent write.
        let err = s
            .write(|tx| tx.set_binding_session_ref("p:heat", 2, "ses_y"))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NoTarget);
    }

    /// T-143 (design §3.6): a reviewer in `Standby` has no job — its task
    /// binding is cleared and the seat keeps everything else.
    #[test]
    fn a_seats_job_binding_is_cleared_and_read_back() {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", 0)?;
            tx.insert_seat(&SeatRow {
                id: "p:review-1".into(),
                project: "p".into(),
                role: SeatRole::Temp,
                name: "review-1".into(),
                profile: "codex".into(),
                generation: 1,
                state: SeatState::Standby,
                state_since: 0,
                state_reason: Some("review done".into()),
                task_id: Some("T-1".into()),
            })?;
            Ok(())
        })
        .unwrap();

        let seat = s.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.task_id.as_deref(), Some("T-1"), "the seat has a job");

        s.write(|tx| tx.clear_seat_task("p:review-1")).unwrap();

        let seat = s.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.task_id, None, "the job binding ended");
        assert_eq!(seat.state, SeatState::Standby, "the seat is still a standby reviewer");
        assert_eq!(seat.generation, 1, "nothing else moved");

        // A seat that does not exist is NoTarget, not a silent write.
        let err = s.write(|tx| tx.clear_seat_task("p:ghost")).unwrap_err();
        assert_eq!(err.code, ErrorCode::NoTarget);
    }

    #[test]
    fn json_helpers_roundtrip() {
        let value = serde_json::json!({"msgs": ["M-1"], "n": 2});
        let text = json_to_text(&value).unwrap();
        assert_eq!(json_from_text(&text).unwrap(), value);

        let ids = vec!["M-1".to_string(), "M-2".to_string()];
        let text = json_strings_to_text(&ids).unwrap();
        assert_eq!(json_strings_from_text(&text).unwrap(), ids);
    }

    #[test]
    fn a_broken_row_is_a_bad_format_error() {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            tx.tx
                .execute("INSERT INTO seats (id, project, role, name, profile, generation, state, state_since)
                          VALUES ('p:x', 'p', 'wizard', 'x', 'codex', 0, 'active', 0)", [])
                .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();
        let err = s.read(|tx| tx.get_seat("p:x")).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat);
    }
}
