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
}

/// Map a board-herdr error from a side-effecting call to an [`Outcome`].
/// Protocol errors with a code that proves no execution → Refused; everything
/// else (io, deadline, disconnect, decode, unknown code) → Uncertain.
pub fn outcome_from_error<T>(err: &board_herdr::HerdrError) -> Outcome<T> {
    todo!("T-herdr")
}
