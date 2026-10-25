//! The engine merges a passed task (design/08, conventions 17 §8, plan T-12).
//!
//! A task whose current attempt has a passing verdict and that has no merge op
//! yet gets one `merge` op. A background thread executes it in the integration
//! worktree of the task's repo (`home/merge`, created once from that repo's
//! integration branch, reopened afterwards):
//!
//! 1. fetch every branch of origin and reset to `origin/<integration>`: a
//!    half-done earlier run is discarded, and a retry of the same op cannot
//!    merge twice;
//! 2. merge the **approved candidate by commit**, wherever it is (T-61): the
//!    branch the lane named may hold it, may hold more than it, or may be
//!    gone; what merges is the commit the review approved, never what a
//!    branch points at now;
//! 3. merge the review branch with it;
//! 4. refuse a merge that left conflict markers, abort a conflicted merge;
//! 5. run the gate of that repo in that worktree;
//! 6. push `<integration>`, then close the branches the merge consumed.
//!
//! The candidate must be reachable from a pushed ref: work that was never
//! pushed is not merged, and the captain is told why. A failure that can clear
//! on its own — a push race, a gate that could not run, a candidate that is
//! not fetchable yet — is retried a bounded number of times with a growing
//! wait; a failure that stands (a conflict, a gate that said no, a candidate
//! on no pushed ref) ends the op and rings the captain once.
//!
//! A project of several repos merges each of them on its own branch and with
//! its own gate (T-53): the task's repo is its lane's repo, and one worker
//! runs per repo.
//!
//! Any failure before the push leaves the remote integration branch untouched
//! (`git reset --hard origin/<integration>`), records the op Failed with the
//! output tail and puts one item in the captain's inbox. An op the previous
//! engine left `Executing` becomes `Unknown` on start; a merge op in `Unknown`
//! re-runs the same idempotent sequence, so a restart can neither merge twice
//! nor lose the push. One worker per project runs beside the tick; the tick
//! collects its result without waiting. An OS lock in the worktree's git
//! directory excludes overlapping recovery workers. Completion timestamps
//! come from the worker after gate and push, rather than the launching tick.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use gear_app::config::{ProjectConfig, Repo};
use gear_herdr::{HerdrAct, Outcome};
use gear_model::{
    ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, OpState, TaskState, Ts,
};
use gear_store::{AttemptStep, NewMessage, NewOp, OpRow, Store};

use crate::EngineCfg;

/// A merge that went through: the task and the branches it closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub task: String,
    pub branch: String,
}

/// Runtime owned by the engine configuration and shared by its clones. Workers
/// own only git inputs; SQLite and Herdr remain on the tick thread.
#[derive(Debug, Clone, Default)]
pub struct MergeWorkers(Arc<Mutex<BTreeMap<String, Worker>>>);

#[derive(Debug)]
struct Worker {
    op: OpRow,
    finished: Arc<Mutex<Option<Finished>>>,
}

#[derive(Debug, Clone)]
struct Finished {
    // None means another engine still holds this worktree's merge lock.
    result: Result<Option<String>, Failed>,
    at: Ts,
}

fn wall_time() -> Ts {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as Ts
}

/// Queue, launch or collect a merge without waiting for git or the gate.
pub fn merge_passed(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    project: &str,
    now: Ts,
) -> GearResult<Vec<Merged>> {
    let mut merged = Vec::new();
    let Some(row) = store.read(|tx| tx.get_project(project))? else {
        return Ok(merged);
    };
    let config = match gear_app::config::parse(&row.config_toml) {
        Ok(config) => config,
        Err(_) => return Ok(merged),
    };

    // 1. queue a merge for every passed task that has none.
    let passed = passed_tasks(store, project)?;
    for (task, lane, candidate, attempt, attempt_candidate) in passed {
        if merge_op_of(store, project, &task, attempt)?.is_some() {
            continue;
        }
        let branch = branch_of(&lane, &task);
        store.write(|tx| {
            tx.insert_op(&NewOp {
                project: project.to_string(),
                // The merge follows the task's own work: its lane is the seat
                // that asked for it.
                seat: format!("{project}:{lane}"),
                generation: 1,
                kind: OpKind::Merge,
                task: Some(task.clone()),
                attempt: Some(attempt),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({
                    "branch": branch,
                    "candidate": candidate,
                    "attempt_candidate": attempt_candidate,
                    "review": review_branch(&task),
                }),
                created: now,
            })?;
            Ok(())
        })?;
    }

    // A worker survives between ticks. Collect only a completed result;
    // never join a running thread. One worker per repo (T-53): the workers of
    // the repos of a project are collected one after the other.
    let mut workers =
        cfg.merges.0.lock().map_err(|_| {
            GearError::new(ErrorCode::BadTransition, "merge worker registry poisoned")
        })?;
    let finished_keys: Vec<String> = workers
        .iter()
        .filter(|(key, worker)| {
            key.starts_with(&format!("{project}\u{1f}"))
                && worker.finished.lock().unwrap().is_some()
        })
        .map(|(key, _)| key.clone())
        .collect();
    if let Some(key) = finished_keys.first() {
        let worker = workers.get(key).expect("the worker was just found");
        let finished = worker.finished.lock().unwrap().clone();
        let Some(finished) = finished else {
            return Ok(merged);
        };
        let op = &worker.op;
        match finished.result {
            Ok(Some(lines)) => {
                store.write(|tx| {
                    let current = tx.get_op(project, &op.id)?.unwrap();
                    if current.state == OpState::Executing {
                        tx.set_op_state(project, &op.id, OpState::Submitted, None, finished.at)?;
                    }
                    tx.set_op_state(project, &op.id, OpState::Confirmed, Some(&lines), finished.at)?;
                    // T-38's merge time belongs to gate/push completion,
                    // independent of both the launching and collecting tick.
                    if let (Some(attempt), Some(task)) = (op.attempt, op.task.as_deref()) {
                        tx.set_attempt_time(project, task, attempt, AttemptStep::Merged, finished.at)?;
                    }
                    Ok(())
                })?;
                merged.push(Merged {
                    task: op.task.clone().unwrap_or_default(),
                    branch: op.payload["branch"].as_str().unwrap_or("").to_string(),
                });
            }
            Ok(None) => retry_merge(store, project, op, finished.at)?,
            // The worker already retried a reason that can clear (T-61): what
            // comes back is the failure that stands.
            Err(failed) => fail_merge(store, project, op, &failed.reason, finished.at)?,
        }
        // Keep the completion until its database write succeeded: a transient
        // store error must not lose the result or launch a second worker.
        workers.remove(key);
        return Ok(merged);
    }

    // One worker per repo. Unknown ops are restart recovery: merge_one
    // resets to origin before doing any merges, and adopts a completed push.
    for op in ready_merges(store, project)? {
        let task = op.task.clone().unwrap_or_default();
        let valid_task = store.read(|tx| {
            Ok(tx
                .get_task(project, &task)?
                .is_some_and(|row| matches!(row.state, TaskState::Verifying | TaskState::Done)))
        })?;
        if !valid_task || still_current(store, project, &op)?.is_err() {
            store.write(|tx| neutrino_void(tx, project, &op.id, now))?;
            continue;
        }
        // The repo the work is in: the repo of the task's lane (T-53).
        let repo = repo_of_op(store, project, &op, &config)?;
        let key = worker_key(project, &repo);
        // A merge of this repo is already running: this one waits for it.
        if workers.contains_key(&key) {
            continue;
        }
        let worktree = match integration_worktree(herdr, &repo, project) {
            Ok(worktree) => worktree,
            Err(reason) => {
                tell_captain(store, project, None, &format!("merge waits: {reason}"), now)?;
                return Ok(merged);
            }
        };
        if op.state == OpState::Intended {
            store.write(|tx| tx.set_op_state(project, &op.id, OpState::Executing, None, now))?;
        }
        let finished = Arc::new(Mutex::new(None));
        let output = finished.clone();
        let job = op.clone();
        let repo = repo.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("merge-{project}-{}", repo.name))
            .spawn(move || {
                let result = std::panic::catch_unwind(|| {
                    let Some(lock) = merge_lock(Path::new(&worktree.path))
                        .map_err(|reason| Failed::clear(reason))?
                    else {
                        return Ok(None);
                    };
                    merge_with_retries(&worktree, &repo, &job, &lock).map(Some)
                })
                .unwrap_or_else(|_| Err(Failed::stuck("merge worker panicked")));
                *output.lock().unwrap() = Some(Finished {
                    result,
                    at: wall_time(),
                });
            });
        match spawned {
            Ok(_) => {
                workers.insert(key, Worker { op, finished });
            }
            Err(error) => fail_merge(
                store,
                project,
                &op,
                &format!("start merge worker: {error}"),
                wall_time(),
            )?,
        }
    }
    Ok(merged)
}

/// A lock in the worktree's git directory also excludes another runtime or a
/// recovering engine. OS locks are released on process death; stale files are
/// harmless. Never wait for a lock on the tick thread.
fn merge_lock(dir: &Path) -> Result<Option<File>, String> {
    let path = git(dir, &["rev-parse", "--git-path", "gear-merge.lock"])?;
    let path = dir.join(path.trim());
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("merge lock {}: {error}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(format!("merge lock: {error}")),
    }
}

fn retry_merge(store: &mut Store, project: &str, op: &OpRow, now: Ts) -> GearResult<()> {
    store.write(|tx| {
        if tx
            .get_op(project, &op.id)?
            .is_some_and(|row| row.state == OpState::Executing)
        {
            tx.set_op_state(
                project,
                &op.id,
                OpState::Unknown,
                Some("merge will retry from origin"),
                now,
            )?;
        }
        Ok(())
    })
}

/// A push rejected because the integration branch moved meanwhile.
const RACE: &str = "push race: ";
/// How many times a merge that failed for a reason that can clear is retried
/// before it fails (T-61).
const MERGE_RETRIES: u32 = 3;
/// The wait before the first retry. It doubles after every attempt.
const RETRY_BACKOFF_S: u64 = 2;
/// The longest wait between two attempts.
const RETRY_BACKOFF_MAX_S: u64 = 30;

/// Why a merge did not go through: a reason that can clear on its own, or one
/// that needs the captain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Blocker {
    /// The reason can clear (a push race, a gate that could not run, a
    /// candidate that is not fetchable yet): the merge is retried.
    Clear,
    /// The reason stands (a conflict, a gate that said no, a candidate on no
    /// pushed ref): the captain is rung, once.
    Stuck,
}

/// A merge that did not go through: the reason, and whether it can clear.
#[derive(Debug, Clone)]
struct Failed {
    reason: String,
    blocker: Blocker,
}

impl Failed {
    /// A reason that can clear on its own: the merge is retried.
    fn clear(reason: impl Into<String>) -> Failed {
        Failed { reason: reason.into(), blocker: Blocker::Clear }
    }

    /// A reason that stands: the captain is rung.
    fn stuck(reason: impl Into<String>) -> Failed {
        Failed { reason: reason.into(), blocker: Blocker::Stuck }
    }
}

/// Run the merge and retry it while the reason can clear. The wait grows after
/// every attempt; after the last one the failure stands and the captain hears
/// about it once (T-61).
fn merge_with_retries(
    worktree: &gear_herdr::Worktree,
    repo: &Repo,
    op: &OpRow,
    lock: &File,
) -> Result<String, Failed> {
    let mut attempt = 0;
    loop {
        match merge_one_locked(worktree, repo, op, lock) {
            Ok(lines) => return Ok(lines),
            Err(mut failed) => {
                if failed.blocker != Blocker::Clear || attempt >= MERGE_RETRIES {
                    if attempt > 0 {
                        failed.reason = format!(
                            "{}\n(retried {attempt} time(s), each with a growing wait)",
                            failed.reason
                        );
                    }
                    return Err(failed);
                }
                attempt += 1;
                std::thread::sleep(backoff(attempt));
            }
        }
    }
}

/// The wait before the next attempt: it doubles after every attempt.
fn backoff(attempt: u32) -> std::time::Duration {
    let shift = attempt.saturating_sub(1).min(6);
    let seconds = RETRY_BACKOFF_S.saturating_mul(1u64 << shift);
    std::time::Duration::from_secs(seconds.min(RETRY_BACKOFF_MAX_S))
}

/// Fail a merge op once and tell the captain once.
fn fail_merge(store: &mut Store, project: &str, op: &OpRow, reason: &str, now: Ts) -> GearResult<()> {
    store.write(|tx| {
        let from = tx.get_op(project, &op.id)?.map(|op| op.state).unwrap_or(OpState::Intended);
        if from == OpState::Intended {
            tx.set_op_state(project, &op.id, OpState::Executing, None, now)?;
        }
        // Unknown from a restart goes straight to Failed.
        tx.set_op_state(project, &op.id, OpState::Failed, Some(reason), now)
    })?;
    let task = op.task.clone().unwrap_or_default();
    // The item names the task and attempt it is about, so the captain's home
    // page can suggest the verdict that settles it (T-90).
    let about = op.task.as_deref().zip(op.attempt);
    tell_captain(
        store,
        project,
        about,
        &format!("merge of {task} failed: {reason}\nnothing was pushed"),
        now,
    )
}

/// Whether the op still merges what was passed: its attempt is the task's
/// latest and its candidate is the attempt's candidate.
fn still_current(store: &mut Store, project: &str, op: &OpRow) -> GearResult<Result<(), String>> {
    store.read(|tx| {
        let Some(task) = op.task.as_deref() else {
            return Ok(Err("the op names no task".to_string()));
        };
        let Some(latest) = tx.attempts_of_task(project, task)?.into_iter().last() else {
            return Ok(Err("the task has no attempt".to_string()));
        };
        if op.attempt != Some(latest.n) {
            return Ok(Err(format!(
                "a newer attempt {} was handed in",
                latest.n
            )));
        }
        let candidate = op.payload["candidate"].as_str().unwrap_or("");
        if latest.candidate.as_deref() != Some(candidate) {
            return Ok(Err("the attempt's candidate changed".to_string()));
        }
        Ok(Ok(()))
    })
}

/// Void a queued merge because its task may not be merged any more.
fn neutrino_void(tx: &gear_store::Tx<'_>, project: &str, id: &str, now: Ts) -> GearResult<()> {
    let from = tx.get_op(project, id)?.map(|op| op.state).unwrap_or(OpState::Intended);
    match from {
        OpState::Intended => tx.set_op_state(project, id, OpState::Void, Some("the task may not be merged"), now)?,
        OpState::Unknown => tx.set_op_state(project, id, OpState::Failed, Some("the task may not be merged"), now)?,
        _ => {}
    }
    Ok(())
}

/// The tasks of the project whose **current** attempt has a passing verdict:
/// (task, lane, candidate, attempt, the attempt's candidate).
fn passed_tasks(store: &mut Store, project: &str) -> GearResult<Vec<(String, String, String, i64, String)>> {
    store.read(|tx| {
        let mut passed = Vec::new();
        for task in tx.tasks_of_project(project)? {
            // A cancelled task is not merged, whatever its verdict says.
            if !matches!(task.state, TaskState::Verifying | TaskState::Done) {
                continue;
            }
            let Some(lane) = task.lane.clone() else {
                continue;
            };
            // Only the latest attempt counts: an older pass does not authorize
            // a newer attempt that was never reviewed.
            let Some(latest) = tx.attempts_of_task(project, &task.id)?.into_iter().last() else {
                continue;
            };
            let Some(verdict) = tx
                .verdicts_of_task(project, &task.id)?
                .into_iter()
                .rev()
                .find(|verdict| {
                    verdict.passed
                        && verdict.attempt == latest.n
                        && verdict.contract_version == task.contract_version
                        && Some(&verdict.candidate) == latest.candidate.as_ref()
                })
            else {
                continue;
            };
            passed.push((task.id.clone(), lane, verdict.candidate, verdict.attempt, latest.candidate.clone().unwrap_or_default()));
        }
        Ok(passed)
    })
}

/// The branch a lane cut for a task (`<lane>/t<n>`, 17 §3).
fn branch_of(lane: &str, task: &str) -> String {
    format!("{lane}/{}", task.to_lowercase().replace('-', ""))
}

/// The reviewers' branch of a task: `review/t<n>`.
fn review_branch(task: &str) -> String {
    format!("review/{}", task.to_lowercase().replace('-', ""))
}

/// The merge op of a task, if one is queued, running or done (a Failed or
/// Void one does not block a retry).
fn merge_op_of(store: &mut Store, project: &str, task: &str, attempt: i64) -> GearResult<Option<OpRow>> {
    // Every state counts, Failed and Void too: a merge that failed waits for
    // the captain (one item) and is never re-queued tick after tick — a new
    // passing attempt gets its own merge.
    store.read(|tx| {
        for state in [
            OpState::Intended,
            OpState::Executing,
            OpState::Submitted,
            OpState::Unknown,
            OpState::Confirmed,
            OpState::Failed,
            OpState::Void,
        ] {
            if let Some(op) = tx.ops_in_state(project, state)?.into_iter().find(|op| {
                op.kind == OpKind::Merge && op.task.as_deref() == Some(task) && op.attempt == Some(attempt)
            }) {
                return Ok(Some(op));
            }
        }
        Ok(None)
    })
}

/// The merge ops that may run now: Intended, or Unknown after a restart.
fn ready_merges(store: &mut Store, project: &str) -> GearResult<Vec<OpRow>> {
    store.read(|tx| {
        let mut ready = Vec::new();
        for state in [OpState::Intended, OpState::Unknown] {
            for op in tx.ops_in_state(project, state)? {
                if op.kind == OpKind::Merge {
                    ready.push(op);
                }
            }
        }
        Ok(ready)
    })
}

/// The engine's integration worktree of one repo: `home/merge`, created once
/// from that repo's integration branch and reopened after that (17 §4:
/// worktrees are Herdr's).
///
/// One worktree per repo (T-53): a project of several repos merges each of
/// them in its own worktree, on its own integration branch, with its own gate.
fn integration_worktree(
    herdr: &mut dyn HerdrAct,
    repo: &Repo,
    project: &str,
) -> Result<gear_herdr::Worktree, String> {
    let label = merge_label(project, repo);
    match herdr.worktree_open(&repo.path, "home/merge", &label) {
        Outcome::Ok(worktree) => Ok(worktree),
        Outcome::Refused { code, .. } if code == "worktree_not_found" => {
            match herdr.worktree_create(
                &repo.path,
                "home/merge",
                &repo.integration,
                &label,
            ) {
                Outcome::Ok(worktree) => Ok(worktree),
                other => Err(step_failure("worktree_create", &other)),
            }
        }
        other => Err(step_failure("worktree_open", &other)),
    }
}

/// The label of a repo's integration worktree: `<project>-merge` for the
/// project root, `<project>-merge-<repo name>` for a child repo (T-53).
fn merge_label(project: &str, repo: &Repo) -> String {
    if repo.name.is_empty() {
        format!("{project}-merge")
    } else {
        format!("{project}-merge-{}", repo.name)
    }
}

/// The repo a merge op works in: the repo of the task's lane (T-53). A task
/// without a lane merges in the project root.
fn repo_of_op(
    store: &mut Store,
    project: &str,
    op: &OpRow,
    config: &ProjectConfig,
) -> GearResult<Repo> {
    let mut lane = None;
    if let Some(task) = op.task.as_deref() {
        lane = store.read(|tx| Ok(tx.get_task(project, task)?.and_then(|row| row.lane.clone())))?;
    }
    Ok(config.repo_of_lane(lane.as_deref().unwrap_or_default()))
}

/// The key of a merge worker: one worker per repo (T-53), so a merge in one
/// repo does not wait for the merge of another.
fn worker_key(project: &str, repo: &Repo) -> String {
    format!("{project}\u{1f}{}", repo.name)
}

/// Merge one op's task branches and run the gate, once. `Ok(lines)` = merged
/// and pushed; `Err(failed)` = nothing was pushed and the worktree is clean
/// again.
#[cfg(test)]
fn merge_one(
    worktree: &gear_herdr::Worktree,
    repo: &Repo,
    op: &OpRow,
) -> Result<String, Failed> {
    let lock = merge_lock(Path::new(&worktree.path))
        .map_err(|reason| Failed::stuck(reason))?
        .ok_or_else(|| Failed::stuck("merge worktree is busy"))?;
    merge_one_locked(worktree, repo, op, &lock)
}

fn merge_one_locked(
    worktree: &gear_herdr::Worktree,
    repo: &Repo,
    op: &OpRow,
    lock: &File,
) -> Result<String, Failed> {
    let git = |dir: &Path, args: &[&str]| git_with_lock(dir, args, Some(lock));
    let dir = Path::new(&worktree.path);
    let integration = repo.integration.clone();
    let branch = op.payload["branch"].as_str().unwrap_or_default().to_string();
    // The op may come from `tasks::settle_review`, which names the branch and
    // the candidate but not the review branch: derive it, and merge it only
    // when it exists (F5 of the T-12 review).
    let task = op.task.clone().unwrap_or_default();
    let review = op
        .payload
        .get("review")
        .and_then(|review| review.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| review_branch(&task));

    // 1. a clean base: the remote integration branch, whatever an interrupted
    //    earlier run left behind.
    //    The fetch names every branch of origin: the approved candidate may
    //    sit on any of them (T-61), and a clone that restricts its refspec
    //    would not bring it in. A fetch that fails may clear (the network,
    //    the remote): it is retried.
    git(dir, &["fetch", "--prune", "origin", "+refs/heads/*:refs/remotes/origin/*"])
        .map_err(|reason| Failed::clear(format!("fetch origin: {reason}")))?;
    git(dir, &["reset", "--hard", &format!("origin/{integration}")])
        .map_err(|reason| Failed::stuck(format!("reset to origin/{integration}: {reason}")))?;

    // 2. the approved candidate, by commit. P0: what merges is the commit the
    //    review approved, never what a branch points at now. The branch the
    //    lane named may hold it, hold more than it, or be gone (T-61).
    let candidate = op.payload["candidate"].as_str().unwrap_or("").trim().to_string();
    if candidate.is_empty() {
        return Err(Failed::stuck(format!("{task}: the merge op names no candidate")));
    }
    // The commit must be known here. It is not yet: the lane's push has not
    // landed (or has not been fetched). That can clear, so it is retried.
    let sha = match git(dir, &["rev-parse", "--verify", &format!("{candidate}^{{commit}}")]) {
        Ok(out) => out.trim().to_string(),
        Err(reason) => {
            return Err(Failed::clear(format!(
                "{task}: the approved candidate {candidate} is not fetchable from origin yet\n{reason}\n\
                 it may not be pushed yet: the merge retries and asks the captain if it stays"
            )))
        }
    };

    // 3. the candidate must be reachable from a pushed ref: work that was
    //    never pushed (or whose branch is gone) is not merged, and the
    //    captain is told why. The branch may have any name: the task's, a
    //    rework's (`engine/t53-v2`), or another one.
    let pushed = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            &format!("--contains={sha}"),
            "refs/remotes/origin/",
        ],
    )
    .map_err(|reason| Failed::clear(format!("cannot read the pushed refs of origin: {reason}")))?;
    if pushed.trim().is_empty() {
        // The candidate may not be pushed yet: the lane pushes it and the
        // merge retries. Only the bounded retry decides whether it was never
        // pushed (F2 of the T-61 review).
        return Err(Failed::clear(format!(
            "{task}: the approved candidate {sha} is on no pushed ref of origin\n\
             it was never pushed, or the branch that held it is gone"
        )));
    }

    // 4. the candidate may already be on the integration branch: a restart
    //    after the push, or another task that carried it in.
    let already = git(
        dir,
        &["merge-base", "--is-ancestor", &sha, &format!("origin/{integration}")],
    )
    .is_ok();

    // 5. the merges, in order (the approved candidate, then the review
    //    branch); the candidate is pinned by sha, so a branch that moves later
    //    cannot slip an unreviewed commit in.
    //    The review branch is *required*: a merge that publishes the task
    //    without its review is a partial merge (F6 of the T-12 review). The
    //    integration branch is not touched: the merge fails before it, so
    //    dev stays where it was. A candidate that is already merged needs no
    //    merge commit; its review branch still merges with it.
    let review_ref = format!("origin/{review}");
    let review_found = git(dir, &["rev-parse", "--verify", &review_ref]);
    let has_review = review_found.is_ok();
    if !has_review && !already {
        let reason = review_found.unwrap_err();
        return Err(Failed::stuck(format!(
            "{task}: the review branch {review_ref} is missing on origin\n{reason}\n\
             a merge publishes the approved candidate and its review branch together"
        )));
    }
    let mut pinned_refs = Vec::new();
    if !already {
        pinned_refs.push(sha.clone());
    }
    if has_review {
        pinned_refs.push(review_ref.clone());
    }
    if pinned_refs.is_empty() {
        // The candidate is on the integration branch and its review branch is
        // gone: a restart after the push. Nothing to merge, nothing to push.
        return Ok(format!(
            "{task} was already merged: {sha} is on origin/{integration}"
        ));
    }
    for pinned in &pinned_refs {
        if let Err(reason) = git(
            dir,
            &[
                "merge",
                "--no-ff",
                pinned,
                "-m",
                &format!("merge({task}): {pinned}"),
            ],
        ) {
            // Abort and leave the worktree exactly at the integration branch.
            let _ = git(dir, &["merge", "--abort"]);
            let _ = git(dir, &["reset", "--hard", &format!("origin/{integration}")]);
            return Err(Failed::stuck(format!("conflict merging {pinned}: {reason}")));
        }
    }

    // 6. conflict markers that a merge left behind without reporting a
    //    conflict (a file committed with markers in it).
    if git(dir, &["grep", "-n", "-e", "^<<<<<<<", "-e", "^>>>>>>>"]).is_ok() {
        let _ = git(dir, &["reset", "--hard", &format!("origin/{integration}")]);
        return Err(Failed::stuck("conflict markers in the merged tree"));
    }

    // 7. the gate, before anything is pushed: the gate of the repo the work
    //    is in (T-53).
    let gate = repo.gate.clone();
    // The empty lock file is inherited as stdin (normally /dev/null for a
    // gate). Keeping the same open file description in the subprocess means
    // it still excludes recovery if the engine is killed before wait returns.
    let output = gate_command(&gate, dir)
        .stdin(
            lock.try_clone()
                .map_err(|error| Failed::clear(format!("gate lock: {error}")))?,
        )
        .output()
        // The gate could not run at all: the environment is broken, not the
        // work. That can clear (a toolchain coming back), so it is retried.
        .map_err(|error| Failed::clear(format!("gate could not run ({gate}): {error}")))?;
    if !output.status.success() {
        // Both streams: cargo reports test failures on stdout, a shell's own
        // errors on stderr.
        let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
        let tail = tail(&combined, 12);
        let _ = git(dir, &["reset", "--hard", &format!("origin/{integration}")]);
        // 126 and 127 are the shell's own "cannot run that" (126: not
        // executable, 127: not found): the toolchain is missing or broken,
        // not the work. That can clear, so it is retried; every other exit
        // status is the project's own gate saying no.
        let status = output.status.code().unwrap_or(0);
        let reason = format!("gate failed ({gate}):\n{tail}");
        return Err(if status == 126 || status == 127 {
            Failed::clear(reason)
        } else {
            Failed::stuck(reason)
        });
    }

    // 8. push, then close the branches the merge consumed.
    // A push the remote rejects because the integration branch moved (the
    // captain or another merge pushed meanwhile) is a race, not a failure: the
    // merge starts again from the new tip.
    if let Err(reason) = git(dir, &["push", "origin", &format!("HEAD:{integration}")]) {
        if reason.contains("non-fast-forward") || reason.contains("fetch first") || reason.contains("cannot lock ref") {
            return Err(Failed::clear(format!("{RACE}{reason}")));
        }
        // Any other push failure can clear too: the network, the remote, a
        // hook that rejects the push once (F1 of the T-61 review).
        return Err(Failed::clear(format!("push failed: {reason}")));
    }
    let (closed, left) = close_branches(dir, repo, &branch, &review, &sha, &task);

    let head = git(dir, &["log", "-1", "--oneline"]).unwrap_or_default();
    let mut line = format!("merged {task} into {integration}: {head}");
    if !closed.is_empty() {
        line.push_str(&format!("\nclosed on origin: {}", closed.join(", ")));
    }
    if !left.is_empty() {
        line.push_str(&format!(
            "\nleft on origin (they still hold the approved commit): {}",
            left.join(", ")
        ));
    }
    Ok(line)
}

/// Close the branches the merge consumed: the review branch, and every branch
/// of this task whose tip is the approved candidate (the task's own branch, or
/// the branch a rework round used, T-61). A branch that holds more than the
/// candidate keeps its commits: it is not closed, and the captain sees it.
///
/// The integration and release branches are never closed, whatever they hold.
/// A branch that is not this task's (a human's branch) is left alone too.
///
/// Returns the branches that were closed and the ones that were left.
fn close_branches(
    dir: &Path,
    repo: &Repo,
    task_branch: &str,
    review: &str,
    candidate: &str,
    task: &str,
) -> (Vec<String>, Vec<String>) {
    let mut closed = Vec::new();
    let mut left = Vec::new();
    // The review branch: it holds the review of this task and nothing else.
    if remote_branch_exists(dir, review) && delete_remote(dir, review) {
        closed.push(review.to_string());
    }
    // The branches whose tip is the candidate: the work the review approved.
    let points = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            &format!("--points-at={candidate}"),
            "refs/remotes/origin/",
        ],
    )
    .unwrap_or_default();
    for line in points.lines() {
        let name = line.trim().trim_start_matches("origin/").to_string();
        if name.is_empty() || name == "HEAD" {
            continue;
        }
        if name == repo.integration || name == repo.release {
            continue;
        }
        if name != task_branch && !is_task_branch(&name, task) {
            // Not this task's branch: it stays, and the captain is told.
            left.push(name);
            continue;
        }
        if delete_remote(dir, &name) && !closed.contains(&name) {
            closed.push(name);
        }
    }
    (closed, left)
}

/// A branch of a task: its last segment is the task's branch name (`t<n>`) or
/// that name with a suffix (`t53-v2`, a rework round).
fn is_task_branch(branch: &str, task: &str) -> bool {
    let base = task.to_lowercase().replace('-', "");
    let segment = branch.rsplit('/').next().unwrap_or(branch);
    segment == base || segment.starts_with(&format!("{base}-"))
}

/// Whether origin still has the branch.
fn remote_branch_exists(dir: &Path, branch: &str) -> bool {
    git(dir, &["rev-parse", "--verify", &format!("origin/{branch}")]).is_ok()
}

/// Close a branch on origin. A branch that is already gone is closed too.
fn delete_remote(dir: &Path, branch: &str) -> bool {
    git(dir, &["push", "origin", "--delete", branch]).is_ok()
}

/// The gate of the project: its build and test command, in `dir`.
///
/// The gate runs without `GEAR_DB` and without `GEAR_CTX`: it is the
/// project's own build and test command, and it must never see the store of
/// this machine (L-59). A test that inherited `GEAR_DB` would write to the
/// production store; the tests give themselves a scratch home instead.
fn gate_command(gate: &str, dir: &Path) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(gate)
        .current_dir(dir)
        .env_remove(gear_model::ctx::ENV_DB)
        .env_remove(gear_model::ctx::ENV_CTX);
    cmd
}

/// Run one git command in `dir`; the output tail comes back as the error.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_with_lock(dir, args, None)
}

fn git_with_lock(dir: &Path, args: &[&str], lock: Option<&File>) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir);
    if let Some(lock) = lock {
        command.stdin(lock.try_clone().map_err(|error| format!("git lock: {error}"))?);
    }
    let output = command.output()
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            tail(&format!("{stdout}{stderr}"), 12)
        ))
    }
}

/// The last `lines` lines of a command's output.
fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

/// Why a Herdr step did not succeed.
fn step_failure<T>(step: &str, outcome: &Outcome<T>) -> String {
    match outcome {
        Outcome::Refused { code, message } => format!("{step}: refused ({code}): {message}"),
        Outcome::Uncertain { reason } => format!("{step}: uncertain: {reason}"),
        Outcome::Ok(_) => format!("{step}: succeeded"),
    }
}

/// One open item in the captain's inbox. `task` is the task and attempt the
/// item is about, when it is about one (T-90: the home page suggests the
/// verdict that settles such an item).
fn tell_captain(
    store: &mut Store,
    project: &str,
    task: Option<(&str, i64)>,
    body: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        let captain = tx.captain_of(project)?.ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!("project {project} has no captain"),
            )
        })?;
        tx.insert_message(&NewMessage {
            project: project.to_string(),
            thread: format!("merge:{}", now),
            task: task.map(|(task, _)| task.to_string()),
            attempt: task.map(|(_, attempt)| attempt),
            sender: captain.id.clone(),
            sender_gen: 1,
            recipient: captain.id,
            kind: MsgKind::Question,
            urgent: false,
            body: body.to_string(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// A scratch repo with a bare origin: `dev` plus one commit each on
    /// `heat/t5` and `review/t5`.
    struct Scratch {
        root: PathBuf,
        work: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let root = std::env::temp_dir().join(format!(
                "gear-merge-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            let origin = root.join("origin.git");
            let work = root.join("work");
            sh(&root, &format!("git init -q --bare {}", origin.display()));
            sh(&root, &format!("git init -q -b dev {}", work.display()));
            fs::write(work.join("README.md"), "dev\n").unwrap();
            sh(&work, "git add -A && git -c user.email=a@b -c user.name=a commit -qm dev");
            sh(&work, "git remote add origin ../origin.git && git push -q origin dev");
            sh(
                &work,
                "git switch -qc heat/t5 && echo task >> README.md && git add -A \
                 && git -c user.email=a@b -c user.name=a commit -qm task && git push -q origin heat/t5",
            );
            sh(
                &work,
                "git switch -qc review/t5 && echo review > review.txt && git add -A \
                 && git -c user.email=a@b -c user.name=a commit -qm review && git push -q origin review/t5",
            );
            Scratch { root, work }
        }

        /// A worktree of this repo for the merge to run in (on `home/merge`).
        fn merge_worktree(&self) -> gear_herdr::Worktree {
            let path = self.root.join("merge");
            sh(
                &self.root,
                &format!(
                    "git -C {} worktree add -q -b home/merge {} origin/dev",
                    self.work.display(),
                    path.display()
                ),
            );
            gear_herdr::Worktree {
                workspace_id: "wt-merge".into(),
                pane_id: "wt-merge:p1".into(),
                path: path.display().to_string(),
                branch: "home/merge".into(),
            }
        }

        /// The head commit of a branch on origin.
        fn head_of(&self, branch: &str) -> String {
            sh(&self.root, &format!("git -C origin.git rev-parse {branch}"))
        }

        /// What origin's integration branch holds.
        fn dev_log(&self) -> String {
            sh(&self.root, "git -C origin.git log --format=%s dev")
        }

        /// The branches origin still has.
        fn origin_branches(&self) -> String {
            sh(&self.root, "git -C origin.git for-each-ref --format='%(refname:short)' refs/heads")
        }

        /// `true` when the task branch's commit is on origin's dev.
        fn dev_has_task_commit(&self) -> bool {
            sh_ok(&self.root, "git -C origin.git log --format=%s dev | grep -qx task")
        }

        /// The head of a branch in the working clone (a branch that was never
        /// pushed has no head on origin).
        fn local_head_of(&self, branch: &str) -> String {
            sh(&self.work, &format!("git rev-parse {branch}"))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// A child repo below the project root (T-53): its own origin, its own
    /// integration branch, a task branch and a review branch.
    struct Child {
        root: PathBuf,
        work: PathBuf,
        origin: PathBuf,
    }

    impl Child {
        fn new(root: &Path, name: &str, integration: &str) -> Child {
            let origin = root.join(format!("{name}-origin.git"));
            let work = root.join(name);
            sh(root, &format!("git init -q --bare {}", origin.display()));
            sh(root, &format!("git init -q -b {integration} {}", work.display()));
            fs::write(work.join("README.md"), "dev\n").unwrap();
            sh(&work, "git add -A && git -c user.email=a@b -c user.name=a commit -qm dev");
            sh(
                &work,
                &format!("git remote add origin ../{name}-origin.git && git push -q origin {integration}"),
            );
            sh(
                &work,
                "git switch -qc heat/t5 && echo task >> README.md && git add -A \
                 && git -c user.email=a@b -c user.name=a commit -qm task && git push -q origin heat/t5",
            );
            sh(
                &work,
                "git switch -qc review/t5 && echo review > review.txt && git add -A \
                 && git -c user.email=a@b -c user.name=a commit -qm review && git push -q origin review/t5",
            );
            Child { root: root.to_path_buf(), work, origin }
        }

        /// The merge worktree of this repo: `home/merge`, cut from its
        /// integration branch.
        fn merge_worktree(&self, integration: &str) -> gear_herdr::Worktree {
            let path = self.root.join("web-merge");
            sh(
                &self.root,
                &format!(
                    "git -C {} worktree add -q -b home/merge {} origin/{integration}",
                    self.work.display(),
                    path.display()
                ),
            );
            gear_herdr::Worktree {
                workspace_id: "wt-merge-web".into(),
                pane_id: "wt-merge-web:p1".into(),
                path: path.display().to_string(),
                branch: "home/merge".into(),
            }
        }

        /// The head of a branch on this repo's origin.
        fn head_of(&self, branch: &str) -> String {
            sh(&self.root, &format!("git -C {} rev-parse {branch}", self.origin.display()))
        }

        /// The commits on a branch of this repo's origin.
        fn log_of(&self, branch: &str) -> String {
            sh(&self.root, &format!("git -C {} log --format=%s {branch}", self.origin.display()))
        }
    }

    fn sh(dir: &Path, command: &str) -> String {
        let output = Command::new("sh").arg("-c").arg(command).current_dir(dir).output().unwrap();
        assert!(
            output.status.success(),
            "{command}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn sh_ok(dir: &Path, command: &str) -> bool {
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// The project config text with the given gate.
    fn config_text(gate: &str) -> String {
        format!(
            r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[conventions]
gate = "{gate}"
"#
        )
    }

    /// The repo the merge runs in: the root repo of the test project, with
    /// the given gate.
    fn repo(gate: &str) -> Repo {
        gear_app::config::parse(&config_text(gate))
            .unwrap()
            .repo_of_lane("heat")
    }

    fn op(candidate: &str) -> OpRow {
        OpRow {
            project: "p".into(),
            id: "O-1".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: OpKind::Merge,
            task: Some("T-5".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({
                "branch": "heat/t5",
                "candidate": candidate,
                "review": "review/t5",
            }),
            state: OpState::Intended,
            result: None,
            check_budget: 0,
            created: 0,
            updated: 0,
        }
    }

    /// A project with one lane, one passed task (Verifying, a passing verdict)
    /// and no merge op yet.
    fn world(task_state: TaskState) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&gear_store::ProjectRow {
                    id: "p".into(),
                    herdr_session: "sess".into(),
                    config_toml: config_text("true"),
                    config_version: 1,
                    quota_lanes: 1,
                    quota_temps: 0,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: 0,
                })?;
                gear_store::testing::seed_active_seat(tx, "p", "heat", gear_model::SeatRole::Lane, "w1:p1", "opencode", 0)?;
                gear_store::testing::seed_active_seat(tx, "p", "captain", gear_model::SeatRole::Captain, "w9:p1", "claude", 0)?;
                tx.insert_task(&gear_store::TaskRow {
                    project: "p".into(),
                    id: "T-5".into(),
                    rank: 1,
                    state: task_state,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                     issue: None,updated: 0,
                })?;
                tx.insert_verdict(&gear_store::VerdictRow {
                    project: "p".into(),
                    task: "T-5".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: "abc1234".into(),
                    passed: true,
                    evidence: Some("ops/reviews/T-5.md".into()),
                    by_seat: "p:captain".into(),
                    created: 0,
                })?;
                Ok(())
            })
            .unwrap();
        store
    }

    #[test]
    fn a_cancelled_task_voids_its_queued_merge() {
        // The task was cancelled after the merge was queued: the op is void and
        // nothing is merged (T-12 adversarial case).
        let mut store = world(TaskState::Verifying);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let mut cfg = EngineCfg::default();
        cfg.profiles.insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
        // The op was queued in an earlier tick (a crash before the execution) …
        store
            .write(|tx| {
                tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    kind: OpKind::Merge,
                    task: Some("T-5".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({
                        "branch": "heat/t5",
                        "candidate": "abc1234",
                        "review": "review/t5",
                    }),
                    created: 10,
                })?;
                Ok(())
            })
            .unwrap();
        // … then the task is cancelled before the tick runs it.
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.state = TaskState::Cancelled;
                tx.update_task(&task)
            })
            .unwrap();

        let merged = merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();

        assert!(merged.is_empty());
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Void, "{op:?}");
    }

    fn queued_merge(
        scratch: &Scratch,
        gate: &str,
    ) -> (Store, gear_herdr::fake::FakeHerdr, EngineCfg) {
        let candidate = scratch.head_of("heat/t5");
        queued_merge_candidate(scratch, gate, &candidate)
    }

    /// A project with a handed-in task and a queued merge op for it, whose
    /// candidate is the given commit (T-61: it may sit on any pushed ref).
    fn queued_merge_candidate(
        scratch: &Scratch,
        gate: &str,
        candidate: &str,
    ) -> (Store, gear_herdr::fake::FakeHerdr, EngineCfg) {
        let worktree = scratch.merge_worktree();
        let candidate = candidate.to_string();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("home/merge", &worktree.path, &worktree.pane_id);
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text(gate);
                tx.update_project(&project)?;
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-5", 1, 1, &candidate, "report", 0)?;
                tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    kind: OpKind::Merge,
                    task: Some("T-5".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({ "branch": "heat/t5", "candidate": candidate }),
                    created: 10,
                })?;
                Ok(())
            })
            .unwrap();
        let mut cfg = EngineCfg::default();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }
        (store, herdr, cfg)
    }

    fn wait_merge(
        store: &mut Store,
        herdr: &mut gear_herdr::fake::FakeHerdr,
        cfg: &EngineCfg,
    ) -> Vec<Merged> {
        // The deadline is generous: a merge that retries waits between its
        // attempts (T-61).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        loop {
            let merged = merge_passed(store, herdr, cfg, "p", 30).unwrap();
            let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
            if matches!(
                op.state,
                OpState::Confirmed | OpState::Failed | OpState::Void
            ) {
                return merged;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "merge did not complete: {op:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn wait_file(path: &Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !path.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "missing {}",
                path.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// The merge with its retries (T-61): a reason that can clear is retried a
    /// bounded number of times, with a growing wait between the attempts.
    fn merge_retried(
        worktree: &gear_herdr::Worktree,
        repo: &Repo,
        op: &OpRow,
    ) -> Result<String, Failed> {
        let lock = merge_lock(Path::new(&worktree.path))
            .map_err(|reason| Failed::stuck(reason))?
            .ok_or_else(|| Failed::stuck("merge worktree is busy"))?;
        merge_with_retries(worktree, repo, op, &lock)
    }

    /// T-61: a push race is not a failure. The merge waits, tries again from
    /// the new tip of the integration branch, and lands.
    #[test]
    fn a_push_race_is_retried_and_the_merge_lands() {
        let scratch = Scratch::new("race-retry");
        let worktree = scratch.merge_worktree();
        let raced = scratch.root.join("raced");
        // The integration branch moves once, while the gate runs: the first
        // push loses the race, the next attempt starts from the new tip.
        let racer = format!(
            "if [ ! -e '{raced}' ]; then touch '{raced}'; cd {work} && git fetch -q origin \
             && git switch -q dev && git reset -q --hard origin/dev && echo x >> other.txt \
             && git add other.txt && git -c user.email=a@b -c user.name=a commit -qm racer \
             && git push -q origin dev; fi",
            raced = raced.display(),
            work = scratch.work.display(),
        );

        let line = merge_retried(&worktree, &repo(&racer), &op(&scratch.head_of("heat/t5"))).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        let log = scratch.dev_log();
        assert!(log.contains("racer"), "the race happened: {log}");
        assert!(log.contains("task"), "the task landed: {log}");
        assert!(scratch.dev_has_task_commit());
    }

    /// T-61: a gate that could not run (the toolchain is missing) is an
    /// infrastructure error. The merge waits, tries again, and lands.
    #[test]
    fn a_gate_that_could_not_run_is_retried_and_the_merge_lands() {
        let scratch = Scratch::new("gate-infra");
        let worktree = scratch.merge_worktree();
        let ran = scratch.root.join("gate-ran");
        // The first run of the gate cannot find its toolchain (127); the
        // second one runs.
        let gate = format!(
            "if [ ! -e '{ran}' ]; then touch '{ran}'; exit 127; fi",
            ran = ran.display()
        );

        let line = merge_retried(&worktree, &repo(&gate), &op(&scratch.head_of("heat/t5"))).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        assert!(scratch.dev_has_task_commit());
    }

    /// T-61, F2 of the review: a candidate on no pushed ref is retried (it may
    /// not be pushed yet) and rings the captain once when the retries are
    /// used up. Nothing in the store is edited by hand.
    #[test]
    fn a_candidate_on_no_pushed_ref_fails_the_op_and_rings_the_captain_once() {
        let scratch = Scratch::new("unpushed-op");
        // The lane committed but never pushed.
        sh(
            &scratch.work,
            "git switch -q dev && git switch -qc heat/t5-local && echo local > local.txt \
             && git add -A && git -c user.email=a@b -c user.name=a commit -qm local",
        );
        let candidate = scratch.local_head_of("heat/t5-local");
        let (mut store, mut herdr, cfg) = queued_merge_candidate(&scratch, "true", &candidate);

        let merged = wait_merge(&mut store, &mut herdr, &cfg);

        assert!(merged.is_empty(), "{merged:?}");
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Failed, "{op:?}");
        let reason = op.result.clone().unwrap_or_default();
        assert!(reason.contains("no pushed ref"), "{reason}");
        assert!(reason.contains("retried"), "the retries are named: {reason}");
        // dev is untouched: nothing was merged.
        assert_eq!(scratch.dev_log(), "dev", "dev is untouched");
        // One item in the captain's inbox: one ring, not one per tick.
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", op.updated + 1))
            .unwrap();
        assert_eq!(inbox.len(), 1, "{inbox:?}");
        assert!(inbox[0].body.contains("no pushed ref"), "{}", inbox[0].body);
    }

    /// The config of a project whose lane "heat" works in the child repo
    /// "web" (T-53): the child repo has its own integration branch and its
    /// own gate.
    fn child_config(root: &Path, child_gate: &str, project_gate: &str) -> String {
        format!(
            r#"
[project]
id = "p"
herdr_session = "sess"
repo = "{root}"

[[repos]]
name = "web"
path = "web"
integration = "web-dev"
gate = "{child_gate}"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
repo = "web"

[conventions]
gate = "{project_gate}"
"#,
            root = root.display(),
        )
    }

    /// A project with a handed-in task of the lane "heat" and a queued merge
    /// op for it.
    fn child_world(config_toml: &str, candidate: &str) -> Store {
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_toml.to_string();
                tx.update_project(&project)?;
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-5", 1, 1, candidate, "report", 0)?;
                tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    kind: OpKind::Merge,
                    task: Some("T-5".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({ "branch": "heat/t5", "candidate": candidate }),
                    created: 10,
                })?;
                Ok(())
            })
            .unwrap();
        store
    }

    /// T-53: the merge of a task of a lane bound to a child repo runs in that
    /// repo: its own worktree, its own integration branch and its own gate.
    #[test]
    fn a_task_merges_in_the_repo_of_its_lane() {
        let scratch = Scratch::new("child");
        let child = Child::new(&scratch.root, "web", "web-dev");
        let worktree = child.merge_worktree("web-dev");
        let candidate = child.head_of("heat/t5");
        let child_gate = scratch.root.join("child-gate");
        let project_gate = scratch.root.join("project-gate");
        let config_toml = child_config(
            &scratch.root,
            &format!("pwd > '{}'", child_gate.display()),
            &format!("touch '{}'", project_gate.display()),
        );
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("home/merge", &worktree.path, &worktree.pane_id);
        let mut store = child_world(&config_toml, &candidate);
        let mut cfg = EngineCfg::default();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        let merged = wait_merge(&mut store, &mut herdr, &cfg);

        assert_eq!(merged.len(), 1, "{merged:?}");
        // The worktree is the one of the child repo, under its own label.
        let opened = herdr
            .calls
            .iter()
            .find_map(|call| match call {
                gear_herdr::fake::Call::WorktreeOpen { repo, branch, label } => {
                    Some((repo.clone(), branch.clone(), label.clone()))
                }
                _ => None,
            })
            .expect("the engine opened the merge worktree");
        assert_eq!(opened.0, format!("{}/web", scratch.root.display()));
        assert_eq!(opened.1, "home/merge");
        assert_eq!(opened.2, "p-merge-web", "the label names the repo");
        // The merge landed on the child's own integration branch.
        let log = child.log_of("web-dev");
        assert!(log.contains("task"), "{log}");
        assert!(log.contains("review"), "{log}");
        // The child's gate ran, in the child's worktree; the project's gate
        // did not run at all.
        assert!(child_gate.exists(), "the child's gate did not run");
        let gate_dir = fs::read_to_string(&child_gate).unwrap();
        assert!(
            gate_dir.trim().ends_with("web-merge"),
            "the gate ran in the child's worktree: {}",
            gate_dir.trim()
        );
        assert!(!project_gate.exists(), "the project's gate must not run");
        // The project root was not touched: its dev is where it was.
        assert_eq!(scratch.dev_log(), "dev", "the root repo was not merged into");
    }

    /// T-53: the merge worktree of a repo is created in that repo, cut from
    /// that repo's integration branch.
    #[test]
    fn a_merge_worktree_is_created_in_the_repo_of_the_task() {
        let scratch = Scratch::new("child-create");
        let child = Child::new(&scratch.root, "web", "web-dev");
        let candidate = child.head_of("heat/t5");
        let config_toml = child_config(&scratch.root, "true", "true");
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let mut store = child_world(&config_toml, &candidate);
        let mut cfg = EngineCfg::default();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        merge_passed(&mut store, &mut herdr, &cfg, "p", 30).unwrap();

        let created = herdr
            .calls
            .iter()
            .find_map(|call| match call {
                gear_herdr::fake::Call::WorktreeCreate { repo, branch, base, label } => {
                    Some((repo.clone(), branch.clone(), base.clone(), label.clone()))
                }
                _ => None,
            })
            .expect("the engine created the merge worktree");
        assert_eq!(created.0, format!("{}/web", scratch.root.display()));
        assert_eq!(created.1, "home/merge");
        assert_eq!(created.2, "web-dev", "the repo's own integration branch");
        assert_eq!(created.3, "p-merge-web");
    }

    /// T-61: the whole path, with no hand edit in the store: the lane worked
    /// on another branch, the review passed, and the engine queued and ran the
    /// merge itself.
    #[test]
    fn the_engine_merges_a_candidate_on_another_branch_itself() {
        let scratch = Scratch::new("other-branch-op");
        let worktree = scratch.merge_worktree();
        // The lane's rework round pushed its work to another branch.
        sh(
            &scratch.work,
            "git switch -q dev && git switch -qc heat/t5-v2 && echo rework > rework.txt \
             && git add -A && git -c user.email=a@b -c user.name=a commit -qm rework \
             && git push -q origin heat/t5-v2",
        );
        let candidate = scratch.head_of("heat/t5-v2");
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("home/merge", &worktree.path, &worktree.pane_id);
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-5", 1, 1, &candidate, "report", 0)?;
                tx.insert_verdict(&gear_store::VerdictRow {
                    project: "p".into(),
                    task: "T-5".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: candidate.clone(),
                    passed: true,
                    evidence: Some("ops/reviews/T-5.md".into()),
                    by_seat: "p:captain".into(),
                    created: 10,
                })?;
                Ok(())
            })
            .unwrap();
        let mut cfg = EngineCfg::default();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        // The engine queues the merge op itself and runs it.
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let merged = wait_merge(&mut store, &mut herdr, &cfg);

        assert_eq!(merged.len(), 1, "{merged:?}");
        let log = scratch.dev_log();
        assert!(log.contains("rework"), "the candidate landed: {log}");
        assert!(log.contains("review"), "the review landed with it: {log}");
        // The branch that held the candidate is closed.
        let branches = scratch.origin_branches();
        assert!(!branches.contains("heat/t5-v2"), "{branches}");
    }

    #[test]
    fn dispatch_and_doorbells_continue_while_the_gate_sleeps() {
        let scratch = Scratch::new("tick-gate");
        let started = scratch.root.join("gate-started");
        let done = scratch.root.join("gate-finished");
        let gate = format!(
            "touch '{}'; sleep 2; touch '{}'",
            started.display(),
            done.display()
        );
        let (mut store, mut herdr, cfg) = queued_merge(&scratch, &gate);
        merge_passed(&mut store, &mut herdr, &cfg, "p", 10).unwrap();
        wait_file(&started);
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Idle);
        herdr.add_agent("w9:p1", "claude", gear_herdr::AgentStatus::Idle);
        store
            .write(|tx| {
                let mut next = tx.get_task("p", "T-5")?.unwrap();
                next.id = "T-6".into();
                next.state = TaskState::Ready;
                next.rank = 2;
                tx.insert_task(&next)?;
                tx.insert_contract(&gear_store::ContractRow {
                    project: "p".into(),
                    task: "T-6".into(),
                    version: 1,
                    goal: "keep working".into(),
                    accept: "done".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: 0,
                })?;
                tx.insert_message(&NewMessage {
                    project: "p".into(),
                    thread: "question:1".into(),
                    task: None,
                    attempt: None,
                    sender: "p:heat".into(),
                    sender_gen: 1,
                    recipient: "p:captain".into(),
                    kind: MsgKind::Question,
                    urgent: false,
                    body: "ready for a decision".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: 11,
                })?;
                Ok(())
            })
            .unwrap();
        let report = crate::tick(&mut store, &mut herdr, &cfg, 20).unwrap();
        assert!(!done.exists(), "tick waited for the gate");
        assert!(report.merged.is_empty());
        assert!(
            herdr
                .prompts_to("w1:p1")
                .iter()
                .any(|text| text.contains("New task T-6")),
            "dispatch: {:?}",
            herdr.calls
        );
        assert!(
            !herdr.prompts_to("w9:p1").is_empty(),
            "captain doorbell: {:?}",
            herdr.calls
        );
        // Cloned configurations share the active worker, so another tick
        // cannot start the same merge or open its worktree again.
        let calls = herdr.calls.len();
        merge_passed(&mut store, &mut herdr, &cfg.clone(), "p", 21).unwrap();
        assert_eq!(herdr.calls.len(), calls);
        let result = wait_merge(&mut store, &mut herdr, &cfg);
        assert_eq!(result.len(), 1);
        let updated = store
            .read(|tx| tx.get_op("p", "O-1"))
            .unwrap()
            .unwrap()
            .updated;
        assert!(updated > 20, "completion time follows gate and push");
        assert!(merge_passed(&mut store, &mut herdr, &cfg, "p", updated + 1)
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .read(|tx| tx.get_op("p", "O-1"))
                .unwrap()
                .unwrap()
                .updated,
            updated
        );
    }

    #[test]
    fn a_second_ready_merge_waits_for_the_first_one() {
        let scratch = Scratch::new("serial");
        sh(&scratch.root, "git -C origin.git branch heat/t6 heat/t5 && git -C origin.git branch review/t6 review/t5");
        let started = scratch.root.join("gates");
        let gate = format!("echo gate >> '{}'; sleep 1", started.display());
        let (mut store, mut herdr, cfg) = queued_merge(&scratch, &gate);
        store
            .write(|tx| {
                let mut next = tx.get_task("p", "T-5")?.unwrap();
                next.id = "T-6".into();
                next.state = TaskState::Ready;
                next.rank = 2;
                tx.insert_task(&next)?;
                let candidate = tx.get_attempt("p", "T-5", 1)?.unwrap().candidate.unwrap();
                gear_app::tasks::start_attempt(tx, "p", "T-6", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-6", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-6", 1, 1, &candidate, "report", 0)?;
                tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    kind: OpKind::Merge,
                    task: Some("T-6".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({ "branch": "heat/t6", "candidate": candidate }),
                    created: 11,
                })?;
                Ok(())
            })
            .unwrap();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        wait_file(&started);
        merge_passed(&mut store, &mut herdr, &cfg, "p", 21).unwrap();
        assert_eq!(
            store
                .read(|tx| tx.get_op("p", "O-2"))
                .unwrap()
                .unwrap()
                .state,
            OpState::Intended
        );
        assert_eq!(fs::read_to_string(&started).unwrap().lines().count(), 1);
        assert_eq!(wait_merge(&mut store, &mut herdr, &cfg).len(), 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let merged = merge_passed(&mut store, &mut herdr, &cfg, "p", 30).unwrap();
            if !merged.is_empty() {
                assert_eq!(merged[0].task, "T-6");
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(fs::read_to_string(&started).unwrap().lines().count(), 2);
        assert_eq!(
            store
                .read(|tx| tx.get_op("p", "O-2"))
                .unwrap()
                .unwrap()
                .state,
            OpState::Confirmed
        );
    }

    #[test]
    fn a_busy_worktree_waits_without_running_a_second_merge() {
        let scratch = Scratch::new("locked");
        let (mut store, mut herdr, cfg) = queued_merge(&scratch, "true");
        let lock = merge_lock(&scratch.root.join("merge")).unwrap().unwrap();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while store
            .read(|tx| tx.get_op("p", "O-1"))
            .unwrap()
            .unwrap()
            .state
            != OpState::Unknown
        {
            merge_passed(&mut store, &mut herdr, &cfg, "p", 21).unwrap();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            scratch.dev_log(),
            "dev",
            "no git merge while another runtime owns the lock"
        );
        drop(lock);
        assert_eq!(wait_merge(&mut store, &mut herdr, &cfg).len(), 1);
    }

    #[test]
    fn interrupted_worker_child() {
        let Some(path) = std::env::var_os("GEAR_TEST_MERGE_WORKTREE") else {
            return;
        };
        let worktree = gear_herdr::Worktree {
            path: PathBuf::from(path).display().to_string(),
            workspace_id: "test".into(),
            pane_id: "test".into(),
            branch: "home/merge".into(),
        };
        merge_one(
            &worktree,
            &repo(&std::env::var("GEAR_TEST_MERGE_GATE").unwrap()),
            &op(&std::env::var("GEAR_TEST_MERGE_CANDIDATE").unwrap()),
        )
        .unwrap();
    }

    #[test]
    fn a_killed_worker_recovers_from_origin_without_a_half_merge() {
        let scratch = Scratch::new("killed");
        let (mut store, mut herdr, cfg) = queued_merge(&scratch, "true");
        let started = scratch.root.join("gate-started");
        let done = scratch.root.join("gate-finished");
        let release = scratch.root.join("release-gate");
        let gate = format!(
            "touch '{}'; for i in 1 2 3 4 5 6 7 8 9 10; do [ -e '{}' ] && break; sleep 1; done; touch '{}'",
            started.display(), release.display(), done.display()
        );
        let candidate = scratch.head_of("heat/t5");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "merge::tests::interrupted_worker_child",
                "--nocapture",
            ])
            .env("GEAR_TEST_MERGE_WORKTREE", scratch.root.join("merge"))
            .env("GEAR_TEST_MERGE_GATE", gate)
            .env("GEAR_TEST_MERGE_CANDIDATE", candidate)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        wait_file(&started);
        assert_eq!(
            scratch.dev_log(),
            "dev",
            "gate has not passed, so origin is untouched"
        );
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(merge_lock(&scratch.root.join("merge")).unwrap().is_none(),
            "the surviving gate must exclude recovery until it exits");
        fs::write(&release, "release").unwrap();
        store
            .write(|tx| tx.set_op_state("p", "O-1", OpState::Executing, None, 20))
            .unwrap();
        crate::ops::recover_on_start(&mut store, 21).unwrap();
        assert_eq!(wait_merge(&mut store, &mut herdr, &cfg).len(), 1);
        let log = scratch.dev_log();
        assert_eq!(
            log.lines()
                .filter(|line| line.starts_with("merge(T-5)"))
                .count(),
            2,
            "{log}"
        );
        assert!(scratch.dev_has_task_commit());
        // The orphaned gate finishes before recovery acquires the lock.
        // No surviving merge thread can push its half-finished tree.
        wait_file(&done);
    }

    #[test]
    fn a_sleeping_gate_does_not_block_the_merge_step() {
        let scratch = Scratch::new("nonblocking");
        let done = scratch.root.join("gate-finished");
        let gate = format!("sleep 1; touch '{}'", done.display());
        let (mut store, mut herdr, cfg) = queued_merge(&scratch, &gate);
        let launched_at = wall_time();
        let result = merge_passed(&mut store, &mut herdr, &cfg, "p", launched_at).unwrap();
        assert!(!done.exists(), "merge_passed waited for the sleeping gate");
        assert!(result.is_empty(), "the merge is still running");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            let result = merge_passed(&mut store, &mut herdr, &cfg, "p", 30).unwrap();
            if !result.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the merge never finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(done.exists());
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Confirmed);
        assert!(op.updated > launched_at, "merge time follows the sleeping gate");
        assert!(op.updated <= wall_time());
        assert_eq!(store.read(|tx| tx.attempt_times("p", "T-5", 1)).unwrap().merged, Some(op.updated));
    }

    #[test]
    fn a_clean_merge_is_pushed_and_the_branches_are_gone() {
        let scratch = Scratch::new("clean");
        let worktree = scratch.merge_worktree();

        let line = merge_one(&worktree, &repo("true"), &op(&scratch.head_of("heat/t5"))).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        // dev has both commits …
        let log = scratch.dev_log();
        assert!(log.contains("task"), "{log}");
        assert!(log.contains("review"), "{log}");
        assert_eq!(log.lines().filter(|line| line.starts_with("merge(T-5)")).count(), 2, "{log}");
        assert!(scratch.dev_has_task_commit());
        // … and the two branches are closed on origin.
        let branches = scratch.origin_branches();
        assert!(!branches.contains("heat/t5"), "{branches}");
        assert!(!branches.contains("review/t5"), "{branches}");
    }

    /// A push rejected because dev moved during the gate (the captain pushed)
    /// is a race the merge retries, not a failed merge.
    #[test]
    fn dev_moving_during_the_gate_is_a_race() {
        let scratch = Scratch::new("race");
        let worktree = scratch.merge_worktree();
        let racer = format!(
            "cd {} && git fetch -q origin && git switch -q dev && git reset -q --hard origin/dev && echo x >> other.txt && git add other.txt && git -c user.email=a@b -c user.name=a commit -qm racer && git push -q origin dev",
            scratch.work.display()
        );

        let failed = merge_one(&worktree, &repo(&racer), &op(&scratch.head_of("heat/t5"))).unwrap_err();

        assert!(failed.reason.starts_with(RACE), "{}", failed.reason);
        assert_eq!(failed.blocker, Blocker::Clear, "a race can clear");
        assert!(!scratch.dev_has_task_commit(), "nothing of T-5 was pushed");
    }

    /// F1 of the T-61 review: a push that a hook rejects once is not a failed
    /// merge. The merge waits, tries again, and lands.
    #[test]
    fn a_push_that_a_hook_rejects_once_is_retried_and_lands() {
        let scratch = Scratch::new("hook");
        let worktree = scratch.merge_worktree();
        // The repo's own pre-push hook says no once, then lets it through.
        let marker = scratch.root.join("hook-said-no");
        let hooks = scratch.work.join(".git/hooks");
        fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("pre-push");
        fs::write(
            &hook,
            format!(
                "#!/bin/sh\nif [ ! -e '{marker}' ]; then touch '{marker}'; echo 'the hook says no' >&2; exit 1; fi\nexit 0\n",
                marker = marker.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let line = merge_retried(&worktree, &repo("true"), &op(&scratch.head_of("heat/t5"))).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        assert!(marker.exists(), "the hook said no once");
        assert!(scratch.dev_has_task_commit());
    }

    /// F2 of the T-61 review: a candidate that is not pushed yet is not a
    /// failed merge. The lane pushes it, and the merge lands.
    #[test]
    fn a_candidate_pushed_late_merges_after_the_retry() {
        let scratch = Scratch::new("late-push");
        let worktree = scratch.merge_worktree();
        // The lane committed but did not push yet.
        sh(
            &scratch.work,
            "git switch -q dev && git switch -qc heat/t5-late && echo late > late.txt \
             && git add -A && git -c user.email=a@b -c user.name=a commit -qm late",
        );
        let candidate = scratch.local_head_of("heat/t5-late");
        // The lane pushes while the merge waits.
        let work = scratch.work.clone();
        let pusher = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            sh(&work, "git push -q origin heat/t5-late");
        });

        let line = merge_retried(&worktree, &repo("true"), &op(&candidate)).unwrap();

        pusher.join().unwrap();
        assert!(line.contains("merged T-5 into dev"), "{line}");
        assert!(scratch.dev_has_task_commit());
    }

    /// L-67: a failed merge is not queued again tick after tick (it filed 120
    /// items for the captain); a new passing attempt gets its own merge.
    #[test]
    fn a_failed_merge_is_not_queued_again() {        let mut store = gear_store::Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                gear_store::testing::seed_project(tx, "p", "sess", 1)?;
                let id = tx
                    .insert_op(&NewOp {
                        project: "p".into(),
                        seat: "p:heat".into(),
                        generation: 1,
                        kind: OpKind::Merge,
                        task: Some("T-5".into()),
                        attempt: Some(1),
                        ref_msg: None,
                        plan_version: None,
                        payload: serde_json::json!({}),
                        created: 1,
                    })?
                    .id;
                tx.set_op_state("p", &id, OpState::Executing, None, 2)?;
                tx.set_op_state("p", &id, OpState::Failed, Some("conflict"), 3)
            })
            .unwrap();

        assert!(merge_op_of(&mut store, "p", "T-5", 1).unwrap().is_some(), "the failed merge stands");
        assert!(merge_op_of(&mut store, "p", "T-5", 2).unwrap().is_none(), "a new attempt merges anew");
    }

    #[test]
    fn a_conflict_leaves_dev_untouched_and_fails() {
        let scratch = Scratch::new("conflict");
        // Make the task branch and dev disagree on the same line.
        // dev moves the same line the task branch appends to.
        sh(&scratch.work, "git switch -q dev && printf 'dev\nother\n' > README.md && git -c user.email=a@b -c user.name=a commit -qam other && git push -q origin dev");
        let worktree = scratch.merge_worktree();
        let before = scratch.dev_log();

        let failed = merge_one(&worktree, &repo("true"), &op(&scratch.head_of("heat/t5"))).unwrap_err();

        assert!(failed.reason.contains("conflict merging"), "{}", failed.reason);
        assert_eq!(failed.blocker, Blocker::Stuck, "a conflict does not clear");
        assert_eq!(scratch.dev_log(), before, "dev is untouched");
        let branches = scratch.origin_branches();
        assert!(branches.contains("heat/t5"), "{branches}");
    }

    #[test]
    fn a_failing_gate_leaves_dev_untouched_and_fails() {
        let scratch = Scratch::new("gate");
        let worktree = scratch.merge_worktree();
        let before = scratch.dev_log();

        let failed = merge_one(&worktree, &repo("echo the gate said no >&2; exit 1"), &op(&scratch.head_of("heat/t5"))).unwrap_err();

        assert!(failed.reason.contains("gate failed"), "{}", failed.reason);
        assert!(failed.reason.contains("the gate said no"), "{}", failed.reason);
        assert_eq!(failed.blocker, Blocker::Stuck, "the work is not good enough");
        assert_eq!(scratch.dev_log(), before, "dev is untouched");
        assert!(!scratch.dev_has_task_commit());
        let branches = scratch.origin_branches();
        assert!(branches.contains("heat/t5"), "{branches}");
        // The worktree is back at the integration branch (nothing half-merged
        // for the next run).
        let head = sh(Path::new(&worktree.path), "git log --format=%s -1");
        assert_eq!(head, "dev", "{head}");
    }

    /// T-41: the gate is the project's build and test command. It must not
    /// see the store of this machine: a test that inherited `GEAR_DB` would
    /// write to the production store (L-59).
    #[test]
    fn the_gate_runs_without_the_store_of_this_machine() {
        let command = gate_command("cargo test", Path::new("/tmp"));
        let removes = |name: &str| {
            command
                .get_envs()
                .filter(|(key, value)| {
                    *key == std::ffi::OsStr::new(name) && value.is_none()
                })
                .count()
        };
        assert_eq!(removes("GEAR_DB"), 1, "the gate removes GEAR_DB");
        assert_eq!(removes("GEAR_CTX"), 1, "the gate removes GEAR_CTX");
    }

    #[test]
    fn a_branch_that_moved_past_the_candidate_merges_the_candidate_only() {
        // P0 of the review round 2, kept by T-61: the approved candidate is
        // what merges. A commit pushed to the task branch after the verdict is
        // not merged, and it is not a failure either: the branch moved, the
        // candidate did not.
        let scratch = Scratch::new("moved");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        // An unreviewed commit lands on the task branch.
        sh(
            &scratch.work,
            "git switch -q heat/t5 && echo sneaky > sneaky.txt && git add -A \
             && git -c user.email=a@b -c user.name=a commit -qm sneaky && git push -q origin heat/t5",
        );

        let line = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        let log = scratch.dev_log();
        assert!(log.contains("task"), "{log}");
        assert!(log.contains("review"), "{log}");
        assert!(!log.contains("sneaky"), "the unreviewed commit is not on dev: {log}");
        assert!(scratch.dev_has_task_commit());
        // The branch holds more than the candidate: it is not closed.
        let branches = scratch.origin_branches();
        assert!(branches.contains("heat/t5"), "{branches}");
        assert!(!branches.contains("review/t5"), "{branches}");
    }

    /// T-61: the approved commit merges, wherever it is. The branch the lane
    /// named points at another commit, and the candidate sits on the branch a
    /// rework round used.
    #[test]
    fn a_candidate_on_another_branch_merges() {
        let scratch = Scratch::new("other-branch");
        let worktree = scratch.merge_worktree();
        // The rework round pushed its work to another branch.
        sh(
            &scratch.work,
            "git switch -q dev && git switch -qc heat/t5-v2 && echo rework > rework.txt \
             && git add -A && git -c user.email=a@b -c user.name=a commit -qm rework \
             && git push -q origin heat/t5-v2",
        );
        let candidate = scratch.head_of("heat/t5-v2");
        assert_ne!(candidate, scratch.head_of("heat/t5"), "the branch moved on");

        let line = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        let log = scratch.dev_log();
        assert!(log.contains("rework"), "the candidate landed: {log}");
        assert!(log.contains("review"), "the review landed with it: {log}");
        // The branch that held the candidate is closed; the branch the op
        // named (which points at other work) is not.
        let branches = scratch.origin_branches();
        assert!(!branches.contains("heat/t5-v2"), "{branches}");
        assert!(branches.contains("heat/t5"), "{branches}");
    }

    /// T-61, F2 of the review: a candidate that is on no pushed ref may not be
    /// pushed yet: the merge retries, and the bounded retry decides.
    #[test]
    fn a_candidate_on_no_pushed_ref_fails_with_a_clear_reason() {
        let scratch = Scratch::new("unpushed");
        let worktree = scratch.merge_worktree();
        // The lane committed but never pushed. The commit is in the repo (the
        // merge worktree shares its objects) but on no pushed ref.
        sh(
            &scratch.work,
            "git switch -q dev && git switch -qc heat/t5-local && echo local > local.txt \
             && git add -A && git -c user.email=a@b -c user.name=a commit -qm local",
        );
        let candidate = scratch.local_head_of("heat/t5-local");
        let before = scratch.dev_log();

        let failed = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap_err();

        assert!(failed.reason.contains("no pushed ref"), "{}", failed.reason);
        assert!(failed.reason.contains(&candidate), "{}", failed.reason);
        assert_eq!(
            failed.blocker,
            Blocker::Clear,
            "a candidate that is not pushed yet can clear"
        );
        assert_eq!(scratch.dev_log(), before, "dev is untouched");
        assert!(!scratch.dev_has_task_commit());
    }

    /// T-61: a candidate that is not fetchable yet may still be: the merge
    /// retries it instead of failing.
    #[test]
    fn a_candidate_that_is_not_fetchable_yet_is_retried() {
        let scratch = Scratch::new("not-fetchable");
        let worktree = scratch.merge_worktree();
        let missing = "0123456789abcdef0123456789abcdef01234567";

        let failed = merge_one(&worktree, &repo("true"), &op(missing)).unwrap_err();

        assert!(failed.reason.contains("not fetchable"), "{}", failed.reason);
        assert_eq!(failed.blocker, Blocker::Clear, "a push that has not landed can clear");
        assert_eq!(scratch.dev_log(), "dev", "dev is untouched");
    }

    #[test]
    fn a_settled_review_op_without_a_review_key_still_merges() {
        // F5 of the review: `tasks::settle_review` queues the op with `branch`
        // and `candidate` only. The missing review branch must be derived —
        // never `git merge --no-ff origin/`.
        let scratch = Scratch::new("settled");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        let mut settled = op(&candidate);
        settled.payload = serde_json::json!({ "branch": "heat/t5", "candidate": candidate.clone() });

        let line = merge_one(&worktree, &repo("true"), &settled).unwrap();

        assert!(line.contains("merged T-5"), "{line}");
        let log = scratch.dev_log();
        assert!(log.contains("task"), "{log}");
        // The review branch exists in this fixture, so it was merged too.
        assert!(log.contains("review"), "{log}");
    }

    #[test]
    fn a_missing_review_branch_does_not_publish_a_partial_merge() {
        // F6 of the review: a merge publishes the approved candidate *and* its
        // review branch. A review branch that is gone on origin is not
        // silently skipped: the merge fails with the git diagnostic, dev
        // stays where it was, and nothing is pushed.
        let scratch = Scratch::new("noreview");
        // Only the review branch is gone; the task branch and the candidate
        // are intact.
        sh(&scratch.root, "git -C origin.git update-ref -d refs/heads/review/t5");
        let worktree = scratch.merge_worktree();
        let before = scratch.dev_log();

        let failed =
            merge_one(&worktree, &repo("true"), &op(&scratch.head_of("heat/t5"))).unwrap_err();

        assert!(failed.reason.contains("review branch"), "{}", failed.reason);
        assert!(failed.reason.contains("origin/review/t5"), "{}", failed.reason);
        assert!(failed.reason.contains("rev-parse"), "the git diagnostic: {}", failed.reason);
        assert_eq!(failed.blocker, Blocker::Stuck, "a missing review does not clear");
        assert_eq!(scratch.dev_log(), before, "dev is untouched");
        assert!(!scratch.dev_has_task_commit(), "no partial merge on dev");
        let branches = scratch.origin_branches();
        assert!(branches.contains("heat/t5"), "{branches}");
        // The worktree is back at the integration branch (nothing half-merged
        // for the next run).
        let head = sh(Path::new(&worktree.path), "git log --format=%s -1");
        assert_eq!(head, "dev", "{head}");
    }

    #[test]
    fn a_missing_review_branch_fails_the_op_and_tells_the_captain() {
        // The public path: the engine's merge step runs, the review branch is
        // gone on origin, so the op fails with the git diagnostic, dev is
        // untouched and the captain gets one item (F6 of the review).
        let scratch = Scratch::new("noreview-op");
        sh(&scratch.root, "git -C origin.git update-ref -d refs/heads/review/t5");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("home/merge", &worktree.path, &worktree.pane_id);
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                gear_app::tasks::submit_result(
                    tx,
                    "p",
                    "T-5",
                    1,
                    1,
                    &candidate,
                    "ops/reports/T-5.md",
                    0,
                )?;
                tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    kind: OpKind::Merge,
                    task: Some("T-5".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({
                        "branch": "heat/t5",
                        "candidate": candidate,
                        "review": "review/t5",
                    }),
                    created: 10,
                })?;
                Ok(())
            })
            .unwrap();

        let mut cfg = EngineCfg::default();
        cfg.profiles.insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let merged = wait_merge(&mut store, &mut herdr, &cfg);

        assert!(merged.is_empty(), "{merged:?}");
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Failed, "{op:?}");
        let reason = op.result.clone().unwrap_or_default();
        assert!(reason.contains("review branch"), "{reason}");
        assert!(reason.contains("origin/review/t5"), "{reason}");
        // dev is untouched: no partial merge.
        assert_eq!(scratch.dev_log(), "dev", "dev is untouched");
        assert!(!scratch.dev_has_task_commit());
        // One item in the captain's inbox.
        let inbox = store.read(|tx| tx.actionable_for("p", "p:captain", op.updated + 1)).unwrap();
        assert_eq!(inbox.len(), 1, "{inbox:?}");
        assert!(inbox[0].body.contains("review branch"), "{}", inbox[0].body);
    }

    #[test]
    fn a_retry_merges_once_and_pushes() {
        // The restart case: the op ran, the engine died before recording it,
        // and the op comes back as Unknown. The sequence is idempotent: the
        // reset discards the half-run, the merge is already there once.
        let scratch = Scratch::new("retry");
        let worktree = scratch.merge_worktree();
        let repo = repo("true");

        let candidate = scratch.head_of("heat/t5");
        merge_one(&worktree, &repo, &op(&candidate)).unwrap();
        let first = scratch.dev_log();
        // The second run: the branches are gone, and the candidate is on dev.
        merge_one(&worktree, &repo, &op(&candidate)).unwrap();
        let second = scratch.dev_log();

        assert_eq!(first, second, "no second merge, no lost push");
        assert_eq!(
            second.lines().filter(|line| line.starts_with("merge(T-5)")).count(),
            2,
            "exactly the two merge commits"
        );
    }

    /// T-38: the merge is a step of the attempt and has its own time: the
    /// candidate landed on the integration branch then, not when the op was
    /// queued.
    #[test]
    fn a_merged_attempt_gets_its_merge_time() {
        let scratch = Scratch::new("mergetime");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("home/merge", &worktree.path, &worktree.pane_id);
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                gear_app::tasks::submit_result(
                    tx,
                    "p",
                    "T-5",
                    1,
                    1,
                    &candidate,
                    "ops/reports/T-5.md",
                    0,
                )?;
                tx.insert_verdict(&gear_store::VerdictRow {
                    project: "p".into(),
                    task: "T-5".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: candidate.clone(),
                    passed: true,
                    evidence: Some("ops/reviews/T-5.md".into()),
                    by_seat: "p:captain".into(),
                    created: 10,
                })?;
                Ok(())
            })
            .unwrap();

        let mut cfg = EngineCfg::default();
        cfg.profiles.insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
        let before = wall_time();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let merged = wait_merge(&mut store, &mut herdr, &cfg);

        assert_eq!(merged.len(), 1, "{merged:?}");
        let times = store.read(|tx| tx.attempt_times("p", "T-5", 1)).unwrap();
        let completed = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap().updated;
        assert_eq!(times.merged, Some(completed), "the merge has its own time");
        assert!(completed >= before && completed <= wall_time());
        // The other steps keep theirs.
        assert_eq!(times.started, Some(0));
        assert_eq!(times.handed_in, Some(0));
    }
    #[test]
    fn passed_work_still_merges_while_the_captain_is_on_quota() {
        let scratch = Scratch::new("captain-quota");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("home/merge", &worktree.path, &worktree.pane_id);
        herdr.add_agent("w9:p1", "claude", gear_herdr::AgentStatus::Idle);
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen("w9:p1", "────────────────\n❯\n────────────────\nOpus 5.5 · medium /repo (dev) · ctx 63% · 5h 100%\n⏵⏵ bypass permissions on");
        let mut store = world(TaskState::Ready);
        store.write(|tx| {
            gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
            gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
            gear_app::tasks::submit_result(tx, "p", "T-5", 1, 1, &candidate, "report", 1)?;
            tx.insert_verdict(&gear_store::VerdictRow {
                project: "p".into(), task: "T-5".into(), attempt: 1, contract_version: 1,
                candidate, passed: true, evidence: Some("review".into()),
                by_seat: "p:captain".into(), created: 10,
            })
        }).unwrap();
        let mut cfg = EngineCfg::default();
        for kind in ["opencode", "claude"] {
            let mut profile = gear_herdr::builtin(kind).unwrap();
            profile.confirm_rounds = 1;
            cfg.profiles.insert(kind.into(), profile);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let report = crate::tick(&mut store, &mut herdr, &cfg, 20).unwrap();
            if !report.merged.is_empty() { break; }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(store.read(|tx| tx.captain_quota_paused("p")).unwrap());
        assert!(scratch.dev_has_task_commit());
        assert!(scratch.dev_log().contains("review"));
        assert!(herdr.prompts_to("w9:p1").is_empty(), "the unavailable captain is not rung");
    }

}
