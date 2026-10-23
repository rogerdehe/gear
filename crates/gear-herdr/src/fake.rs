//! Scriptable in-memory Herdr for tests (design/15).
//!
//! Tests set panes/agents/screens and failure modes, run the engine or CLI
//! logic, then assert on [`FakeHerdr::calls`].

use crate::{AgentInfo, AgentSession, AgentStatus, HerdrAct, HerdrRead, ObserveError, Outcome, Worktree};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Prompt { pane: String, text: String },
    PaneRun { pane: String, command: String },
    AgentStart { name: String, kind: String, pane: String, args: Vec<String> },
    SendKeys { pane: String, keys: Vec<String> },
    PaneClose { pane: String },
    SetTitle { pane: String, title: String },
    Notify { title: String, body: String },
    WorktreeCreate { repo: String, branch: String, base: String, label: String },
    WorktreeOpen { repo: String, branch: String, label: String },
    WorktreeRemove { workspace: String },
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
    screen: String,
    revision: u64,
}

#[derive(Debug, Default)]
pub struct FakeHerdr {
    panes: BTreeMap<String, FakePane>,
    /// When true every read returns ObserveError.
    pub observe_fails: bool,
    /// Consumed by the next side-effecting call.
    pub inject_next: Option<Inject>,
    pub calls: Vec<Call>,
    /// When true, a successful `prompt` flips the agent to Working (like a real turn start).
    pub prompt_starts_turn: bool,
    /// Worktrees by branch: (workspace id if open, root pane if open, path).
    worktrees: BTreeMap<String, (Option<String>, Option<String>, String)>,
    next_workspace: u32,
    /// When set, the next `agent_start` is refused with this code (the shell
    /// of a fresh worktree is still starting). Other calls are unaffected.
    pub refuse_next_start: Option<String>,
    /// The names of the agents the fake started successfully (a refused start
    /// is not in here).
    pub started_agents: Vec<String>,
}

impl FakeHerdr {
    pub fn new() -> Self {
        FakeHerdr { prompt_starts_turn: false, ..Default::default() }
    }

    /// Add an empty shell pane (no agent).
    pub fn add_shell(&mut self, pane: &str) {
        self.panes.insert(pane.into(), FakePane { agent: None, screen: String::new(), revision: 0 });
    }

    /// Add a pane hosting an agent of `kind` in `status`.
    pub fn add_agent(&mut self, pane: &str, kind: &str, status: AgentStatus) {
        self.panes.insert(
            pane.into(),
            FakePane { agent: Some((kind.into(), status, None)), screen: String::new(), revision: 1 },
        );
    }

    pub fn set_status(&mut self, pane: &str, status: AgentStatus) {
        let p = self.panes.get_mut(pane).expect("unknown pane");
        let a = p.agent.as_mut().expect("pane has no agent");
        a.1 = status;
        p.revision += 1;
    }

    pub fn set_session_ref(&mut self, pane: &str, session: Option<&str>) {
        let p = self.panes.get_mut(pane).expect("unknown pane");
        p.agent.as_mut().expect("pane has no agent").2 = session.map(str::to_string);
    }

    /// Herdr reports the agent's session: the agent in `pane` gets one (as it
    /// does a moment after the start, when the session is first known).
    pub fn report_session(&mut self, pane: &str, session: &str) {
        self.set_session_ref(pane, Some(session));
    }

    pub fn set_screen(&mut self, pane: &str, screen: &str) {
        self.panes.get_mut(pane).expect("unknown pane").screen = screen.into();
    }

    /// Agent exits; pane becomes a shell.
    pub fn exit_agent(&mut self, pane: &str) {
        let p = self.panes.get_mut(pane).expect("unknown pane");
        p.agent = None;
        p.revision += 1;
    }

    pub fn remove_pane(&mut self, pane: &str) {
        self.panes.remove(pane);
    }

    pub fn prompts_to(&self, pane: &str) -> Vec<&str> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Prompt { pane: p, text } if p == pane => Some(text.as_str()),
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
        self.worktrees.insert(branch.into(), (Some(ws.clone()), Some(pane.clone()), path.clone()));
        Worktree { workspace_id: ws, pane_id: pane, path, branch: branch.into() }
    }

    fn act<T>(&mut self, call: Call, ok: impl FnOnce(&mut Self) -> Outcome<T>) -> Outcome<T> {
        self.calls.push(call);
        match self.inject_next.take() {
            Some(Inject::Refuse { code }) => Outcome::Refused { code, message: "injected".into() },
            Some(Inject::Uncertain) => Outcome::Uncertain { reason: "injected".into() },
            None => ok(self),
        }
    }
}

impl HerdrRead for FakeHerdr {
    fn agents(&mut self) -> Result<Vec<AgentInfo>, ObserveError> {
        if self.observe_fails {
            return Err(ObserveError("injected".into()));
        }
        Ok(self.panes.iter().filter_map(|(id, p)| Self::info(id, p)).collect())
    }

    fn agent_get(&mut self, pane_id: &str) -> Result<Option<AgentInfo>, ObserveError> {
        if self.observe_fails {
            return Err(ObserveError("injected".into()));
        }
        Ok(self.panes.get(pane_id).and_then(|p| Self::info(pane_id, p)))
    }

    fn read_tail(&mut self, pane_id: &str, lines: u32) -> Result<String, ObserveError> {
        if self.observe_fails {
            return Err(ObserveError("injected".into()));
        }
        let p = self.panes.get(pane_id).ok_or_else(|| ObserveError(format!("no pane {pane_id}")))?;
        let all: Vec<&str> = p.screen.lines().collect();
        let start = all.len().saturating_sub(lines as usize);
        Ok(all[start..].join("\n"))
    }
}

impl HerdrAct for FakeHerdr {
    fn prompt(&mut self, pane_id: &str, text: &str) -> Outcome<()> {
        let call = Call::Prompt { pane: pane_id.into(), text: text.into() };
        let pane = pane_id.to_string();
        self.act(call, move |s| {
            let starts = s.prompt_starts_turn;
            match s.panes.get_mut(&pane).and_then(|p| p.agent.as_mut()) {
                None => Outcome::Refused { code: "not_found".into(), message: "no agent".into() },
                Some(a) if a.1 == AgentStatus::Blocked => {
                    Outcome::Refused { code: "agent_blocked".into(), message: "blocked".into() }
                }
                Some(a) => {
                    if starts {
                        a.1 = AgentStatus::Working;
                    }
                    Outcome::Ok(())
                }
            }
        })
    }

    fn pane_run(&mut self, pane_id: &str, command: &str) -> Outcome<()> {
        let pane = pane_id.to_string();
        self.act(Call::PaneRun { pane: pane_id.into(), command: command.into() }, move |s| {
            if s.panes.contains_key(&pane) {
                Outcome::Ok(())
            } else {
                Outcome::Refused { code: "not_found".into(), message: "no pane".into() }
            }
        })
    }

    fn agent_start(&mut self, name: &str, kind: &str, pane_id: &str, args: &[String]) -> Outcome<AgentInfo> {
        let call = Call::AgentStart {
            name: name.into(),
            kind: kind.into(),
            pane: pane_id.into(),
            args: args.to_vec(),
        };
        let (pane, kind) = (pane_id.to_string(), kind.to_string());
        if let Some(code) = self.refuse_next_start.take() {
            self.calls.push(call);
            return Outcome::Refused { code, message: "refused".into() };
        }
        self.act(call, move |s| match s.panes.get_mut(&pane) {
            None => Outcome::Refused { code: "not_found".into(), message: "no pane".into() },
            Some(p) if p.agent.is_some() => {
                Outcome::Refused { code: "pane_not_available".into(), message: "occupied".into() }
            }
            Some(p) => {
                // Herdr reports the session of the agent it started
                // asynchronously: the reply of `agent.start` often carries
                // none, and the session shows up in the next observation. The
                // fake does the same — a test that needs a session reports it
                // with `report_session`.
                p.agent = Some((kind, AgentStatus::Idle, None));
                p.revision += 1;
                let p = p.clone();
                s.started_agents.push(name.to_string());
                Outcome::Ok(Self::info(&pane, &p).expect("just started"))
            }
        })
    }

    fn send_keys(&mut self, pane_id: &str, keys: &[String]) -> Outcome<()> {
        self.act(Call::SendKeys { pane: pane_id.into(), keys: keys.to_vec() }, |_| Outcome::Ok(()))
    }

    fn pane_close(&mut self, pane_id: &str) -> Outcome<()> {
        let pane = pane_id.to_string();
        self.act(Call::PaneClose { pane: pane_id.into() }, move |s| {
            s.panes.remove(&pane);
            Outcome::Ok(())
        })
    }

    fn set_title(&mut self, pane_id: &str, title: &str) -> Outcome<()> {
        self.act(Call::SetTitle { pane: pane_id.into(), title: title.into() }, |_| Outcome::Ok(()))
    }

    fn notify(&mut self, title: &str, body: &str) -> Outcome<()> {
        self.act(Call::Notify { title: title.into(), body: body.into() }, |_| Outcome::Ok(()))
    }

    fn worktree_create(&mut self, repo: &str, branch: &str, base: &str, label: &str) -> Outcome<Worktree> {
        let call = Call::WorktreeCreate { repo: repo.into(), branch: branch.into(), base: base.into(), label: label.into() };
        let branch = branch.to_string();
        self.act(call, move |s| {
            if s.worktrees.contains_key(&branch) {
                return Outcome::Refused { code: "invalid_request".into(), message: "branch exists".into() };
            }
            Outcome::Ok(s.open_worktree(&branch))
        })
    }

    fn worktree_open(&mut self, repo: &str, branch: &str, label: &str) -> Outcome<Worktree> {
        let call = Call::WorktreeOpen { repo: repo.into(), branch: branch.into(), label: label.into() };
        let branch = branch.to_string();
        self.act(call, move |s| match s.worktrees.get(&branch).cloned() {
            None => Outcome::Refused { code: "worktree_not_found".into(), message: "worktree branch not found".into() },
            Some((Some(ws), Some(pane), path)) => {
                Outcome::Ok(Worktree { workspace_id: ws, pane_id: pane, path, branch })
            }
            Some(_) => Outcome::Ok(s.open_worktree(&branch)),
        })
    }

    fn worktree_remove(&mut self, workspace_id: &str) -> Outcome<()> {
        let ws = workspace_id.to_string();
        self.act(Call::WorktreeRemove { workspace: ws.clone() }, move |s| {
            let Some(branch) = s.worktrees.iter().find(|(_, v)| v.0.as_deref() == Some(&ws)).map(|(k, _)| k.clone())
            else {
                return Outcome::Refused { code: "not_found".into(), message: "no workspace".into() };
            };
            if let Some((_, Some(pane), _)) = s.worktrees.remove(&branch) {
                s.panes.remove(&pane);
            }
            Outcome::Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_records_and_respects_blocked_and_injection() {
        let mut h = FakeHerdr::new();
        h.add_agent("w1:p1", "opencode", AgentStatus::Idle);
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
        assert!(h.agent_get(&w.pane_id).unwrap().is_none(), "root pane is an empty shell");
        assert!(matches!(h.worktree_create("/repo", "data/t1-store", "dev", "x"), Outcome::Refused { .. }));
        assert_eq!(h.worktree_open("/repo", "data/t1-store", "x"), Outcome::Ok(w.clone()));
        assert_eq!(h.worktree_remove(&w.workspace_id), Outcome::Ok(()));
        assert!(h.worktree_branches().is_empty());
        assert!(matches!(h.worktree_open("/repo", "data/t1-store", "x"), Outcome::Refused { .. }));
    }

    #[test]
    fn tail_returns_last_lines() {
        let mut h = FakeHerdr::new();
        h.add_agent("w1:p1", "codex", AgentStatus::Idle);
        h.set_screen("w1:p1", "a\nb\nc\nd");
        assert_eq!(h.read_tail("w1:p1", 2).unwrap(), "c\nd");
    }
}
