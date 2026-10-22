//! GEAR's only door to Herdr (design/04).
//!
//! - [`HerdrRead`]: observation. Usable by `gear` CLI commands (identity
//!   fingerprint) and by the engine. Never has side effects.
//! - [`HerdrAct`]: side effects (prompt, start, close…). **Only the engine**
//!   may hold one.
//! - [`SocketHerdr`]: real implementation over the Herdr socket, built on the
//!   vendored `board-herdr` client (MIT, see third_party/board-herdr/NOTICE).
//! - [`fake::FakeHerdr`]: scriptable in-memory Herdr for tests.
//!
//! Outcome rules (design/04, /08): an action whose effect may have happened
//! (timeout, disconnect, undecodable reply) is `Uncertain`, never `Refused`.
//! Only a reply that proves nothing was executed is `Refused`.

#![allow(unused_variables, dead_code)] // skeleton: SocketHerdr bodies are `todo!()`

pub mod fake;
pub mod profile;

use serde_json::Value;

pub use board_herdr::{AgentInfo, AgentSession, AgentStatus};
pub use profile::{classify_screen, validate_template, Profile, ScreenFlag};

/// Reading Herdr failed. Callers must treat this as "observation failed",
/// never as "agent gone" (design/08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserveError(pub String);

/// Result of a side-effecting call.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome<T> {
    /// Herdr accepted the call. For `prompt` this only means "text submitted to
    /// the terminal" (op → Submitted), not "agent started working".
    Ok(T),
    /// Herdr refused and nothing was executed (e.g. `agent_blocked`, `not_found`).
    Refused { code: String, message: String },
    /// The effect may or may not have happened (timeout, disconnect, bad reply).
    /// Op → Unknown; never retried blindly.
    Uncertain { reason: String },
}

/// A worktree opened in Herdr (design/04, conventions 17 §4): the Herdr
/// workspace that shows it, that workspace's root pane, the checkout path Herdr
/// chose, and the branch checked out there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub workspace_id: String,
    pub pane_id: String,
    pub path: String,
    pub branch: String,
}

pub trait HerdrRead {
    /// All agents of the bound session (one `session.snapshot` call).
    fn agents(&mut self) -> Result<Vec<AgentInfo>, ObserveError>;
    /// One pane's current occupant, `None` if the pane hosts no agent.
    fn agent_get(&mut self, pane_id: &str) -> Result<Option<AgentInfo>, ObserveError>;
    /// Last `lines` lines of the pane (recent-unwrapped), for screen flags.
    fn read_tail(&mut self, pane_id: &str, lines: u32) -> Result<String, ObserveError>;
}

pub trait HerdrAct: HerdrRead {
    /// Submit `text` + Enter to the agent in `pane_id` (no --wait).
    fn prompt(&mut self, pane_id: &str, text: &str) -> Outcome<()>;
    /// Run a shell command in a shell pane (used to export GEAR_CTX before start).
    fn pane_run(&mut self, pane_id: &str, command: &str) -> Outcome<()>;
    /// Start an agent of `kind` in an available shell pane.
    fn agent_start(&mut self, name: &str, kind: &str, pane_id: &str, args: &[String]) -> Outcome<AgentInfo>;
    fn send_keys(&mut self, pane_id: &str, keys: &[String]) -> Outcome<()>;
    fn pane_close(&mut self, pane_id: &str) -> Outcome<()>;
    fn set_title(&mut self, pane_id: &str, title: &str) -> Outcome<()>;
    fn notify(&mut self, title: &str, body: &str) -> Outcome<()>;
    /// Herdr `worktree.create` (never focuses): new branch `branch` cut from
    /// `base` in the repo at `repo`, opened in a new workspace labelled `label`.
    fn worktree_create(&mut self, repo: &str, branch: &str, base: &str, label: &str) -> Outcome<Worktree>;
    /// Herdr `worktree.open` (never focuses): reopen the existing worktree of
    /// `branch` (temp-worker rework / resume).
    fn worktree_open(&mut self, repo: &str, branch: &str, label: &str) -> Outcome<Worktree>;
    /// Herdr `worktree.remove` (never forced): removes the checkout and its
    /// workspace; the branch is kept.
    fn worktree_remove(&mut self, workspace_id: &str) -> Outcome<()>;
}

/// Real Herdr over its unix socket. Holds one blocking client connection.
pub struct SocketHerdr {
    client: board_herdr::HerdrClient,
}

impl SocketHerdr {
    /// Connect to `socket_path` (from the project binding / HERDR_SOCKET_PATH) and
    /// require the supported protocol (22). Failure is an ObserveError.
    pub fn connect(socket_path: &std::path::Path) -> Result<SocketHerdr, ObserveError> {
        todo!("T-herdr")
    }
}

impl HerdrRead for SocketHerdr {
    fn agents(&mut self) -> Result<Vec<AgentInfo>, ObserveError> {
        todo!("T-herdr")
    }
    fn agent_get(&mut self, pane_id: &str) -> Result<Option<AgentInfo>, ObserveError> {
        todo!("T-herdr")
    }
    fn read_tail(&mut self, pane_id: &str, lines: u32) -> Result<String, ObserveError> {
        todo!("T-herdr")
    }
}

impl HerdrAct for SocketHerdr {
    fn prompt(&mut self, pane_id: &str, text: &str) -> Outcome<()> {
        todo!("T-herdr")
    }
    fn pane_run(&mut self, pane_id: &str, command: &str) -> Outcome<()> {
        todo!("T-herdr")
    }
    fn agent_start(&mut self, name: &str, kind: &str, pane_id: &str, args: &[String]) -> Outcome<AgentInfo> {
        todo!("T-herdr")
    }
    fn send_keys(&mut self, pane_id: &str, keys: &[String]) -> Outcome<()> {
        todo!("T-herdr")
    }
    fn pane_close(&mut self, pane_id: &str) -> Outcome<()> {
        todo!("T-herdr")
    }
    fn set_title(&mut self, pane_id: &str, title: &str) -> Outcome<()> {
        todo!("T-herdr")
    }
    fn notify(&mut self, title: &str, body: &str) -> Outcome<()> {
        todo!("T-herdr")
    }
    fn worktree_create(&mut self, repo: &str, branch: &str, base: &str, label: &str) -> Outcome<Worktree> {
        todo!("T-herdr")
    }
    fn worktree_open(&mut self, repo: &str, branch: &str, label: &str) -> Outcome<Worktree> {
        todo!("T-herdr")
    }
    fn worktree_remove(&mut self, workspace_id: &str) -> Outcome<()> {
        todo!("T-herdr")
    }
}

/// Map a board-herdr error from a side-effecting call to an [`Outcome`].
/// Protocol errors with a code that proves no execution → Refused; everything
/// else (io, deadline, disconnect, decode, unknown code) → Uncertain.
pub fn outcome_from_error<T>(err: &board_herdr::HerdrError) -> Outcome<T> {
    match err {
        board_herdr::HerdrError::Protocol { code, message }
            if REFUSED_CODES.contains(&code.as_str()) =>
        {
            Outcome::Refused { code: code.clone(), message: message.clone() }
        }
        board_herdr::HerdrError::Protocol { code, message } => {
            Outcome::Uncertain { reason: format!("herdr error [{code}]: {message}") }
        }
        other => Outcome::Uncertain { reason: other.to_string() },
    }
}

/// Protocol error codes that prove nothing was executed: the call was
/// rejected before it ran. Every other code (and every transport error) is
/// Uncertain — the call may have taken effect.
const REFUSED_CODES: &[&str] = &[
    "agent_blocked",
    "not_found",
    "invalid_request",
    "invalid_params",
    "agent_not_found",
    "pane_not_found",
    "pane_not_available",
];

/// Protocol error codes that answer "there is no agent in that pane": a
/// negative answer to a question, not a failed observation.
const MISSING_CODES: &[&str] = &["not_found", "agent_not_found", "pane_not_found"];

/// Startup timeout for `agent.start` (design/04: 60 seconds).
const AGENT_START_TIMEOUT_MS: u64 = 60_000;

/// The key that submits a line in a terminal pane.
const ENTER_KEY: &str = "enter";

/// The source GEAR reports pane metadata under.
const METADATA_SOURCE: &str = "gear";

/// Build a [`Worktree`] from a `worktree.create` / `worktree.open` reply.
/// A reply missing any of the four fields → Uncertain: the worktree may exist
/// even though GEAR cannot name it.
fn worktree_from_reply(reply: &Value) -> Outcome<Worktree> {
    let workspace_id = reply
        .pointer("/workspace/workspace_id")
        .and_then(Value::as_str);
    let pane_id = reply.pointer("/root_pane/pane_id").and_then(Value::as_str);
    let path = reply.pointer("/worktree/path").and_then(Value::as_str);
    let branch = reply.pointer("/worktree/branch").and_then(Value::as_str);
    match (workspace_id, pane_id, path, branch) {
        (Some(workspace_id), Some(pane_id), Some(path), Some(branch)) => Outcome::Ok(Worktree {
            workspace_id: workspace_id.to_string(),
            pane_id: pane_id.to_string(),
            path: path.to_string(),
            branch: branch.to_string(),
        }),
        _ => Outcome::Uncertain {
            reason: format!("worktree reply is incomplete: {reply}"),
        },
    }
}
