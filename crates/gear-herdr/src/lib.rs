//! GEAR's only door to Herdr (design/04).
//!
//! - [`HerdrRead`]: observation. Usable by `gear` CLI commands (identity
//!   fingerprint) and by the engine. Never has side effects.
//! - [`HerdrAct`]: side effects (prompt, start, close…). **Only the engine**
//!   may hold one.
//! - [`SocketHerdr`]: real implementation over the Herdr socket, built on the
//!   `board-herdr` client (MIT, git dependency pinned in the workspace Cargo.toml).
//! - [`fake::FakeHerdr`]: scriptable in-memory Herdr for tests.
//!
//! Outcome rules (design/04, /08): an action whose effect may have happened
//! (timeout, disconnect, undecodable reply) is `Uncertain`, never `Refused`.
//! Only a reply that proves nothing was executed is `Refused`.

pub mod fake;
pub mod opencode;
pub mod profile;
pub mod quota_bar;
pub use quota_bar::{QuotaBar, QuotaReading};

use serde_json::{json, Value};
use std::collections::BTreeMap;

pub use board_herdr::{AgentInfo, AgentSession, AgentStatus};

/// What making sure an agent carries its name did (T-134).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ensured {
    /// Herdr already reports the wanted name.
    Named,
    /// The agent was renamed now.
    Renamed,
    /// Another pane holds the name: it is reported, never stolen.
    Taken { holder: Option<String> },
    /// The pane hosts no agent (between agents, or a shell).
    NoAgent,
    /// Herdr refused the rename or the outcome is unknown.
    Failed(String),
}

/// The other pane that holds `name`, if any.
fn name_holder(names: &BTreeMap<String, Option<String>>, pane: &str, name: &str) -> Option<String> {
    names
        .iter()
        .find(|(other, held)| other.as_str() != pane && held.as_deref() == Some(name))
        .map(|(other, _)| other.clone())
}

/// Make sure the agent in `pane` carries `name`, against a snapshot of
/// [`HerdrRead::agent_names`] already read. A name another pane holds is never
/// stolen.
pub fn ensure_agent_name_with(
    herdr: &mut dyn HerdrAct,
    names: &BTreeMap<String, Option<String>>,
    pane: &str,
    name: &str,
) -> Ensured {
    match names.get(pane) {
        // The pane hosts no agent: nothing to name (a seat between agents).
        None => return Ensured::NoAgent,
        Some(Some(current)) if current == name => return Ensured::Named,
        Some(_) => {}
    }
    if let Some(pane) = name_holder(names, pane, name) {
        return Ensured::Taken { holder: Some(pane) };
    }
    match herdr.agent_rename(pane, name) {
        Outcome::Ok(()) => Ensured::Renamed,
        Outcome::Refused { code, .. } if code == "agent_name_taken" => Ensured::Taken {
            holder: name_holder(names, pane, name),
        },
        Outcome::Refused { code, message } => Ensured::Failed(format!("{code}: {message}")),
        Outcome::Uncertain { reason } => Ensured::Failed(reason),
    }
}

/// [`ensure_agent_name_with`] with its own read of every agent's name: what a
/// caller outside the engine tick uses before it prompts a pane (T-134). A read
/// that fails is a failure to ensure, never a rename.
pub fn ensure_agent_name(herdr: &mut dyn HerdrAct, pane: &str, name: &str) -> Ensured {
    match herdr.agent_names() {
        Ok(names) => ensure_agent_name_with(herdr, &names, pane, name),
        Err(error) => Ensured::Failed(error.0),
    }
}

/// Does an agent of this kind queue the text typed while it works, and read it
/// after the current step? (T-130)
///
/// pi and claude do: a prompt typed mid-turn waits in their input queue. An
/// opencode or codex turn takes the text as keystrokes into the running turn,
/// so only an urgent message goes in while it works. `kind` is the Herdr agent
/// kind of the seat's active binding.
pub fn queues_input(kind: &str) -> bool {
    matches!(kind, "pi" | "claude")
}

/// Answer the startup dialog an agent's screen shows (T-457).
///
/// Reads the pane's tail, matches the profile's dialogs and sends the keys of
/// the one that is open. `None` = no dialog open (or the screen could not be
/// read): nothing was sent. A dialog is answered although Herdr reports the
/// agent done/idle — the screen is the only place it shows, and text typed
/// into it is lost.
pub fn answer_dialog(
    herdr: &mut dyn HerdrAct,
    pane: &str,
    profile: &Profile,
) -> Option<Outcome<()>> {
    let screen = herdr.read_tail(pane, profile.tail_lines.max(1)).ok()?;
    let dialog = profile.dialog(&screen)?;
    Some(herdr.send_keys(pane, &dialog.keys))
}

/// The processes of a pane, as Herdr reports them (T-104): its shell, the
/// foreground process group, and the terminal (`/dev/ttys…`). The one pane
/// fact a shell cannot set with an environment variable — an identity check
/// that trusts `GEAR_CTX`/`HERDR_PANE_ID` alone can be claimed by any agent.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct PaneProcessInfo {
    pub pane_id: String,
    #[serde(default)]
    pub shell_pid: Option<u32>,
    /// The pane's terminal, when Herdr knows it.
    #[serde(default)]
    pub tty: Option<String>,
    #[serde(default)]
    pub foreground_process_group_id: Option<u32>,
    #[serde(default)]
    pub foreground_processes: Vec<PaneProcess>,
}

/// One process running in a pane (T-104).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct PaneProcess {
    pub pid: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub cmdline: Option<String>,
}
pub use profile::{
    builtin, builtins, classify_screen, render_args, validate_template, Dialog, Profile,
    ProfilePatch, ScreenFlag,
};

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
    /// The whole **visible** screen, for the model switch (T-33): the picker,
    /// the "Select variant" dialog and the status bar are drawn in the visible
    /// pane, and a dialog can sit far above the last 16 lines.
    fn read_screen(&mut self, pane_id: &str) -> Result<String, ObserveError>;
    /// The pane's shell and foreground processes (T-104): an identity check
    /// proves the caller runs in the pane it claims with them.
    fn pane_process_info(&mut self, pane_id: &str) -> Result<PaneProcessInfo, ObserveError>;
    /// Every pane of the session, agents and shells alike (T-104): the pane a
    /// caller really runs in may be a shell with no agent (the user's own
    /// terminal).
    fn panes(&mut self) -> Result<Vec<String>, ObserveError>;
    /// The name Herdr holds for every agent-bearing pane, by pane id (T-134).
    ///
    /// Herdr clears an agent's name when it re-detects the occupant (a server
    /// restart, a CLI that changes its session), and `agent.prompt` refuses a
    /// pane whose agent is unnamed. The typed [`AgentInfo`] of the board
    /// client does not carry the name, so this reads `agent.list` itself.
    /// `None` = Herdr shows the agent with no name.
    fn agent_names(&mut self) -> Result<BTreeMap<String, Option<String>>, ObserveError>;
    /// The Herdr socket this door asked, for an error a reader can trace
    /// (T-314). `None` = a fake or a door that does not know its socket.
    fn socket(&self) -> Option<String> {
        None
    }
}

pub trait HerdrAct: HerdrRead {
    /// OpenCode service requests, separate from terminal input. The real door
    /// runs the local CLI; fakes must opt in with scripted API replies.
    fn opencode_api(&mut self, _args: &[String]) -> Result<Value, String> {
        Err("OpenCode service API unavailable".into())
    }
    /// Submit `text` + Enter to the agent Herdr knows as `target` (no
    /// --wait). The target is the agent's **name** (design §10.1: a prompt
    /// targets the live agent name, never the pane id, so a pane whose agent
    /// is gone refuses the text instead of typing it into a shell).
    fn prompt(&mut self, target: &str, text: &str) -> Outcome<()>;
    /// Run a shell command in a shell pane (used to export GEAR_CTX before start).
    fn pane_run(&mut self, pane_id: &str, command: &str) -> Outcome<()>;
    /// Start an agent of `kind` in an available shell pane.
    fn agent_start(
        &mut self,
        name: &str,
        kind: &str,
        pane_id: &str,
        args: &[String],
    ) -> Outcome<AgentInfo>;
    /// Herdr `agent.rename`: give the agent in `pane_id` the name `name`
    /// (T-134). A name another pane holds is refused, never stolen.
    fn agent_rename(&mut self, pane_id: &str, name: &str) -> Outcome<()>;
    /// Herdr `tab.rename`: give the tab `tab_id` the label `label` (T-278).
    /// `gear board` names its own tab, so the tab bar shows the board's name
    /// instead of a number.
    fn tab_rename(&mut self, tab_id: &str, label: &str) -> Outcome<()>;
    fn send_keys(&mut self, pane_id: &str, keys: &[String]) -> Outcome<()>;
    /// Type `text` into the pane as keystrokes (`pane.send_text`), without
    /// Enter. Unlike `pane_run`/`prompt` (a paste), typed text triggers an
    /// agent's slash-command menu: `/model` typed opens OpenCode's picker,
    /// `/model` pasted is sent to the model as a message (L-71).
    fn pane_type(&mut self, pane_id: &str, text: &str) -> Outcome<()>;
    /// The pause the terminal needs between typed text and the Enter that
    /// follows it: an agent's TUI must take the keystrokes first, or the Enter
    /// arrives before the input shows it (L-71). The real terminal waits
    /// [`TYPE_SETTLE_MS`]; a fake has no TUI to wait for, and its wait is
    /// nothing — so a test never sleeps on the wall clock (T-127).
    fn type_settle(&mut self);
    fn pane_close(&mut self, pane_id: &str) -> Outcome<()>;
    fn set_title(&mut self, pane_id: &str, title: &str) -> Outcome<()>;
    fn notify(&mut self, title: &str, body: &str) -> Outcome<()>;
    /// Herdr `worktree.create` (never focuses): new branch `branch` cut from
    /// `base` in the repo at `repo`, opened in a new workspace labelled `label`.
    fn worktree_create(
        &mut self,
        repo: &str,
        branch: &str,
        base: &str,
        label: &str,
    ) -> Outcome<Worktree>;
    /// Herdr `worktree.open` (never focuses): reopen the existing worktree of
    /// `branch` (temp-worker rework / resume).
    fn worktree_open(&mut self, repo: &str, branch: &str, label: &str) -> Outcome<Worktree>;
    /// Herdr `worktree.open --path`: reopen the worktree at `path`, whatever
    /// branch it is on now (a warm review seat's checkout moves to each job's
    /// review branch).
    fn worktree_open_path(&mut self, repo: &str, path: &str, label: &str) -> Outcome<Worktree>;
    /// Herdr `worktree.remove` (never forced): removes the checkout and its
    /// workspace; the branch is kept.
    fn worktree_remove(&mut self, workspace_id: &str) -> Outcome<()>;
}

/// How long the terminal needs after typed text before the Enter that follows
/// it: an agent's TUI has to draw and accept the keystrokes (L-71).
pub const TYPE_SETTLE_MS: u64 = 300;

/// Real Herdr over its unix socket. Each call opens its own connection to the
/// socket path (board-herdr `HerdrClient::call_inner`), so a Herdr server
/// restarted on the same path is reached on the next call; nothing reconnects.
pub struct SocketHerdr {
    client: board_herdr::HerdrClient,
    /// The path `connect` was given (T-314): an error names the Herdr it asked.
    socket: std::path::PathBuf,
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
        Ok(SocketHerdr {
            client,
            socket: socket_path.to_path_buf(),
        })
    }
}

impl HerdrRead for SocketHerdr {
    fn socket(&self) -> Option<String> {
        Some(self.socket.to_string_lossy().into_owned())
    }

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

    fn panes(&mut self) -> Result<Vec<String>, ObserveError> {
        let panes = self
            .client
            .pane_list(None)
            .map_err(|e| ObserveError(format!("pane.list failed: {e}")))?;
        Ok(panes.into_iter().map(|pane| pane.pane_id).collect())
    }

    fn agent_names(&mut self) -> Result<BTreeMap<String, Option<String>>, ObserveError> {
        let value = self
            .client
            .call("agent.list", json!({}))
            .map_err(|e| ObserveError(format!("agent.list failed: {e}")))?;
        let agents = value
            .get("agents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut names = BTreeMap::new();
        for agent in agents {
            let Some(pane) = agent.get("pane_id").and_then(Value::as_str) else {
                continue;
            };
            // Herdr reports `null` (and, in practice, `""` after a clear) for
            // an agent with no name: both are "unnamed".
            let name = agent
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_string);
            names.insert(pane.to_string(), name);
        }
        Ok(names)
    }

    fn pane_process_info(&mut self, pane_id: &str) -> Result<PaneProcessInfo, ObserveError> {
        let value = self
            .client
            .call(
                "pane.process_info",
                serde_json::json!({ "pane_id": pane_id }),
            )
            .map_err(|e| ObserveError(format!("pane.process_info failed: {e}")))?;
        let info = value
            .get("process_info")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        serde_json::from_value(info).map_err(|e| ObserveError(format!("pane.process_info: {e}")))
    }

    fn read_tail(&mut self, pane_id: &str, lines: u32) -> Result<String, ObserveError> {
        let read = self
            .client
            .pane_read(
                pane_id,
                board_herdr::ReadSource::RecentUnwrapped,
                Some(lines),
            )
            .map_err(|e| ObserveError(format!("pane.read failed: {e}")))?;
        Ok(read.text)
    }

    fn read_screen(&mut self, pane_id: &str) -> Result<String, ObserveError> {
        let read = self
            .client
            .pane_read(pane_id, board_herdr::ReadSource::Visible, None)
            .map_err(|e| ObserveError(format!("pane.read (visible) failed: {e}")))?;
        Ok(read.text)
    }
}

impl HerdrAct for SocketHerdr {
    fn opencode_api(&mut self, args: &[String]) -> Result<Value, String> {
        opencode::request(args)
    }
    fn prompt(&mut self, target: &str, text: &str) -> Outcome<()> {
        let params = board_herdr::AgentPromptParams {
            target: target.to_string(),
            text: text.to_string(),
            wait: None, // no --wait: "submitted" is confirmed by the next observation round
        };
        match self.client.agent_prompt(&params) {
            Ok(_) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn pane_run(&mut self, pane_id: &str, command: &str) -> Outcome<()> {
        // Design §10.1: a shell command runs only in a pane that hosts no
        // agent. With an agent in it, the command would be typed into the
        // agent's input — the wrong thing.
        match self.agent_get(pane_id) {
            Ok(Some(info)) => {
                return Outcome::Refused {
                    code: AGENT_PRESENT.to_string(),
                    message: format!(
                        "pane {pane_id} hosts the {} agent {}",
                        info.agent.as_deref().unwrap_or("unknown"),
                        info.pane_id
                    ),
                }
            }
            // A pane that hosts no agent is a shell: the command belongs there.
            Ok(None) => {}
            // The pane could not be read: nothing is typed.
            Err(error) => return Outcome::Uncertain { reason: error.0 },
        }
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

    fn agent_start(
        &mut self,
        name: &str,
        kind: &str,
        pane_id: &str,
        args: &[String],
    ) -> Outcome<AgentInfo> {
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

    fn agent_rename(&mut self, pane_id: &str, name: &str) -> Outcome<()> {
        let params = json!({ "target": pane_id, "name": name });
        match self.client.call("agent.rename", params) {
            Ok(_) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn tab_rename(&mut self, tab_id: &str, label: &str) -> Outcome<()> {
        let params = json!({ "tab_id": tab_id, "label": label });
        match self.client.call("tab.rename", params) {
            Ok(_) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn send_keys(&mut self, pane_id: &str, keys: &[String]) -> Outcome<()> {
        match self.client.pane_send_keys(pane_id, keys) {
            Ok(()) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn pane_type(&mut self, pane_id: &str, text: &str) -> Outcome<()> {
        let params = json!({ "pane_id": pane_id, "text": text });
        match self.client.call("pane.send_text", params) {
            Ok(_) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn type_settle(&mut self) {
        std::thread::sleep(std::time::Duration::from_millis(TYPE_SETTLE_MS));
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
        match self
            .client
            .notification_show(title, Some(body), board_herdr::NotificationSound::None)
        {
            Ok(_) => Outcome::Ok(()),
            Err(e) => outcome_from_error(&e),
        }
    }

    fn worktree_create(
        &mut self,
        repo: &str,
        branch: &str,
        base: &str,
        label: &str,
    ) -> Outcome<Worktree> {
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

    fn worktree_open_path(&mut self, repo: &str, path: &str, label: &str) -> Outcome<Worktree> {
        let params = json!({
            "cwd": repo,
            "path": path,
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
            Outcome::Refused {
                code: code.clone(),
                message: message.clone(),
            }
        }
        board_herdr::HerdrError::Protocol { code, message } => Outcome::Uncertain {
            reason: format!("herdr error [{code}]: {message}"),
        },
        other => Outcome::Uncertain {
            reason: other.to_string(),
        },
    }
}

/// The code of a refusal that says "the pane hosts an agent" (design §10.1).
pub const AGENT_PRESENT: &str = "agent_present";

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
    // `worktree.open` on a branch with no worktree (seen on 0.9.1).
    "worktree_not_found",
    // A prompt to an agent Herdr has not registered yet (right after
    // `agent.start`, seen 2026-10-23): nothing was typed; retry next tick.
    "agent_not_ready",
    // A prompt to an agent with no name (T-134: Herdr says "agent <pane> is
    // not an active named agent"): nothing was typed. The engine names the
    // agent first, so this is the code for "that did not happen yet".
    "agent_unnamed",
    // A shell command aimed at a pane that hosts an agent (design §10.1):
    // nothing was typed.
    AGENT_PRESENT,
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
/// A reply missing the workspace, the pane or the path → Uncertain: the
/// worktree may exist even though GEAR cannot name it.
///
/// A reply **without a branch** is not incomplete: it is a **detached**
/// checkout — its HEAD is kept and the branch was let go of (T-69) — so
/// Herdr has no branch to name. The worktree is named with an empty branch,
/// and the caller that only needs its pane and path goes on (B02-04: a
/// detached answer used to leave a reviewer seat unknown).
fn worktree_from_reply(reply: &Value) -> Outcome<Worktree> {
    let workspace_id = reply
        .pointer("/workspace/workspace_id")
        .and_then(Value::as_str);
    let pane_id = reply.pointer("/root_pane/pane_id").and_then(Value::as_str);
    let path = reply.pointer("/worktree/path").and_then(Value::as_str);
    let branch = reply
        .pointer("/worktree/branch")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match (workspace_id, pane_id, path) {
        (Some(workspace_id), Some(pane_id), Some(path)) => Outcome::Ok(Worktree {
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

        let mut no_path = created_reply();
        no_path["worktree"]["path"] = Value::Null;
        assert!(
            matches!(worktree_from_reply(&no_path), Outcome::Uncertain { .. }),
            "a null path leaves the worktree unnamed"
        );

        let mut no_pane = created_reply();
        no_pane["root_pane"]["pane_id"] = Value::Null;
        assert!(
            matches!(worktree_from_reply(&no_pane), Outcome::Uncertain { .. }),
            "a reply without its pane names no place to start the agent"
        );
    }

    /// B02-04: a **detached** checkout answers without a branch — its HEAD is
    /// kept and the branch was let go of (T-69). That is a named worktree, not
    /// an incomplete answer: the reviewer seat goes on (its pane and path are
    /// what it needs).
    #[test]
    fn a_detached_worktree_reply_is_named_without_its_branch() {
        for field in [Value::Null, Value::String(String::new())] {
            let mut reply = created_reply();
            reply["worktree"]["branch"] = field.clone();
            let Outcome::Ok(worktree) = worktree_from_reply(&reply) else {
                panic!("a detached worktree is a named worktree (branch {field:?})");
            };
            assert_eq!(
                worktree.branch, "",
                "detached: no branch holds the checkout"
            );
            assert_eq!(worktree.pane_id, "w3:p1", "the pane is still named");
        }
        // A reply that names no branch at all (the key is missing) is the same.
        let mut missing = created_reply();
        missing["worktree"]
            .as_object_mut()
            .unwrap()
            .remove("branch");
        let Outcome::Ok(worktree) = worktree_from_reply(&missing) else {
            panic!("a reply without the branch key is detached too");
        };
        assert_eq!(worktree.branch, "");
    }
}
