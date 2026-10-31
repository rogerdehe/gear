//! Voyage audits (T-505): a task the captain marks `audit: true` is checked by
//! reviewers against the integration branch, not worked by its lane.
//!
//! The captain plans an audit as a queue task whose `deps:` are the tasks it
//! checks (`skills/gear/references/decompose.md` rule 7). When the task is
//! Ready and every dependency is merged, this step hands it in the way a lane
//! would: an attempt `Submitted` at the tip of `<remote>/<integration>` (read
//! after a fetch), the writer being the lane's seat and the hand-in time now.
//! `tasks_to_review` then starts the review of it as it does for any hand-in,
//! and the captain rules the verdict: a pass marks the audit merged at the
//! candidate it checked, a fail puts it back on Ready so the captain can add
//! the fix tasks to its `deps:`.

use gear_app::config::ProjectConfig;
use gear_model::{AttemptState, ErrorCode, GearError, GearResult, SeatRole, TaskState, Ts};
use gear_store::{AttemptRow, Store};

use crate::review::{repo_tips, review_git, RepoTips};
use crate::EngineCfg;

/// What the audit step did in one tick: `project:task/attempt` of every audit
/// handed in, and the seats it could not hand in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditReport {
    /// `project:task/attempt` of the audits handed in for review this tick.
    pub handed_in: Vec<String>,
    /// `project:task: why` of the audits that could not be handed in.
    pub failed: Vec<String>,
}

/// Hand in every audit of `project` whose dependencies are merged (T-505).
///
/// An audit is never assigned to its lane, so this is the only way its attempt
/// exists: one attempt per ready audit, `n` = the last one + 1, `Submitted`
/// with the integration branch's tip as its candidate. The task goes
/// `Verifying`, which is what `tasks_to_review` looks for.
pub fn start_audits(
    store: &mut Store,
    cfg: &EngineCfg,
    tips: &mut RepoTips,
    config: &ProjectConfig,
    project: &str,
    now: Ts,
) -> GearResult<AuditReport> {
    let mut report = AuditReport::default();
    let audits: Vec<String> = store.read(|tx| {
        let mut ids = Vec::new();
        for task in tx.tasks_of_project(project)? {
            if task.state != TaskState::Ready {
                continue;
            }
            if tx.task_audit(project, &task.id)?.is_none() {
                continue;
            }
            ids.push(task.id);
        }
        Ok(ids)
    })?;
    for task in audits {
        match hand_in(store, cfg, tips, config, project, &task, now) {
            Ok(Some(attempt)) => {
                report.handed_in.push(format!("{project}:{task}/{attempt}"));
            }
            Ok(None) => {}
            Err(error) => report.failed.push(format!(
                "{project}:{task}: {}: {}",
                error.code, error.detail
            )),
        }
    }
    Ok(report)
}

/// Hand one audit in; `None` when it is not ready to be (a dependency is not
/// merged, or its lane has no seat).
fn hand_in(
    store: &mut Store,
    cfg: &EngineCfg,
    tips: &mut RepoTips,
    config: &ProjectConfig,
    project: &str,
    task: &str,
    now: Ts,
) -> GearResult<Option<i64>> {
    let (row, lane, seat, last) = store.read(|tx| {
        let row = tx.get_task(project, task)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("no task {task} in {project}"))
        })?;
        // The same merged rule as a dependent task (T-236): Done is set at the
        // review pass, the merge is what makes a task available.
        for dep in &row.deps {
            if gear_app::tasks::merged_at(tx, project, dep)?.is_none() {
                return Ok((row, None, None, Vec::new()));
            }
        }
        let lane = row.lane.clone().unwrap_or_default();
        // The writer is the lane's seat: the audit is read as that lane's work
        // is read, and the seat names the writer in the review packet.
        let seat = tx
            .seats_of_project(project)?
            .into_iter()
            .find(|seat| seat.role == SeatRole::Lane && seat.name == lane);
        let last = tx.attempts_of_task(project, task)?;
        Ok((row, Some(lane), seat, last))
    })?;
    let (Some(lane), Some(seat)) = (lane, seat) else {
        return Ok(None);
    };
    // The candidate is the integration branch's tip in the repo the task reads:
    // fetched now, because an audit checks the branch as it stands.
    let repo = config.repo_of_task(&row);
    let remote = config.conventions.remote.clone();
    let fetched = review_git(cfg.review_git.as_ref(), &repo.path, &["fetch", &remote]);
    if let Err(error) = fetched {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!("fetch {remote} in {}: {error}", repo.path),
        ));
    }
    let candidate = repo_tips(tips, cfg, &repo.path, &remote)
        .get(&repo.integration)
        .map(|(sha, _)| sha.clone());
    let Some(candidate) = candidate else {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{remote}/{} cannot be read in {}, so the audit has no candidate",
                repo.integration, repo.path
            ),
        ));
    };
    let n = last.iter().map(|attempt| attempt.n).max().unwrap_or(0) + 1;
    // The attempt is handed in before the task it belongs to is Verifying, as a
    // lane's hand-in writes them (design §10.3).
    store.write(|tx| {
        tx.insert_attempt(&AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: project.to_string(),
            task: task.to_string(),
            n,
            seat: seat.id.clone(),
            generation: seat.generation,
            contract_version: row.contract_version,
            adopted_version: row.contract_version,
            pending_update: None,
            state: AttemptState::Submitted,
            candidate: Some(candidate.clone()),
            report: None,
            safe_point: None,
            started: now,
            updated: now,
        })?;
        tx.set_attempt_time(project, task, n, gear_store::AttemptStep::HandedIn, now)?;
        let mut task_row = tx
            .get_task(project, task)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no task {task}")))?;
        task_row.state = TaskState::Verifying;
        task_row.updated = now;
        tx.update_task(&task_row)
    })?;
    let _ = lane;
    Ok(Some(n))
}
