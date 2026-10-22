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

pub mod fake;
pub mod profile;

use serde_json::{json, Value};

pub use board_herdr::{AgentInfo, AgentSession, AgentStatus};
pub use profile::{builtin, classify_screen, validate_template, Profile, ScreenFlag};

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
        let mut client = board_herdr::HerdrClient::connect(socket_path)
            .map_err(|e| ObserveError(format!("herdr connect failed: {e}")))?;
        client
            .require_supported_protocol()
            .map_err(|e| ObserveError(format!("herdr protocol check failed: {e}")))?;
        Ok(SocketHerdr { client })
    }
}

impl HerdrRead for SocketHerdr {
    fn agents(&mut self) -> Result<Vec<AgentInfo>, ObserveError> {
        let snapshot = self
            .client
            .session_snapshot()
            .map_err(|e| ObserveError(format!("session.snapshot failed: {e}")))?;
        Ok(snapshot.agents)
    }

    fn agent_get(&mut self, pane_id: &str) -> Result<Option<AgentInfo>, ObserveError> {
        match self.client.agent_get(pane_id) {
            Ok(info) => {
                // A pane that hosts no agent is a negative answer, not a failure.
                if info.agent.is_none() {
                    Ok(None)
                } else {
                    Ok(Some(info))
                }
            }
            Err(board_herdr::HerdrError::Protocol { code, .. }) if is_missing(&code) => Ok(None),
            Err(e) => Err(ObserveError(format!("agent.get failed: {e}"))),
        }
    }

    fn read_tail(&mut self, pane_id: &str, lines: u32) -> Result<String, ObserveError> {
        let read = self
            .client
            .pane_read(pane_id, board_herdr::ReadSource::RecentUnwrapped, Some(lines))
            .map_err(|e| ObserveError(format!("pane.read failed: {e}")))?;
        Ok(read.text)
    }
}

impl HerdrAct for SocketHerdr {
    fn prompt(&mut self, pane_id: &str, text: &str) -> Outcome<()> {
        let params = board_herdr::AgentPromptParams {
            target: pane_id.to_string(),
            text: text.to_string(),
            wait: None, // no --wait: "submitted" is confirmed by the next observation round
        };
        match self.client.agent_prompt(&params) {
            Ok(_) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn pane_run(&mut self, pane_id: &str, command: &str) -> Outcome<()> {
        // `pane.send_input` is the atomic "text + Enter" submission Herdr's
        // own `pane run` uses (the socket protocol has no other one): the
        // command is queued and submitted as one unit, so a rejected line
        // never leaves a half-typed command in the pane.
        let params = json!({
            "pane_id": pane_id,
            "text": command,
            "keys": [ENTER_KEY],
        });
        match self.client.call("pane.send_input", params) {
            Ok(_) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn agent_start(&mut self, name: &str, kind: &str, pane_id: &str, args: &[String]) -> Outcome<AgentInfo> {
        let params = board_herdr::AgentStartParams {
            name: name.to_string(),
            kind: kind.to_string(),
            pane_id: pane_id.to_string(),
            args: args.to_vec(),
            timeout_ms: Some(AGENT_START_TIMEOUT_MS),
        };
        match self.client.agent_start(&params) {
            Ok(started) => {
                let mut info = started.agent;
                if info.agent.is_none() {
                    // Herdr started the agent but did not name its kind: refresh
                    // the pane's own info instead of guessing.
                    if let Ok(fresh) = self.client.agent_get(pane_id) {
                        info = fresh;
                    }
                    // A failed refresh keeps the start reply's info: the start
                    // itself was submitted, so the outcome stays Ok.
                }
                Outcome::Ok(info)
            }
            Err(e) => outcome_from_error(&e),
        }
    }

    fn send_keys(&mut self, pane_id: &str, keys: &[String]) -> Outcome<()> {
        match self.client.pane_send_keys(pane_id, keys) {
            Ok(()) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn pane_close(&mut self, pane_id: &str) -> Outcome<()> {
        match self.client.pane_close(pane_id) {
            Ok(()) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn set_title(&mut self, pane_id: &str, title: &str) -> Outcome<()> {
        // Herdr has no "pane.title" call: the pane title is metadata reported
        // by the pane's owner. GEAR reports it under the source "gear".
        let params = json!({
            "pane_id": pane_id,
            "source": METADATA_SOURCE,
            "title": title,
        });
        match self.client.call("pane.report_metadata", params) {
            Ok(_) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn notify(&mut self, title: &str, body: &str) -> Outcome<()> {
        match self.client.notification_show(title, Some(body), board_herdr::NotificationSound::None) {
            Ok(_) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn worktree_create(&mut self, repo: &str, branch: &str, base: &str, label: &str) -> Outcome<Worktree> {
        // `focus: false`: GEAR never steals the user's focus.
        let params = json!({
            "cwd": repo,
            "branch": branch,
            "base": base,
            "label": label,
            "focus": false,
        });
        match self.client.call("worktree.create", params) {
            Ok(result) => worktree_from_reply(&result),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn worktree_open(&mut self, repo: &str, branch: &str, label: &str) -> Outcome<Worktree> {
        let params = json!({
            "cwd": repo,
            "branch": branch,
            "label": label,
            "focus": false,
        });
        match self.client.call("worktree.open", params) {
            Ok(result) => worktree_from_reply(&result),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn worktree_remove(&mut self, workspace_id: &str) -> Outcome<()> {
        // `force: false` always: a dirty worktree is never discarded silently.
        let params = json!({
            "workspace_id": workspace_id,
            "force": false,
        });
        match self.client.call("worktree.remove", params) {
            Ok(_) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }
}

/// Map a board-herdr error from a side-effecting call to an [`Outcome`].
/// Protocol errors with a code that proves no execution → Refused; everything
/// else (io, deadline, disconnect, decode, unknown code) → Uncertain.
pub fn outcome_from_error<T>(err: &board_herdr::HerdrError) -> Outcome<T> {
    match err {
        board_herdr::HerdrError::Protocol { code, message } if is_refused(code) => {
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
    // Pre-execution refusals of `agent.start` (captain decision, review R-3):
    // the pane was busy, the agent kind is unavailable or the name is taken —
    // in each case nothing was started.
    "agent_pane_busy",
    "agent_pane_unavailable",
    "agent_name_taken",
    // Captain decision (review R-8): starting an unsupported kind and removing
    // an unknown workspace are refused before anything runs.
    "unsupported_agent_kind",
    "workspace_not_found",
];

/// Protocol error codes that answer "there is no agent in that pane": an
/// unambiguous negative answer to a question, not a failed observation. The
/// generic `not_found` is **not** one of them: an unexplained "not found"
/// must stay an observation failure, never a destructive "agent gone"
/// judgement (design/08).
const MISSING_CODES: &[&str] = &["pane_not_found", "agent_not_found"];

/// Whether this protocol error code proves that nothing was executed.
fn is_refused(code: &str) -> bool {
    REFUSED_CODES.contains(&code)
}

/// Whether this protocol error code unambiguously means "no agent in that
/// pane".
fn is_missing(code: &str) -> bool {
    MISSING_CODES.contains(&code)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A `worktree.create` reply (schema variant `worktree_created`).
    fn created_reply() -> Value {
        serde_json::json!({
            "type": "worktree_created",
            "workspace": {"workspace_id": "w3", "label": "gear-data"},
            "tab": {"tab_id": "w3:t1"},
            "root_pane": {"pane_id": "w3:p1", "terminal_id": "term-1"},
            "worktree": {
                "branch": "t1-store",
                "path": "/Volumes/Data/.herdr-worktrees/gear/t1-store",
                "is_bare": false,
            }
        })
    }

    #[test]
    fn refusal_codes_prove_nothing_ran() {
        for code in [
            "agent_blocked",
            "not_found",
            "invalid_request",
            "invalid_params",
            "agent_not_found",
            "pane_not_found",
            "pane_not_available",
            "agent_pane_busy",
            "agent_pane_unavailable",
            "agent_name_taken",
            "unsupported_agent_kind",
            "workspace_not_found",
        ] {
            assert!(is_refused(code), "{code} must be a refusal");
        }
        assert!(!is_refused("something_new"), "an unknown code is uncertain");
    }

    #[test]
    fn only_unambiguous_codes_mean_no_agent() {
        assert!(is_missing("pane_not_found"));
        assert!(is_missing("agent_not_found"));
        assert!(
            !is_missing("not_found"),
            "an unexplained not_found stays an observation failure"
        );
    }

    #[test]
    fn worktree_created_reply_maps_to_the_four_fields() {
        let Outcome::Ok(w) = worktree_from_reply(&created_reply()) else {
            panic!("a complete reply must map to a worktree");
        };
        assert_eq!(
            w,
            Worktree {
                workspace_id: "w3".into(),
                pane_id: "w3:p1".into(),
                path: "/Volumes/Data/.herdr-worktrees/gear/t1-store".into(),
                branch: "t1-store".into(),
            }
        );
    }

    #[test]
    fn worktree_opened_reply_maps_to_the_four_fields() {
        let mut reply = created_reply();
        reply["type"] = serde_json::json!("worktree_opened");
        reply["already_open"] = serde_json::json!(false);
        let Outcome::Ok(w) = worktree_from_reply(&reply) else {
            panic!("a complete reply must map to a worktree");
        };
        assert_eq!(w.workspace_id, "w3");
        assert_eq!(w.pane_id, "w3:p1");
        assert_eq!(w.branch, "t1-store");
        assert_eq!(w.path, "/Volumes/Data/.herdr-worktrees/gear/t1-store");
    }

    #[test]
    fn incomplete_worktree_reply_is_uncertain() {
        assert!(
            matches!(worktree_from_reply(&Value::Null), Outcome::Uncertain { .. }),
            "a null reply names no worktree"
        );

        let mut no_branch = created_reply();
        no_branch["worktree"]["branch"] = Value::Null;
        assert!(
            matches!(worktree_from_reply(&no_branch), Outcome::Uncertain { .. }),
            "a null branch leaves the worktree unnamed"
        );

        let mut no_path = created_reply();
        no_path["worktree"]["path"] = Value::Null;
        assert!(
            matches!(worktree_from_reply(&no_path), Outcome::Uncertain { .. }),
            "a null path leaves the worktree unnamed"
        );
    }
}
