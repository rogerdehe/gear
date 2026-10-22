//! GEAR SQLite store (design/02).
//!
//! Responsibilities: open the single per-host database, apply the schema,
//! run short `BEGIN IMMEDIATE` transactions, allocate ids, replay requests,
//! and read/write rows. **No business rules live here**: callers (gear-app,
//! gear-engine) decide what to write and check transitions with gear-model.
//!
//! Every method on [`Tx`] runs inside the caller's transaction. Never call
//! Herdr or git while holding a [`Tx`].

#![allow(unused_variables, dead_code)] // skeleton: bodies are `todo!()` until implemented

use gear_model::{
    GearResult, MsgKind, MsgState, OpKind, OpState, SeatRole, SeatState, Ts,
};
use std::path::Path;

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
        todo!("T-store: open + schema + version check")
    }

    /// In-memory database for tests (schema applied).
    pub fn open_in_memory() -> GearResult<Store> {
        todo!("T-store")
    }

    /// Run `f` inside `BEGIN IMMEDIATE`. Commit on `Ok`, roll back on `Err`.
    /// Lock timeout maps to `ErrorCode::Locked` with nothing written.
    pub fn write<T>(&mut self, f: impl FnOnce(&Tx<'_>) -> GearResult<T>) -> GearResult<T> {
        todo!("T-store")
    }

    /// Run `f` in a deferred (read) transaction for a consistent snapshot.
    pub fn read<T>(&mut self, f: impl FnOnce(&Tx<'_>) -> GearResult<T>) -> GearResult<T> {
        todo!("T-store")
    }
}

// ---------------------------------------------------------------- tx

impl<'s> Tx<'s> {
    // ---- infrastructure

    /// Next id for `kind` in `project`: `T-1`, `T-2`, …; kinds: T M R V O. Never reused.
    pub fn next_id(&self, project: &str, kind: char) -> GearResult<String> {
        todo!("T-store")
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
        todo!("T-store")
    }

    pub fn get_meta(&self, key: &str) -> GearResult<Option<String>> {
        todo!("T-store")
    }
    pub fn set_meta(&self, key: &str, value: &str) -> GearResult<()> {
        todo!("T-store")
    }
    pub fn audit(&self, now: Ts, project: Option<&str>, actor: &str, action: &str, detail: &str) -> GearResult<()> {
        todo!("T-store")
    }

    // ---- projects

    pub fn insert_project(&self, row: &ProjectRow) -> GearResult<()> {
        todo!("T-store")
    }
    pub fn get_project(&self, id: &str) -> GearResult<Option<ProjectRow>> {
        todo!("T-store")
    }

    // ---- seats & bindings

    pub fn insert_seat(&self, row: &SeatRow) -> GearResult<()> {
        todo!("T-store")
    }
    pub fn get_seat(&self, seat_id: &str) -> GearResult<Option<SeatRow>> {
        todo!("T-store")
    }
    pub fn seats_of_project(&self, project: &str) -> GearResult<Vec<SeatRow>> {
        todo!("T-store")
    }
    /// The seat with role Captain in `project`, if any.
    pub fn captain_of(&self, project: &str) -> GearResult<Option<SeatRow>> {
        todo!("T-store")
    }
    /// Raw state write. Caller must have checked `SeatState::can_transition`.
    pub fn set_seat_state(&self, seat_id: &str, state: SeatState, reason: Option<&str>, now: Ts) -> GearResult<()> {
        todo!("T-store")
    }
    pub fn set_seat_generation(&self, seat_id: &str, generation: i64) -> GearResult<()> {
        todo!("T-store")
    }
    pub fn insert_binding(&self, row: &BindingRow) -> GearResult<()> {
        todo!("T-store")
    }
    pub fn get_binding(&self, seat_id: &str, generation: i64) -> GearResult<Option<BindingRow>> {
        todo!("T-store")
    }
    pub fn set_binding_status(&self, seat_id: &str, generation: i64, status: BindingStatus) -> GearResult<()> {
        todo!("T-store")
    }

    // ---- messages

    /// Allocates `M-<n>` via `next_id` and inserts. Returns the stored row.
    pub fn insert_message(&self, msg: &NewMessage) -> GearResult<MessageRow> {
        todo!("T-store")
    }
    pub fn get_message(&self, project: &str, id: &str) -> GearResult<Option<MessageRow>> {
        todo!("T-store")
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
        todo!("T-store")
    }
    /// Messages to `recipient` in state Open, or Waiting with `wait_until <= now`.
    /// Ordered by created ascending.
    pub fn actionable_for(&self, project: &str, recipient: &str, now: Ts) -> GearResult<Vec<MessageRow>> {
        todo!("T-store")
    }
    /// Info/Reply/Instruction messages to `recipient` created after `since` (for "unshown" checks).
    pub fn messages_to_since(&self, project: &str, recipient: &str, since: Ts) -> GearResult<Vec<MessageRow>> {
        todo!("T-store")
    }
    pub fn thread_messages(&self, project: &str, thread: &str) -> GearResult<Vec<MessageRow>> {
        todo!("T-store")
    }

    // ---- views

    pub fn insert_view(&self, v: &ViewRecord) -> GearResult<()> {
        todo!("T-store")
    }
    /// Views by `seat` at `generation` with `ts >= since`, oldest first.
    pub fn views_since(&self, seat: &str, generation: i64, since: Ts) -> GearResult<Vec<ViewRecord>> {
        todo!("T-store")
    }

    // ---- operations

    /// Allocates `O-<n>`, inserts in state Intended. Returns the stored row.
    pub fn insert_op(&self, op: &NewOp) -> GearResult<OpRow> {
        todo!("T-store")
    }
    pub fn get_op(&self, project: &str, id: &str) -> GearResult<Option<OpRow>> {
        todo!("T-store")
    }
    /// Raw state write. Caller checks `OpState::can_transition`.
    pub fn set_op_state(&self, project: &str, id: &str, state: OpState, result: Option<&str>, now: Ts) -> GearResult<()> {
        todo!("T-store")
    }
    pub fn bump_op_check_budget(&self, project: &str, id: &str, now: Ts) -> GearResult<i64> {
        todo!("T-store")
    }
    /// Deliver/Doorbell ops of `seat` whose state holds the delivery slot.
    pub fn slot_ops(&self, seat: &str) -> GearResult<Vec<OpRow>> {
        todo!("T-store")
    }
    pub fn ops_in_state(&self, project: &str, state: OpState) -> GearResult<Vec<OpRow>> {
        todo!("T-store")
    }
    /// Most recent op of `kind` for `seat` (any state), for cooldowns.
    pub fn last_op(&self, seat: &str, kind: OpKind) -> GearResult<Option<OpRow>> {
        todo!("T-store")
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
        todo!("T-store")
    }

    /// Insert a project with quota lanes=4 temps=2, captain excluded, config_toml "", version 1.
    pub fn seed_project(tx: &Tx<'_>, project: &str, session: &str, now: Ts) -> GearResult<ProjectRow> {
        todo!("T-store")
    }
}
