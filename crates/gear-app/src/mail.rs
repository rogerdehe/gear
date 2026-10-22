//! Messages: send, reply, inbox batch selection (design/07).

use crate::identity::Fingerprint;
use crate::Env;
use gear_model::{GearResult, MsgKind, Ts};
use gear_store::{MessageRow, Store, Tx};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendArgs {
    pub kind: MsgKind,
    pub body: String,
    /// Required for Result/Blocked (design/03): exact task, attempt, contract.
    pub task: Option<String>,
    pub attempt: Option<i64>,
    pub contract: Option<i64>,
    /// Upstream task to ask (must be a registered dependency) — slice 2.
    pub about: Option<String>,
    /// Captain only: seat name to instruct.
    pub to: Option<String>,
    pub reply_to: Option<String>,
    pub urgent: bool,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SendResult {
    pub msg_id: String,
    pub thread: String,
    /// Seat id, e.g. `p:captain`.
    pub recipient: String,
    /// `true` when the stored result was replayed for a repeated request_id.
    pub replayed: bool,
}

/// `gear send`. One transaction via `run_request`.
///
/// Slice 1 scope:
/// - sender must be an Active/Standby/Retiring lane/temp, or the captain;
/// - default recipient = project captain; `--to` only for the captain;
///   `--reply-to M-x` → the sender of M-x;
/// - Progress → Info; Question/Blocked/Result/Instruction → Open;
///   Reply-kind is produced by `reply()`, not by `send`;
/// - thread: reply_to inherits; otherwise new thread `<task or seat>:<M-id>`;
/// - Ack/Result/Blocked side effects (tasks, handover) arrive in slice 2 and
///   must return BadFormat until then rather than silently storing.
pub fn send(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    args: &SendArgs,
    now: Ts,
) -> GearResult<SendResult> {
    todo!("slice1")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadView {
    pub thread: String,
    pub urgent: bool,
    /// Unhandled messages in this thread (max 3 shown), oldest first.
    pub open: Vec<MessageRow>,
    /// How many more unhandled messages exist beyond `open`.
    pub more: usize,
    /// Latest reply/decision in the thread, for context.
    pub last_reply: Option<MessageRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchLimits {
    pub max_threads: usize,
    pub per_sender: usize,
}

impl Default for BatchLimits {
    fn default() -> Self {
        BatchLimits { max_threads: 6, per_sender: 2 }
    }
}

/// Pick the next batch for `recipient` (design/07):
/// 1. candidates = Open + due Waiting;
/// 2. if any non-urgent thread exists, reserve one slot for the oldest one;
/// 3. fill the rest: urgent threads first, then oldest first; at most
///    `per_sender` threads per sender (the reserved one counts);
/// Returns threads in display order. Reading never changes state.
pub fn select_batch(
    tx: &Tx<'_>,
    project: &str,
    recipient: &str,
    now: Ts,
    limits: BatchLimits,
) -> GearResult<Vec<ThreadView>> {
    todo!("slice1")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyMode {
    Resolve,
    Wait,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyArgs {
    pub ids: Vec<String>,
    pub text: String,
    pub mode: ReplyMode,
    pub wait_on: Option<String>,
    pub wait_until: Option<Ts>,
    /// Only adjusts urgency; with no explicit mode the CLI passes Note.
    pub downgrade: bool,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReplyResult {
    /// (message id, new state as string)
    pub handled: Vec<(String, String)>,
    /// Reply messages created, one per original sender.
    pub replies: Vec<String>,
    pub replayed: bool,
}

/// `gear reply` (captain only). Each id must be in this project, addressed to
/// the captain, Open or Waiting. Resolve → Resolved; Wait → Waiting with
/// wait_on/wait_until; Note → state unchanged. Writes one Reply (state Info)
/// into each original sender's inbox, same thread. Never changes task state.
pub fn reply(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    args: &ReplyArgs,
    now: Ts,
) -> GearResult<ReplyResult> {
    todo!("slice1")
}
