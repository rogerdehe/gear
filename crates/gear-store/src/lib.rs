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
use std::time::Duration;

pub const SCHEMA_SQL: &str = include_str!("schema.sql");
pub const SCHEMA_VERSION: i64 = 1;
/// SQLite busy timeout for every connection.
pub const BUSY_TIMEOUT_MS: u64 = 5_000;

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
    /// foreign keys on, schema applied. Refuses a database whose
    /// `meta.schema_version` is newer than [`SCHEMA_VERSION`] (BadFormat).
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
        store.prepare_connection()?;
        store.apply_schema()?;
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
    pub fn write<T>(&mut self, f: impl FnOnce(&Tx<'_>) -> GearResult<T>) -> GearResult<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        let tx = Tx { tx };
        match f(&tx) {
            Ok(value) => {
                tx.tx.commit().map_err(map_sqlite)?;
                Ok(value)
            }
            Err(e) => {
                drop(tx); // rolls back, nothing written
                Err(e)
            }
        }
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

    /// Apply the schema and check the stored schema version. The schema is
    /// created inside one immediate transaction so that concurrent openers
    /// cannot collide halfway through.
    fn apply_schema(&mut self) -> GearResult<()> {
        {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite)?;
            tx.execute_batch(SCHEMA_SQL).map_err(map_sqlite)?;
            tx.commit().map_err(map_sqlite)?;
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        let stored: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        match stored {
            None => {
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                    params![SCHEMA_VERSION.to_string()],
                )
                .map_err(map_sqlite)?;
            }
            Some(raw) => {
                let version: i64 = raw.trim().parse().map_err(|_| {
                    GearError::new(
                        ErrorCode::BadFormat,
                        format!("meta.schema_version is not a number: {raw}"),
                    )
                })?;
                if version > SCHEMA_VERSION {
                    return Err(GearError::new(
                        ErrorCode::BadFormat,
                        format!(
                            "database schema version {version} is newer than the supported {SCHEMA_VERSION}"
                        ),
                    ));
                }
            }
        }
        tx.commit().map_err(map_sqlite)?;
        Ok(())
    }
}

// ---------------------------------------------------------------- tx

impl<'s> Tx<'s> {
    // ---- infrastructure

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
                .prepare("SELECT args_hash, result FROM requests WHERE id=?1")
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
            .prepare("SELECT id FROM projects ORDER BY id")
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(map_sqlite)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(map_sqlite)
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
                   (id, project, role, name, profile, generation, state, state_since, state_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
            .prepare(&seat_select(" WHERE project=?1 ORDER BY id"))
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

    // ---- messages

    /// Allocates `M-<n>` via `next_id` and inserts. Returns the stored row.
    pub fn insert_message(&self, msg: &NewMessage) -> GearResult<MessageRow> {
        let id = self.next_id(&msg.project, 'M')?;
        self.tx
            .execute(
                "INSERT INTO messages
                   (project, id, thread, task, attempt, sender, sender_gen, recipient, kind, urgent,
                    body, reply_to, needs_ack, state, wait_on, wait_until, result, created, resolved)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, NULL, NULL, NULL, ?15, NULL)",
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
            .prepare(&message_select(
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
            .prepare(&message_select(
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

    pub fn thread_messages(&self, project: &str, thread: &str) -> GearResult<Vec<MessageRow>> {
        let mut stmt = self
            .tx
            .prepare(&message_select(
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
            .prepare(
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
            .prepare(&op_select(
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
            .prepare(&op_select(" WHERE project=?1 AND state=?2 ORDER BY created, id"))
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

// Slice-2 skeleton: bodies are T-33/T-34; the allow goes when they land.
#[allow(unused_variables)]
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
                    deps, followup_of, wait_on, wait_cond, rework_note, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
            .prepare(&task_select(" WHERE project=?1 ORDER BY rank, id"))
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
                     rework_note=?14, updated=?15
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
                   (project, task, version, goal, accept, evidence_files, evidence_due_min, body, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
                ],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    pub fn get_contract(&self, project: &str, task: &str, version: i64) -> GearResult<Option<ContractRow>> {
        let row: Option<ContractRow> = self
            .tx
            .query_row(
                "SELECT project, task, version, goal, accept, evidence_files, evidence_due_min, body, created
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
                    pending_update, state, candidate, report, safe_point, started, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
            .prepare(&attempt_select(" WHERE project=?1 AND task=?2 ORDER BY n"))
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
            .prepare(&attempt_select(
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
    pub fn open_attempts_of_seat(&self, seat: &str) -> GearResult<Vec<AttemptRow>> {
        let mut stmt = self
            .tx
            .prepare(&attempt_select(
                " WHERE seat=?1
                   AND state IN ('delivered', 'started', 'stopped', 'unknown', 'submitted')
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

    /// Replace every column of an existing attempt (raw write). Missing → NoTarget.
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
            .prepare(
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
fn map_sqlite(e: rusqlite::Error) -> GearError {
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
        "SELECT id, project, role, name, profile, generation, state, state_since, state_reason
         FROM seats{tail}"
    )
}

fn message_select(tail: &str) -> String {
    format!(
        "SELECT project, id, thread, task, attempt, sender, sender_gen, recipient, kind, urgent, body,
                reply_to, needs_ack, state, wait_on, wait_until, result, created, resolved
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
                deps, followup_of, wait_on, wait_cond, rework_note, updated
         FROM tasks{tail}"
    )
}

fn attempt_select(tail: &str) -> String {
    format!(
        "SELECT project, task, n, seat, generation, contract_version, adopted_version,
                pending_update, state, candidate, report, safe_point, started, updated
         FROM attempts{tail}"
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
        updated: row.get(14)?,
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
