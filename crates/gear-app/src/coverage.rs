//! The captain's coverage report (T-474).
//!
//! An attempt the captain accepted is the work that should have landed: its
//! candidate must be on the project's integration branch. The report lists the
//! accepted attempts whose candidate is not there, so a task that was accepted
//! but never merged (a lost merge, a branch closed early, a merge that took a
//! different commit) is named before it is forgotten.
//!
//! Whether a commit is on the integration branch is a git question, so the
//! caller hands in the check: the CLI runs git, the tests answer from a table.
//! Read-only: nothing is written.

use crate::config::{self, ProjectConfig, Repo};
use crate::identity::require_seat;
use crate::Env;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, Ts};
use gear_store::Store;

/// An accepted attempt whose candidate is not on the integration branch.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Off {
    pub task: String,
    pub attempt: i64,
    pub candidate: String,
    /// The repo the work belongs to (its path).
    pub repo: String,
    /// The lane the attempt ran on, when the store names one.
    pub lane: Option<String>,
    /// When the attempt last changed (its acceptance is at or before this).
    pub accepted: Ts,
}

/// The coverage report of a project.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Coverage {
    pub project: String,
    /// The integration branch the candidates were checked against.
    pub integration: String,
    /// Accepted attempts with a candidate, checked.
    pub checked: usize,
    /// The ones whose candidate is not on the integration branch.
    pub off: Vec<Off>,
}

/// Whether `sha` is on `integration` in `repo`, fetched from `remote`.
pub type OnIntegration<'a> = dyn Fn(&Repo, &str, &str) -> bool + 'a;

/// Build the coverage report of the caller's project.
///
/// The caller must be the captain of the project (`Forbidden` otherwise).
pub fn build(
    store: &mut Store,
    env: &Env,
    fp: Option<&crate::identity::Fingerprint>,
    on_integration: &OnIntegration<'_>,
) -> GearResult<Coverage> {
    // Read the accepted attempts first; the git checks run after the store is
    // let go (each one may run a command).
    let (project, config, rows): (String, ProjectConfig, Vec<Off>) = store.read(|tx| {
        let (ctx, _seat) = require_seat(
            tx,
            env,
            fp,
            Some(&[SeatRole::Captain]),
            Some(&[SeatState::Active]),
        )?;
        let project = ctx.project.clone();
        let row = tx
            .get_project(&project)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no project {project}")))?;
        let config = config::parse(&row.config_toml)?;
        let mut rows = Vec::new();
        for task in tx.tasks_of_project(&project)? {
            for attempt in tx.attempts_of_task(&project, &task.id)? {
                if attempt.state != gear_model::AttemptState::Accepted {
                    continue;
                }
                let Some(candidate) = attempt.candidate.clone().filter(|c| !c.is_empty()) else {
                    continue;
                };
                rows.push(Off {
                    task: task.id.clone(),
                    attempt: attempt.n,
                    candidate,
                    repo: config.repo_of_task(&task).path,
                    lane: task.lane.clone(),
                    accepted: attempt.updated,
                });
            }
        }
        Ok((project, config, rows))
    })?;
    let remote = config.conventions.remote.clone();
    let mut checked = 0;
    let mut off = Vec::new();
    for row in rows {
        checked += 1;
        let repo = config
            .repos()
            .into_iter()
            .find(|repo| repo.path == row.repo)
            .unwrap_or_else(|| config.root_repo());
        if !on_integration(&repo, &remote, &row.candidate) {
            off.push(row);
        }
    }
    Ok(Coverage {
        project,
        integration: config.conventions.integration,
        checked,
        off,
    })
}

/// The report in words, for the captain.
pub fn render(report: &Coverage) -> String {
    let mut out = format!(
        "Coverage report · project {}\n{} accepted attempt(s) checked against {}",
        report.project, report.checked, report.integration
    );
    if report.off.is_empty() {
        out.push_str(": every candidate is on it.\n");
        return out;
    }
    out.push_str(&format!(
        ": {} not on it — the work was accepted and never landed:\n",
        report.off.len()
    ));
    for row in &report.off {
        out.push_str(&format!(
            "- {} attempt {}: candidate {} (repo {}, lane {}) — accepted at {}\n",
            row.task,
            row.attempt,
            row.candidate,
            row.repo,
            row.lane.as_deref().unwrap_or("?"),
            row.accepted
        ));
    }
    out
}
