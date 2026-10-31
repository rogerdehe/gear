//! Scriptable in-memory Herdr for tests (design/15).
//!
//! Tests set panes/agents/screens and failure modes, run the engine or CLI
//! logic, then assert on [`FakeHerdr::calls`].

use crate::{
    AgentInfo, AgentSession, AgentStatus, HerdrAct, HerdrRead, ObserveError, Outcome, PaneProcess,
    PaneProcessInfo, Worktree,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    OpenCodeApi {
        args: Vec<String>,
    },
    Prompt {
        pane: String,
        target: String,
        text: String,
    },
    Rename {
        pane: String,
        name: String,
    },
    /// Herdr `tab.rename` (T-278): the board names its own tab.
    TabRename {
        tab_id: String,
        label: String,
    },
    PaneRun {
        pane: String,
        command: String,
    },
    AgentStart {
        name: String,
        kind: String,
        pane: String,
        args: Vec<String>,
    },
    SendKeys {
        pane: String,
        keys: Vec<String>,
    },
    PaneClose {
        pane: String,
    },
    PaneType {
        pane: String,
        text: String,
    },
    SetTitle {
        pane: String,
        title: String,
    },
    Notify {
        title: String,
        body: String,
    },
    WorktreeCreate {
        repo: String,
        branch: String,
        base: String,
        label: String,
    },
    WorktreeOpen {
        repo: String,
        branch: String,
        label: String,
    },
    WorktreeOpenPath {
        repo: String,
        path: String,
        label: String,
    },
    WorktreeRemove {
        workspace: String,
    },
}

/// What the next side-effecting call (of any kind) should return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inject {
    Refuse { code: String },
    Uncertain,
}

#[derive(Debug, Clone)]
struct FakePane {
    agent: Option<(String, AgentStatus, Option<String>)>, // (kind, status, session_ref)
    /// The name Herdr holds for the agent (T-134). `None` = Herdr shows it
    /// unnamed (it cleared the name when it re-detected the occupant), and
    /// `prompt` refuses it, as the real door does.
    name: Option<String>,
    screen: String,
    revision: u64,
}

#[derive(Debug, Default)]
pub struct FakeHerdr {
    /// Ordered service replies, independent of terminal failure injection.
    pub api_replies: std::collections::VecDeque<Result<serde_json::Value, String>>,
    panes: BTreeMap<String, FakePane>,
    /// When true every read returns ObserveError.
    pub observe_fails: bool,
    /// When true only `agents()` returns ObserveError: the session cannot list
    /// its agents while single-pane reads still answer (a release asks for the
    /// list, T-133).
    pub agents_fail: bool,
    /// Consumed by the next side-effecting call.
    pub inject_next: Option<Inject>,
    pub calls: Vec<Call>,
    /// When true, a successful `prompt` flips the agent to Working (like a real turn start).
    pub prompt_starts_turn: bool,
    /// Worktrees by branch: (workspace id if open, root pane if open, path).
    worktrees: BTreeMap<String, (Option<String>, Option<String>, String)>,
    next_workspace: u32,
    /// When set, the next `agent_start` calls are refused with this code (the
    /// shell of a fresh worktree is still starting), as many as the count
    /// says. Other calls are unaffected.
    pub refuse_starts: Option<(String, u32)>,
    /// The names of the agents the fake started successfully (a refused start
    /// is not in here).
    pub started_agents: Vec<String>,
    /// Screens to show after an action: (a `pane_run` command substring, or a
    /// key sent, then the new screen). Models how the real UI renders after a
    /// submission, one action at a time.
    pub screen_after: Vec<(String, String)>,
    /// Text typed into a pane (`pane_type`) and not yet submitted by Enter.
    typed: BTreeMap<String, String>,
    /// The screen a freshly started agent shows (a real agent renders its UI,
    /// status bar included). Empty by default.
    pub start_screen: Option<String>,
    /// The processes each pane reports (T-104). A pane without an entry claims
    /// the process that asks (the test process), which is what a fixture that
    /// runs `gear` as itself needs; `set_pane_processes` makes a pane claim
    /// someone else.
    processes: BTreeMap<String, PaneProcessInfo>,
    /// The Herdr socket this fake stands in for, named in an identity refusal
    /// (T-314). `None` by default: an unnamed Herdr.
    pub socket: Option<String>,
    /// How long `agents()` takes before it answers (T-458): a test can make
    /// the observation slow without a real Herdr.
    pub agents_delay: Option<std::time::Duration>,
}

impl FakeHerdr {
    pub fn new() -> Self {
        FakeHerdr {
            prompt_starts_turn: false,
            ..Default::default()
        }
    }

    /// Add an empty shell pane (no agent).
    pub fn add_shell(&mut self, pane: &str) {
        self.panes.insert(
            pane.into(),
            FakePane {
                agent: None,
                name: None,
                screen: String::new(),
                revision: 0,
            },
        );
    }

    /// Add a pane hosting an agent of `kind` in `status`.
    pub fn add_agent(&mut self, pane: &str, kind: &str, status: AgentStatus) {
        self.panes.insert(
            pane.into(),
            FakePane {
                agent: Some((kind.into(), status, None)),
                name: None,
                screen: String::new(),
                revision: 1,
            },
        );
    }

    /// Add a pane whose agent carries `name` (T-134) — an agent Herdr still
    /// knows by name, as opposed to [`FakeHerdr::add_agent`], whose agent has
    /// no name (the M3.1 state after a restart).
    pub fn add_agent_named(&mut self, pane: &str, kind: &str, status: AgentStatus, name: &str) {
        self.add_agent(pane, kind, status);
        self.set_agent_name(pane, Some(name));
    }

    /// Set (or clear, with `None`) the name Herdr holds for the agent in
    /// `pane`: what a Herdr restart does to every agent it re-detects (T-134).
    pub fn set_agent_name(&mut self, pane: &str, name: Option<&str>) {
        let p = self.panes.get_mut(pane).expect("unknown pane");
        assert!(p.agent.is_some(), "pane {pane} hosts no agent");
        p.name = name.map(str::to_string);
        p.revision += 1;
    }

    /// The name Herdr holds for the agent in `pane`.
    pub fn agent_name(&self, pane: &str) -> Option<&str> {
        self.panes.get(pane).and_then(|p| p.name.as_deref())
    }

    /// The pane a prompt target names: a pane id, or the pane of the agent
    /// that carries the name (design §10.1: a delivery aims at the live
    /// agent name, never at the pane id).
    pub fn pane_of_target(&self, target: &str) -> Option<String> {
        if self.panes.contains_key(target) {
            return Some(target.to_string());
        }
        self.panes
            .iter()
            .find(|(_, pane)| pane.name.as_deref() == Some(target))
            .map(|(id, _)| id.clone())
    }

    /// The panes renamed to `name`, in call order.
    pub fn renames_to(&self, name: &str) -> Vec<&str> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Rename { pane, name: n } if n == name => Some(pane.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The tabs renamed to `label`, in call order (T-278).
    pub fn tab_renames_to(&self, label: &str) -> Vec<&str> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::TabRename { tab_id, label: l } if l == label => Some(tab_id.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn set_status(&mut self, pane: &str, status: AgentStatus) {
        let p = self.panes.get_mut(pane).expect("unknown pane");
        let a = p.agent.as_mut().expect("pane has no agent");
        a.1 = status;
        p.revision += 1;
    }

    /// Set the shell and foreground processes of a pane (T-104). A pid that is
    /// not an ancestor of the caller is a pane the caller does not run in.
    pub fn set_pane_processes(&mut self, pane: &str, shell: Option<u32>, foreground: &[u32]) {
        self.processes.insert(
            pane.into(),
            PaneProcessInfo {
                pane_id: pane.into(),
                shell_pid: shell,
                tty: None,
                foreground_process_group_id: foreground.first().copied(),
                foreground_processes: foreground
                    .iter()
                    .map(|pid| PaneProcess {
                        pid: *pid,
                        name: String::new(),
                        cmdline: None,
                    })
                    .collect(),
            },
        );
    }

    /// The agent in `pane` reports a new session: what a hand restart of the
    /// agent looks like to the engine (T-97).
    pub fn set_agent_session(&mut self, pane: &str, session: &str) {
        let p = self.panes.get_mut(pane).expect("unknown pane");
        let a = p.agent.as_mut().expect("pane has no agent");
        a.2 = Some(session.to_string());
        p.revision += 1;
    }

    pub fn set_screen(&mut self, pane: &str, screen: &str) {
        self.panes.get_mut(pane).expect("unknown pane").screen = screen.into();
    }

    /// Register a worktree that Herdr already has open (the engine's
    /// integration worktree of a real repo).
    pub fn add_worktree(&mut self, branch: &str, path: &str, pane: &str) {
        let workspace = format!("wt-{}", branch.replace('/', "-"));
        self.worktrees.insert(
            branch.to_string(),
            (Some(workspace), Some(pane.to_string()), path.to_string()),
        );
    }

    /// Agent exits; pane becomes a shell.
    pub fn exit_agent(&mut self, pane: &str) {
        let p = self.panes.get_mut(pane).expect("unknown pane");
        p.agent = None;
        p.revision += 1;
    }

    pub fn prompts_to(&self, pane: &str) -> Vec<&str> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Prompt { pane: p, text, .. } if p == pane => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// What the engine aimed its prompts at: an agent name (design §10.1) or
    /// a pane id.
    pub fn prompt_targets(&self) -> Vec<&str> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Prompt { target, .. } => Some(target.as_str()),
                _ => None,
            })
            .collect()
    }

    fn info(pane_id: &str, p: &FakePane) -> Option<AgentInfo> {
        let (kind, status, session) = p.agent.as_ref()?;
        Some(AgentInfo {
            pane_id: pane_id.to_string(),
            terminal_id: format!("term-{pane_id}"),
            workspace_id: pane_id.split(':').next().unwrap_or("").to_string(),
            tab_id: String::new(),
            agent: Some(kind.clone()),
            agent_status: *status,
            custom_status: None,
            focused: false,
            revision: p.revision,
            launch_pending: false,
            interactive_ready: true,
            agent_session: session.as_ref().map(|v| AgentSession {
                agent: kind.clone(),
                kind: "id".into(),
                source: format!("herdr:{kind}"),
                value: v.clone(),
            }),
        })
    }

    /// Branches that currently have a worktree (open or not).
    pub fn worktree_branches(&self) -> Vec<&str> {
        self.worktrees.keys().map(String::as_str).collect()
    }

    /// Open a fresh workspace for `branch`'s worktree, adding a shell root pane.
    fn open_worktree(&mut self, branch: &str) -> Worktree {
        self.next_workspace += 1;
        let ws = format!("wt{}", self.next_workspace);
        let pane = format!("{ws}:p1");
        self.add_shell(&pane);
        let path = format!("/fake-worktrees/{}", branch.replace('/', "-"));
        self.worktrees.insert(
            branch.into(),
            (Some(ws.clone()), Some(pane.clone()), path.clone()),
        );
        Worktree {
            workspace_id: ws,
            pane_id: pane,
            path,
            branch: branch.into(),
        }
    }

    fn act<T>(&mut self, call: Call, ok: impl FnOnce(&mut Self) -> Outcome<T>) -> Outcome<T> {
        self.calls.push(call);
        match self.inject_next.take() {
            Some(Inject::Refuse { code }) => Outcome::Refused {
                code,
                message: "injected".into(),
            },
            Some(Inject::Uncertain) => Outcome::Uncertain {
                reason: "injected".into(),
            },
            None => ok(self),
        }
    }

    /// After a submission, the screen the fake shows next (`screen_after`).
    fn render_after(&mut self, pane: &str, trigger: &str) {
        let next = self
            .screen_after
            .iter()
            .find(|(t, _)| trigger.contains(t.as_str()))
            .map(|(_, screen)| screen.clone());
        if let Some(screen) = next {
            if let Some(p) = self.panes.get_mut(pane) {
                p.screen = screen;
                p.revision += 1;
            }
        }
    }
}

impl HerdrRead for FakeHerdr {
    fn socket(&self) -> Option<String> {
        self.socket.clone()
    }

    fn agents(&mut self) -> Result<Vec<AgentInfo>, ObserveError> {
        if let Some(delay) = self.agents_delay {
            std::thread::sleep(delay);
        }
        if self.observe_fails || self.agents_fail {
            return Err(ObserveError("injected".into()));
        }
        Ok(self
            .panes
            .iter()
            .filter_map(|(id, p)| Self::info(id, p))
            .collect())
    }

    fn agent_get(&mut self, pane_id: &str) -> Result<Option<AgentInfo>, ObserveError> {
        if self.observe_fails {
            return Err(ObserveError("injected".into()));
        }
        Ok(self.panes.get(pane_id).and_then(|p| Self::info(pane_id, p)))
    }

    fn panes(&mut self) -> Result<Vec<String>, ObserveError> {
        Ok(self.panes.keys().cloned().collect())
    }

    fn agent_names(&mut self) -> Result<BTreeMap<String, Option<String>>, ObserveError> {
        if self.observe_fails {
            return Err(ObserveError("injected".into()));
        }
        Ok(self
            .panes
            .iter()
            .filter(|(_, p)| p.agent.is_some())
            .map(|(id, p)| (id.clone(), p.name.clone()))
            .collect())
    }

    fn pane_process_info(&mut self, pane_id: &str) -> Result<PaneProcessInfo, ObserveError> {
        if !self.panes.contains_key(pane_id) && !self.processes.contains_key(pane_id) {
            return Err(ObserveError(format!("no pane {pane_id}")));
        }
        Ok(self
            .processes
            .get(pane_id)
            .cloned()
            .unwrap_or_else(|| PaneProcessInfo {
                pane_id: pane_id.to_string(),
                shell_pid: Some(std::process::id()),
                tty: None,
                foreground_process_group_id: None,
                foreground_processes: Vec::new(),
            }))
    }

    fn read_tail(&mut self, pane_id: &str, lines: u32) -> Result<String, ObserveError> {
        if self.observe_fails {
            return Err(ObserveError("injected".into()));
        }
        let p = self
            .panes
            .get(pane_id)
            .ok_or_else(|| ObserveError(format!("no pane {pane_id}")))?;
        let all: Vec<&str> = p.screen.lines().collect();
        let start = all.len().saturating_sub(lines as usize);
        Ok(all[start..].join("\n"))
    }

    /// The whole visible screen: the fake's screen is the visible pane (tests
    /// that need a dialog above the tail window put it high in `set_screen`).
    fn read_screen(&mut self, pane_id: &str) -> Result<String, ObserveError> {
        if self.observe_fails {
            return Err(ObserveError("injected".into()));
        }
        let p = self
            .panes
            .get(pane_id)
            .ok_or_else(|| ObserveError(format!("no pane {pane_id}")))?;
        Ok(p.screen.clone())
    }
}

impl HerdrAct for FakeHerdr {
    fn opencode_api(&mut self, args: &[String]) -> Result<serde_json::Value, String> {
        self.calls.push(Call::OpenCodeApi {
            args: args.to_vec(),
        });
        self.api_replies
            .pop_front()
            .unwrap_or_else(|| Err("fake API unavailable".into()))
    }
    fn prompt(&mut self, target: &str, text: &str) -> Outcome<()> {
        // The target is an agent name (design §10.1) or a pane id. The call
        // log names the pane the text went to, so a test can ask "what did
        // this pane receive?".
        let pane = self.pane_of_target(target);
        let call = Call::Prompt {
            pane: pane.clone().unwrap_or_else(|| target.to_string()),
            // What the engine aimed at: an agent name (design §10.1) or a
            // pane id. The pane is where the text went.
            target: target.to_string(),
            text: text.into(),
        };
        let target = target.to_string();
        self.act(call, move |s| {
            // Herdr refuses a prompt when no agent of that name runs
            // (design §10.1): a pane that lost its agent, or a name no agent
            // carries.
            let Some(pane) = s.pane_of_target(&target) else {
                return Outcome::Refused {
                    code: "agent_not_ready".into(),
                    message: format!("no agent named {target}"),
                };
            };
            let starts = s.prompt_starts_turn;
            // Herdr refuses a prompt to an agent with no name (T-134):
            // "agent <pane> is not an active named agent".
            let unnamed = s
                .panes
                .get(&pane)
                .is_some_and(|p| p.agent.is_some() && p.name.is_none());
            if unnamed {
                return Outcome::Refused {
                    code: "agent_unnamed".into(),
                    message: format!("agent {pane} is not an active named agent"),
                };
            }
            match s.panes.get_mut(&pane).and_then(|p| p.agent.as_mut()) {
                None => Outcome::Refused {
                    code: "agent_not_ready".into(),
                    message: "no agent".into(),
                },
                Some(a) if a.1 == AgentStatus::Blocked => Outcome::Refused {
                    code: "agent_blocked".into(),
                    message: "blocked".into(),
                },
                Some(a) => {
                    if starts {
                        a.1 = AgentStatus::Working;
                    }
                    Outcome::Ok(())
                }
            }
        })
    }

    fn agent_rename(&mut self, pane_id: &str, name: &str) -> Outcome<()> {
        let call = Call::Rename {
            pane: pane_id.into(),
            name: name.into(),
        };
        let (pane, name) = (pane_id.to_string(), name.to_string());
        self.act(call, move |s| {
            // Never steal a name another pane holds (T-134).
            if s.panes
                .iter()
                .any(|(id, p)| id != &pane && p.name.as_deref() == Some(name.as_str()))
            {
                return Outcome::Refused {
                    code: "agent_name_taken".into(),
                    message: format!("the name {name} is held by another pane"),
                };
            }
            match s.panes.get_mut(&pane) {
                None => Outcome::Refused {
                    code: "not_found".into(),
                    message: "no pane".into(),
                },
                Some(p) if p.agent.is_none() => Outcome::Refused {
                    code: "agent_not_found".into(),
                    message: "no agent".into(),
                },
                Some(p) => {
                    p.name = Some(name);
                    p.revision += 1;
                    Outcome::Ok(())
                }
            }
        })
    }

    /// Herdr `tab.rename` (T-278): the board labels its own tab. A tab Herdr
    /// does not know is refused; the board goes on drawing either way.
    fn tab_rename(&mut self, tab_id: &str, label: &str) -> Outcome<()> {
        let call = Call::TabRename {
            tab_id: tab_id.into(),
            label: label.into(),
        };
        self.act(call, |_| Outcome::Ok(()))
    }

    fn pane_run(&mut self, pane_id: &str, command: &str) -> Outcome<()> {
        let pane = pane_id.to_string();
        let command_owned = command.to_string();
        self.act(
            Call::PaneRun {
                pane: pane_id.into(),
                command: command.into(),
            },
            move |s| {
                // Design §10.1: a shell command runs only in a pane that hosts no
                // agent — otherwise it is typed into the agent's input.
                if s.panes.get(&pane).is_some_and(|p| p.agent.is_some()) {
                    return Outcome::Refused {
                        code: crate::AGENT_PRESENT.into(),
                        message: format!("pane {pane} hosts an agent"),
                    };
                }
                if s.panes.contains_key(&pane) {
                    // Text + Enter is one submission (the socket path sends the
                    // Enter itself); the screen it produces comes next.
                    s.render_after(&pane, &command_owned);
                    Outcome::Ok(())
                } else {
                    Outcome::Refused {
                        code: "not_found".into(),
                        message: "no pane".into(),
                    }
                }
            },
        )
    }

    fn agent_start(
        &mut self,
        name: &str,
        kind: &str,
        pane_id: &str,
        args: &[String],
    ) -> Outcome<AgentInfo> {
        let call = Call::AgentStart {
            name: name.into(),
            kind: kind.into(),
            pane: pane_id.into(),
            args: args.to_vec(),
        };
        let (pane, kind) = (pane_id.to_string(), kind.to_string());
        let start_screen = self.start_screen.clone();
        if let Some((code, left)) = self.refuse_starts.clone() {
            self.calls.push(call);
            self.refuse_starts = (left > 1).then(|| (code.clone(), left - 1));
            return Outcome::Refused {
                code,
                message: "refused".into(),
            };
        }
        let wanted = name.to_string();
        self.act(call, move |s| {
            // A name another pane holds is refused before anything starts.
            let taken = s
                .panes
                .iter()
                .any(|(id, p)| id != &pane && p.name.as_deref() == Some(wanted.as_str()));
            if taken {
                return Outcome::Refused {
                    code: "agent_name_taken".into(),
                    message: "taken".into(),
                };
            }
            match s.panes.get_mut(&pane) {
                None => Outcome::Refused {
                    code: "not_found".into(),
                    message: "no pane".into(),
                },
                Some(p) if p.agent.is_some() => Outcome::Refused {
                    code: "pane_not_available".into(),
                    message: "occupied".into(),
                },
                Some(p) => {
                    // Herdr reports the session of the agent it started (the codex
                    // uuid), so the fake does too: a caller that resumes a session
                    // needs one to resume.
                    p.agent = Some((kind, AgentStatus::Idle, Some(format!("{name}:session"))));
                    // An agent Herdr started carries the name it was started with
                    // (T-134); a name another pane holds is refused first.
                    p.name = Some(wanted.clone());
                    if let Some(screen) = start_screen {
                        p.screen = screen;
                    }
                    p.revision += 1;
                    let p = p.clone();
                    s.started_agents.push(wanted.clone());
                    Outcome::Ok(Self::info(&pane, &p).expect("just started"))
                }
            }
        })
    }

    fn send_keys(&mut self, pane_id: &str, keys: &[String]) -> Outcome<()> {
        let pane = pane_id.to_string();
        let first = keys.first().cloned().unwrap_or_default();
        self.act(
            Call::SendKeys {
                pane: pane_id.into(),
                keys: keys.to_vec(),
            },
            move |s| {
                // Enter submits what was typed: the screen follows the typed text.
                let typed = if first == "enter" {
                    s.typed.remove(&pane)
                } else {
                    None
                };
                match typed {
                    Some(text) if !text.is_empty() => s.render_after(&pane, &text),
                    _ => s.render_after(&pane, &first),
                }
                Outcome::Ok(())
            },
        )
    }

    fn pane_type(&mut self, pane_id: &str, text: &str) -> Outcome<()> {
        let (pane, typed) = (pane_id.to_string(), text.to_string());
        self.act(
            Call::PaneType {
                pane: pane_id.into(),
                text: text.into(),
            },
            move |s| {
                s.typed.entry(pane).or_default().push_str(&typed);
                Outcome::Ok(())
            },
        )
    }

    /// Nothing to settle: the fake records the keystrokes and has no TUI to
    /// draw them (T-127) — so no test sleeps on the wall clock.
    fn type_settle(&mut self) {}

    fn pane_close(&mut self, pane_id: &str) -> Outcome<()> {
        let pane = pane_id.to_string();
        self.act(
            Call::PaneClose {
                pane: pane_id.into(),
            },
            move |s| {
                s.panes.remove(&pane);
                // A worktree's only pane closed: its workspace is gone too.
                for entry in s.worktrees.values_mut() {
                    if entry.1.as_deref() == Some(pane.as_str()) {
                        entry.0 = None;
                        entry.1 = None;
                    }
                }
                Outcome::Ok(())
            },
        )
    }

    fn set_title(&mut self, pane_id: &str, title: &str) -> Outcome<()> {
        self.act(
            Call::SetTitle {
                pane: pane_id.into(),
                title: title.into(),
            },
            |_| Outcome::Ok(()),
        )
    }

    fn notify(&mut self, title: &str, body: &str) -> Outcome<()> {
        self.act(
            Call::Notify {
                title: title.into(),
                body: body.into(),
            },
            |_| Outcome::Ok(()),
        )
    }

    fn worktree_create(
        &mut self,
        repo: &str,
        branch: &str,
        base: &str,
        label: &str,
    ) -> Outcome<Worktree> {
        let call = Call::WorktreeCreate {
            repo: repo.into(),
            branch: branch.into(),
            base: base.into(),
            label: label.into(),
        };
        let branch = branch.to_string();
        self.act(call, move |s| {
            if s.worktrees.contains_key(&branch) {
                return Outcome::Refused {
                    code: "invalid_request".into(),
                    message: "branch exists".into(),
                };
            }
            Outcome::Ok(s.open_worktree(&branch))
        })
    }

    fn worktree_open(&mut self, repo: &str, branch: &str, label: &str) -> Outcome<Worktree> {
        let call = Call::WorktreeOpen {
            repo: repo.into(),
            branch: branch.into(),
            label: label.into(),
        };
        let branch = branch.to_string();
        self.act(call, move |s| match s.worktrees.get(&branch).cloned() {
            None => Outcome::Refused {
                code: "worktree_not_found".into(),
                message: "worktree branch not found".into(),
            },
            Some((Some(ws), Some(pane), path)) => Outcome::Ok(Worktree {
                workspace_id: ws,
                pane_id: pane,
                path,
                branch,
            }),
            Some(_) => Outcome::Ok(s.open_worktree(&branch)),
        })
    }

    fn worktree_open_path(&mut self, repo: &str, path: &str, label: &str) -> Outcome<Worktree> {
        let call = Call::WorktreeOpenPath {
            repo: repo.into(),
            path: path.into(),
            label: label.into(),
        };
        let path = path.to_string();
        self.act(call, move |s| {
            let Some(branch) = s
                .worktrees
                .iter()
                .find(|(_, v)| v.2 == path)
                .map(|(k, _)| k.clone())
            else {
                return Outcome::Refused {
                    code: "worktree_not_found".into(),
                    message: "no worktree at that path".into(),
                };
            };
            match s.worktrees.get(&branch).cloned() {
                Some((Some(ws), Some(pane), path)) if s.panes.contains_key(&pane) => {
                    Outcome::Ok(Worktree {
                        workspace_id: ws,
                        pane_id: pane,
                        path,
                        branch,
                    })
                }
                _ => Outcome::Ok(s.open_worktree(&branch)),
            }
        })
    }

    fn worktree_remove(&mut self, workspace_id: &str) -> Outcome<()> {
        let ws = workspace_id.to_string();
        self.act(
            Call::WorktreeRemove {
                workspace: ws.clone(),
            },
            move |s| {
                let Some(branch) = s
                    .worktrees
                    .iter()
                    .find(|(_, v)| v.0.as_deref() == Some(&ws))
                    .map(|(k, _)| k.clone())
                else {
                    return Outcome::Refused {
                        code: "not_found".into(),
                        message: "no workspace".into(),
                    };
                };
                if let Some((_, Some(pane), _)) = s.worktrees.remove(&branch) {
                    s.panes.remove(&pane);
                }
                Outcome::Ok(())
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_records_and_respects_blocked_and_injection() {
        let mut h = FakeHerdr::new();
        h.add_agent_named("w1:p1", "opencode", AgentStatus::Idle, "p-heat");
        assert_eq!(h.prompt("w1:p1", "hi"), Outcome::Ok(()));
        h.set_status("w1:p1", AgentStatus::Blocked);
        assert!(matches!(h.prompt("w1:p1", "x"), Outcome::Refused { .. }));
        h.set_status("w1:p1", AgentStatus::Idle);
        h.inject_next = Some(Inject::Uncertain);
        assert!(matches!(h.prompt("w1:p1", "y"), Outcome::Uncertain { .. }));
        assert_eq!(h.prompts_to("w1:p1"), vec!["hi", "x", "y"]);
    }

    #[test]
    fn observe_failure_is_an_error_not_empty() {
        let mut h = FakeHerdr::new();
        h.add_agent("w1:p1", "codex", AgentStatus::Idle);
        h.observe_fails = true;
        assert!(h.agents().is_err());
    }

    #[test]
    fn worktree_create_open_remove() {
        let mut h = FakeHerdr::new();
        let w = match h.worktree_create("/repo", "data/t1-store", "dev", "gear-data") {
            Outcome::Ok(w) => w,
            o => panic!("{o:?}"),
        };
        assert_eq!(w.path, "/fake-worktrees/data-t1-store");
        assert!(
            h.agent_get(&w.pane_id).unwrap().is_none(),
            "root pane is an empty shell"
        );
        assert!(matches!(
            h.worktree_create("/repo", "data/t1-store", "dev", "x"),
            Outcome::Refused { .. }
        ));
        assert_eq!(
            h.worktree_open("/repo", "data/t1-store", "x"),
            Outcome::Ok(w.clone())
        );
        assert_eq!(h.worktree_remove(&w.workspace_id), Outcome::Ok(()));
        assert!(h.worktree_branches().is_empty());
        assert!(matches!(
            h.worktree_open("/repo", "data/t1-store", "x"),
            Outcome::Refused { .. }
        ));
    }

    #[test]
    fn tab_rename_is_recorded_with_its_tab_and_label() {
        let mut h = FakeHerdr::new();
        assert_eq!(h.tab_rename("w1:t2", "gear board"), Outcome::Ok(()));
        assert_eq!(
            h.calls,
            vec![Call::TabRename {
                tab_id: "w1:t2".into(),
                label: "gear board".into()
            }]
        );
        assert_eq!(h.tab_renames_to("gear board"), vec!["w1:t2"]);
        // A refused rename is recorded too, and the caller may ignore it.
        h.inject_next = Some(Inject::Refuse {
            code: "not_found".into(),
        });
        assert!(matches!(
            h.tab_rename("w1:t9", "p board"),
            Outcome::Refused { .. }
        ));
    }

    #[test]
    fn tail_returns_last_lines() {
        let mut h = FakeHerdr::new();
        h.add_agent("w1:p1", "codex", AgentStatus::Idle);
        h.set_screen("w1:p1", "a\nb\nc\nd");
        assert_eq!(h.read_tail("w1:p1", 2).unwrap(), "c\nd");
    }
}
