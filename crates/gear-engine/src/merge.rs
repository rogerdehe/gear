//! The engine merges a passed task (design/08, conventions 17 §8, plan T-12).
//!
//! A task whose current attempt has a passing verdict and that has no merge op
//! yet gets one `merge` op. A background thread executes it in the integration
//! worktree of the task's repo (`<project>/merge`, created once from that repo's
//! integration branch, reopened afterwards):
//!
//! 1. fetch every branch of the project's remote (`[conventions].remote`,
//!    `origin` by default) and reset to `<remote>/<integration>`: a
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
//! A candidate that carries another task's work waits for that task (T-62):
//! the work of a task that has not landed on the integration branch yet must
//! not ride in with the task built on it. The wait is not a failure: the op
//! goes back to the queue, and the captain is told once which task it waits
//! for.
//!
//! A project of several repos merges each of them on its own branch and with
//! its own gate (T-53): the task's repo is its lane's repo, and one worker
//! runs per repo.
//!
//! Any failure before the push leaves the remote integration branch untouched
//! (`git reset --hard <remote>/<integration>`), records the op Failed with the
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
use std::sync::{Arc, Condvar, Mutex};
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
pub struct MergeWorkers {
    /// The one worker per repo and integration branch, whichever project
    /// it merges for: two projects that land on the same branch of the same
    /// repo merge one after the other, never gate side by side on a base
    /// the other is about to move.
    workers: Arc<Mutex<BTreeMap<String, Worker>>>,
    /// Bumped whenever the merge step moves: a worker reports its result, and
    /// the tick collects one. A caller that must know whether a step did
    /// anything waits on this instead of polling the clock (T-127).
    progress: Arc<(Mutex<u64>, Condvar)>,
    /// The merge worktrees whose `core.hooksPath` an older engine's install was
    /// already cleaned from (T-107). Cleaning is idempotent, the path of a
    /// repo's merge worktree does not change, and nothing in this engine
    /// installs a hook again — so once per worktree, not once per merge
    /// attempt, each of which is a git process the tick pays otherwise
    /// (T-127).
    cleaned: Arc<Mutex<std::collections::BTreeSet<String>>>,
    /// The merge workers whose last train stopped on a reason that can clear:
    /// their next merge runs alone, with its bounded retries, so such a
    /// reason ends as it would without a train.
    single: Arc<Mutex<std::collections::BTreeSet<String>>>,
    /// The batch size a repo's next batch merge starts with (T-547): the
    /// project's `merge_batch`, doubled by every batch that landed whole and
    /// halved by a batch a gate turned red, up to `merge_batch` again.
    batch_cap: Arc<Mutex<BTreeMap<String, usize>>>,
}

impl MergeWorkers {
    /// How many steps were taken so far: the token
    /// [`MergeWorkers::wait_progress`] waits past.
    pub fn progress(&self) -> u64 {
        *self.progress.0.lock().unwrap()
    }

    /// Block until the merge step moves after this many steps. `false` means
    /// the wait timed out. A tick never waits here; a test (or a one-shot
    /// merge) does, and never sleeps on the wall clock.
    pub fn wait_progress(&self, seen: u64, timeout: std::time::Duration) -> bool {
        let (count, signal) = &*self.progress;
        let guard = count.lock().unwrap();
        let (guard, _) = signal
            .wait_timeout_while(guard, timeout, |count| *count == seen)
            .unwrap();
        *guard != seen
    }

    /// Whether this worktree still needs an older engine's hook cleaned.
    fn hook_cleaned(&self, worktree: &str) -> bool {
        self.cleaned.lock().unwrap().contains(worktree)
    }

    /// Record that this worktree's hook was cleaned.
    fn hook_cleaned_now(&self, worktree: &str) {
        self.cleaned.lock().unwrap().insert(worktree.to_string());
    }

    /// Whether a merge of this project is running in any of its repos.
    pub fn running(&self, project: &str) -> bool {
        self.workers
            .lock()
            .unwrap()
            .values()
            .any(|worker| worker.op.project == project)
    }
}

#[derive(Debug)]
struct Worker {
    op: OpRow,
    finished: Arc<Mutex<Option<Finished>>>,
}

#[derive(Debug, Clone)]
struct Finished {
    result: Report,
    at: Ts,
}

/// What the merge worker reports back.
#[derive(Debug, Clone)]
enum Report {
    /// Merged and pushed: the result line.
    Merged(String),
    /// The gate passed and the pull request's head is pushed (design §11.2
    /// step 2): the pull request's number and the head the gate checked. The
    /// merge op waits for the land op (step 3).
    Pr { pr: u64, head: String },
    /// The worktree is busy: another engine still holds the merge lock.
    Busy,
    /// The merge waits: the candidate carries the work of other tasks that
    /// have not landed on the integration branch yet (T-62). Each task is
    /// named with whether it has passed its review.
    Waiting(Vec<(String, bool)>),
    /// The merge lost the push race on every try (T-280): the integration
    /// branch moved under each attempt, so the merge goes back to the queue
    /// instead of failing.
    Raced(String),
    /// The merge failed: the reason, and whether it can clear.
    Failed(Failed),
    /// The merge goes back to the queue and runs again from the new tip: the
    /// merge ahead of it in its train did not land, or a reason that can
    /// clear stopped the train.
    Requeue(String),
    /// A merge train: each of its merges with what became of it.
    Train(Vec<(OpRow, Report)>),
}

fn wall_time() -> Ts {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as Ts
}

/// The wall clock in milliseconds: the unit of the probe lines (T-407).
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// The steps of one merge attempt, each one line of the engine log (T-407):
/// `fetch`, `merge`, `gate`, `push`, `close`.
///
/// A step's **begin** and its end are both written into the merge op's meta as
/// they happen (`engine.merge_probe.<op id>`), and every stamp is the wall
/// clock: a step that an engine restart interrupts keeps its durable begin, so
/// the attempt that runs after the restart measures it from the time it really
/// started — its clock is not restarted. The whole always runs from the op's
/// durable intent (`created`).
struct Probe {
    task: String,
    started_ms: i64,
    /// The steps of the op, oldest first: (step, begin_ms, end_ms).
    kept: Vec<(String, i64, Option<i64>)>,
    /// Where `kept` is durable: `None` when the engine has no store path (a
    /// library test), which keeps the probe working without a store.
    store: Option<(gear_store::Store, String)>,
}

impl Probe {
    /// `created` is the merge op's durable intent (whole seconds); `db_path`
    /// and `op_id` name the op's meta record.
    fn new(task: &str, created: Ts, db_path: &str, op_id: &str) -> Probe {
        let now = now_ms();
        let mut store = if db_path.is_empty() {
            None
        } else {
            gear_store::Store::open(std::path::Path::new(db_path))
                .ok()
                .map(|store| (store, probe_key(op_id)))
        };
        let mut kept = Vec::new();
        if let Some((store, key)) = store.as_mut() {
            kept = read_steps(store, key);
        }
        Probe {
            task: task.to_string(),
            started_ms: if created > 0 {
                created.saturating_mul(1000)
            } else {
                now
            },
            kept,
            store,
        }
    }

    /// The durable record, as it stands.
    fn save(&mut self) {
        let Probe { kept, store, .. } = self;
        if let Some((store, key)) = store.as_mut() {
            write_steps(store, key, kept);
        }
    }

    /// A step begins: a step this op already began (an engine restart in the
    /// middle of it) keeps that durable beginning; every other one starts now.
    /// A retry of a step that already ended gets a new boundary, or it would
    /// carry the first attempt's time (F1 of the T-407 review).
    fn begin(&mut self, name: &str) {
        if self
            .kept
            .iter()
            .any(|(step, _, end)| step == name && end.is_none())
        {
            return;
        }
        self.kept.push((name.to_string(), now_ms(), None));
        self.save();
    }

    /// The merge's own completion line (T-407), as a session action's `done`:
    /// the whole operation, from the op's durable intent.
    fn done(&mut self) {
        let total = now_ms().saturating_sub(self.started_ms);
        crate::ops::probe("merge", &self.task, "done", total, total);
    }

    /// The step that ends here, with the operation's time so far: its own time
    /// runs from its durable beginning.
    fn step(&mut self, name: &str) {
        let now = now_ms();
        let begin = match self
            .kept
            .iter()
            .position(|(step, _, end)| step == name && end.is_none())
        {
            Some(at) => {
                let begin = self.kept[at].1;
                self.kept[at].2 = Some(now);
                begin
            }
            None => {
                // A step that ended without a beginning (a path that logged
                // it before the restart): it is measured from now.
                self.kept.push((name.to_string(), now, Some(now)));
                now
            }
        };
        self.save();
        crate::ops::probe(
            "merge",
            &self.task,
            name,
            now.saturating_sub(begin),
            now.saturating_sub(self.started_ms),
        );
    }
}

/// Where a merge op's step boundaries are kept (T-407).
fn probe_key(op_id: &str) -> String {
    format!("engine.merge_probe.{op_id}")
}

/// The step boundaries recorded for an op, oldest first: (step, begin, end).
fn read_steps(store: &mut gear_store::Store, key: &str) -> Vec<(String, i64, Option<i64>)> {
    let Ok(Some(raw)) = store.read(|tx| tx.get_meta(key)) else {
        return Vec::new();
    };
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let step = row.get(0)?.as_str()?.to_string();
            let begin = row.get(1)?.as_i64()?;
            Some((step, begin, row.get(2).and_then(serde_json::Value::as_i64)))
        })
        .collect()
}

/// Record the step boundaries of an op (T-407). A failing store write changes
/// nothing the merge does: the lines are still in the log.
fn write_steps(store: &mut gear_store::Store, key: &str, steps: &[(String, i64, Option<i64>)]) {
    let rows: Vec<serde_json::Value> = steps
        .iter()
        .map(|(step, begin, end)| serde_json::json!([step, begin, end]))
        .collect();
    store
        .write(|tx| tx.set_meta(key, &serde_json::json!(rows).to_string()))
        .ok();
}

/// Collect a finished background merge of `project`, if there is one: its
/// result is settled. A paused project still has its running merge collected
/// (Prometheus O-6339: the merge landed on dev while the project was paused,
/// and the store kept it executing). None when no merge of it has finished.
pub fn collect_finished(
    store: &mut Store,
    cfg: &EngineCfg,
    project: &str,
    config: &gear_app::config::ProjectConfig,
) -> GearResult<Option<Vec<Merged>>> {
    // A worker survives between ticks. Collect only a completed result;
    // never join a running thread. One worker per repo (T-53): the workers of
    // the repos of a project are collected one after the other.
    let mut workers =
        cfg.merges.workers.lock().map_err(|_| {
            GearError::new(ErrorCode::BadTransition, "merge worker registry poisoned")
        })?;
    let finished_keys: Vec<String> = workers
        .iter()
        .filter(|(_, worker)| {
            worker.op.project == project && worker.finished.lock().unwrap().is_some()
        })
        .map(|(key, _)| key.clone())
        .collect();
    if let Some(key) = finished_keys.first() {
        let worker = workers.get(key).expect("the worker was just found");
        let finished = worker.finished.lock().unwrap().clone();
        let Some(finished) = finished else {
            return Ok(None);
        };
        let op = worker.op.clone();
        // A merge that ran alone and landed lets the batch grow again
        // (Prometheus, 2026-11-04: red batches halved the cap to 1, the batch
        // path never runs at 1, and only a batch that landed could double it,
        // so every merge ran alone for the rest of the day).
        let landed_alone = matches!(finished.result, Report::Merged(_));
        let done = settle_report(store, project, config, &op, finished.result, finished.at)?;
        if landed_alone {
            let batch = config.conventions.merge_batch.max(1) as usize;
            let mut caps = cfg.merges.batch_cap.lock().unwrap();
            if let Some(cap) = caps.get_mut(key) {
                *cap = (*cap * 2).min(batch);
            }
        }
        // Keep the completion until its database write succeeded: a transient
        // store error must not lose the result or launch a second worker.
        workers.remove(key);
        // The collection is a step of its own: a waiter must not miss it.
        *cfg.merges.progress.0.lock().unwrap() += 1;
        cfg.merges.progress.1.notify_all();
        return Ok(Some(done));
    }
    Ok(None)
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
    let config = match gear_app::config::parse_row(&row) {
        Ok(config) => config,
        Err(_) => return Ok(merged),
    };

    // 0. the pull-request merges that landed (design §11.2 step 5): a merge
    //    op that waits for its land op is confirmed with it.
    merged.extend(confirm_pr_merges(store, herdr, cfg, project, &config, now)?);

    // 1. queue a merge for every passed task that has none.
    let passed = passed_tasks(store, project)?;
    // One read of the merge ops per tick, not one full scan per passed task:
    // the scan per task grew with every op ever written and held a core.
    let queued = merge_keys(store, project)?;
    for (task, lane, candidate, attempt, attempt_candidate, mechanical) in passed {
        if queued.contains(&(task.clone(), attempt)) {
            continue;
        }
        // T-582: an integration task's branch is the feature's own branch, not
        // one the lane cut from the template, and the close keeps it (the
        // feature's; the captain deletes it at landing).
        let feature_branch = store.read(|tx| tx.task_branch_override(project, &task))?;
        let branch = match &feature_branch {
            Some(branch) => branch.clone(),
            None => branch_of(&config.conventions.task_branch, project, &lane, &task),
        };
        let mut payload = serde_json::json!({
            "branch": branch,
            "candidate": candidate,
            "attempt_candidate": attempt_candidate,
            "review": review_branch(&config.conventions.review_branch, project, &lane, &task),
        });
        // T-582: the feature branch is not a task branch to close.
        if feature_branch.is_some() {
            payload["keep_branch"] = serde_json::json!(true);
        }
        // T-405: the merge op of a mechanically settled attempt says so, so
        // the merge does not require a review branch that never existed.
        if mechanical {
            payload["mechanical"] = serde_json::json!(true);
        }
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
                payload,
                created: now,
            })?;
            Ok(())
        })?;
    }

    // 1b. an accepted attempt that nothing will merge is named (T-397): no
    //     work sits accepted and unmerged without a word. This comes after the
    //     queue step, so a merge that was just queued is a merge in flight.
    //
    //     T-516: the scan reads every done task of the project, so it is not on
    //     every tick (it held a core on a quiet fleet): once every
    //     `LOST_SCAN_EVERY_S`, and at once in the tick after a merge op of the
    //     project reached a final state — a merge that failed or landed can
    //     leave an attempt lost.
    let last_lost = store.read(|tx| tx.get_meta(&lost_scan_key(project)))?;
    let last_lost = last_lost
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    // A merge op of the project that reached Confirmed or Failed since the last
    // scan can leave an attempt lost (`last_settled`, written where it settles).
    let settled = store
        .read(|tx| tx.get_meta(&settled_key(project)))?
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .is_some_and(|at| at >= last_lost);
    if settled || now - last_lost >= LOST_SCAN_EVERY_S {
        store.write(|tx| tx.set_meta(&lost_scan_key(project), &now.to_string()))?;
        // A chain (T-517 by T-614 by T-675) lands one link per pass: repeat
        // while a pass marks something, so the whole chain lands at once.
        for _ in 0..8 {
            if !land_carried(store, project, &config)? {
                break;
            }
        }
        tell_captain_lost(store, project, &config, now)?;
        close_landed_failures(store, project, now)?;
        close_settled_task_items(store, project, now)?;
    }

    // 1c. the task branches on the remote (the branch sweep): a cancelled
    //     task's branches are deleted, and a branch older than the project's
    //     `branch_max_age_min` is named to the captain.
    sweep_branches(store, cfg, project, &config, now)?;

    // A worker survives between ticks: a finished one is collected, and the
    // tick launches nothing else.
    let collected = collect_finished(store, cfg, project, &config)?;
    if let Some(done) = collected {
        merged.extend(done);
        return Ok(merged);
    }
    let mut workers =
        cfg.merges.workers.lock().map_err(|_| {
            GearError::new(ErrorCode::BadTransition, "merge worker registry poisoned")
        })?;

    // One worker per repo. Unknown ops are restart recovery: merge_one
    // resets to the remote before doing any merges, and adopts a completed push.
    let ready = ready_merges(store, project)?;
    let ready = void_carried(store, project, ready, now)?;
    let ready = deps_landed(store, project, ready)?;
    let ready = waits_cleared(store, project, ready)?;
    let ready = merge_order(now_first(store, project, ready)?);
    // The merges a train took (or voided) in this tick.
    let mut taken: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (position, op) in ready.iter().enumerate() {
        if taken.contains(&op.id) {
            continue;
        }
        let op = op.clone();
        let task = op.task.clone().unwrap_or_default();
        // The work of this attempt is already on the integration branch: it
        // landed with another task's merge (design §10.7, row B03-01). The
        // queued merge is voided instead of run — a second gate and push would
        // merge the same work twice.
        let already_merged = store.read(|tx| attempt_already_merged(tx, project, &op))?;
        if already_merged {
            store.write(|tx| void_merged_op(tx, project, &op.id, now))?;
            continue;
        }
        let valid_task = store.read(|tx| {
            Ok(tx
                .get_task(project, &task)?
                .is_some_and(|row| matches!(row.state, TaskState::Verifying | TaskState::Done)))
        })?;
        if !valid_task || still_current(store, project, &op)?.is_err() {
            store.write(|tx| neutrino_void(tx, project, &op.id, now))?;
            // The merge will not happen: its wait is over (T-62).
            resolve_wait(store, project, &task, now)?;
            continue;
        }
        // The repo the work is in: the repo of the task's lane (T-53).
        let repo = repo_of_op(store, project, &op, &config)?;
        // A repo in pull-request mode lands through its pull request
        // (design §11.2): the merge needs the claim's pull request.
        let pr = match pr_claim(store, project, &op, &repo)? {
            PrClaim::Local => None,
            PrClaim::Pr(target) => Some(target),
            // §11.3, row 1: no pull request: the merge fails and says so.
            PrClaim::NoPullRequest => {
                fail_merge(
                    store,
                    project,
                    &op,
                    &format!("no pull request for {task}: the claim PR was never opened"),
                    now,
                )?;
                continue;
            }
            // The claim is opening: the merge waits for it (the op stays
            // queued and runs again on a later tick).
            PrClaim::Pending => continue,
        };
        let key = worker_key(&repo);
        // A merge into this repo's integration branch is already running,
        // for this project or another one: this one waits for it.
        if workers.contains_key(&key) {
            continue;
        }
        // A merge train: the merges behind this one that land on the same
        // branch, each gated on the commit the one ahead of it lands.
        let parallel = config.conventions.merge_parallel.max(1) as usize;
        let single = cfg.merges.single.lock().unwrap().remove(&key);
        // T-574: the merge the captain put first runs alone — never batched,
        // never in a train — so nothing waits behind it and nothing changes
        // under its gate. The mark is one-shot: it is cleared here, when the
        // merge starts.
        let alone = store.read(|tx| {
            Ok(tx
                .get_meta(&gear_app::plan::merge_first_key(project))?
                .is_some_and(|marked| !marked.is_empty() && marked == task))
        })?;
        // A batch merge (T-547): up to `cap` passed merges of this project are
        // merged into one worktree and gated once. `merge_parallel` is not
        // consulted then; a batch of one op takes the normal single path.
        let batch = config.conventions.merge_batch.max(1) as usize;
        if pr.is_none() && batch > 1 && !single && !alone {
            let cap = cfg
                .merges
                .batch_cap
                .lock()
                .unwrap()
                .get(&key)
                .copied()
                .unwrap_or(batch)
                .clamp(1, batch);
            let mut members = vec![op.clone()];
            for later in ready.iter().skip(position + 1) {
                if members.len() >= cap {
                    break;
                }
                if taken.contains(&later.id) {
                    continue;
                }
                match train_member(store, project, later, &config, &key, now)? {
                    Member::Joins => members.push(later.clone()),
                    Member::Voided => {
                        taken.insert(later.id.clone());
                    }
                    Member::Not => {}
                }
            }
            if members.len() > 1 {
                match integration_worktree(store, herdr, &repo, project, &config, &cfg.merges, now)
                {
                    Ok(worktree) => {
                        // T-566: a merge of this project starts: the wait's
                        // item has done its work.
                        close_merge_waits(store, project, now)?;
                        launch_batch(
                            store,
                            cfg,
                            project,
                            &config,
                            &repo,
                            &key,
                            members.clone(),
                            worktree,
                            &mut workers,
                            now,
                        )?;
                        for member in &members {
                            taken.insert(member.id.clone());
                        }
                        continue;
                    }
                    Err(reason) => {
                        tell_captain_merge_wait(store, project, &reason, now)?;
                        return Ok(merged);
                    }
                }
            }
        }
        if pr.is_none() && parallel > 1 && !single && !alone {
            let mut train = vec![op.clone()];
            for later in ready.iter().skip(position + 1) {
                if train.len() >= parallel {
                    break;
                }
                if taken.contains(&later.id) {
                    continue;
                }
                match train_member(store, project, later, &config, &key, now)? {
                    Member::Joins => train.push(later.clone()),
                    Member::Voided => {
                        taken.insert(later.id.clone());
                    }
                    Member::Not => {}
                }
            }
            if train.len() > 1 {
                let mut worktrees = Vec::new();
                for stage in 0..train.len() {
                    match stage_worktree(
                        store,
                        herdr,
                        &repo,
                        project,
                        &config,
                        &cfg.merges,
                        now,
                        stage,
                    ) {
                        Ok(worktree) => {
                            close_merge_waits(store, project, now)?;
                            worktrees.push(worktree)
                        }
                        Err(reason) if stage == 0 => {
                            tell_captain_merge_wait(store, project, &reason, now)?;
                            return Ok(merged);
                        }
                        Err(_) => break,
                    }
                }
                train.truncate(worktrees.len());
                if train.len() > 1 {
                    launch_train(
                        store,
                        cfg,
                        project,
                        &config,
                        &repo,
                        &key,
                        train.clone(),
                        worktrees,
                        &mut workers,
                        now,
                    )?;
                    for member in &train {
                        taken.insert(member.id.clone());
                    }
                    continue;
                }
            }
        }
        let worktree =
            match integration_worktree(store, herdr, &repo, project, &config, &cfg.merges, now) {
                Ok(worktree) => {
                    // T-566: the merge starts, so whatever held it is gone.
                    close_merge_waits(store, project, now)?;
                    worktree
                }
                Err(reason) => {
                    tell_captain_merge_wait(store, project, &reason, now)?;
                    return Ok(merged);
                }
            };
        // A merge op the engine recovered (Unknown) runs again: it is
        // Executing while its worker runs, so the completion writes the
        // Submitted result (design §11.2 step 3).
        if matches!(op.state, OpState::Intended | OpState::Unknown) {
            store.write(|tx| tx.set_op_state(project, &op.id, OpState::Executing, None, now))?;
        }
        if alone {
            // T-574: the captain's line is one-shot; it is used here.
            store.write(|tx| tx.set_meta(&gear_app::plan::merge_first_key(project), ""))?;
        }
        // The other tasks of the project with a candidate: the merge waits for
        // the ones whose work its candidate carries (T-62).
        let others = other_candidates(store, project, &task)?;
        // The branch the review would have been on, when the op names none
        // (T-253): the project's own template.
        let lane = lane_of(store, project, op.task.as_deref())?;
        let review_fallback =
            review_branch(&config.conventions.review_branch, project, &lane, &task);
        // The project's own merge message (T-296): `{project}` and `{task}` do
        // not change between the attempts of one op, so they are filled once
        // here; `{source}` is what the attempt merges, filled by the worker.
        let commit = gear_model::commit::fill(
            &config.conventions.merge_message,
            &[("project", project), ("task", &task)],
        );
        // T-564: what a review commit's subject starts with for this task.
        let prefix = review_prefix(&config, project, &task, &op_attempt(&op));
        let finished = Arc::new(Mutex::new(None));
        let output = finished.clone();
        let job = op.clone();
        let repo = repo.clone();
        // The remote is the project's own (T-262), resolved from the config
        // this tick already holds.
        let remote_name = config.conventions.remote.clone();
        let remote = cfg.remote.clone();
        let job_wait = cfg.merge_retry_wait.clone();
        let progress = cfg.merges.progress.clone();
        let pr = pr.clone();
        let fallback = review_fallback.clone();
        let db_path = cfg.db_path.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("merge-{project}-{}", repo.name))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let lock = match merge_lock(Path::new(&worktree.path)) {
                        Ok(Some(lock)) => lock,
                        Ok(None) => return Report::Busy,
                        Err(reason) => return Report::Failed(Failed::clear(reason)),
                    };
                    let target = Target {
                        name: &remote_name,
                        runner: remote.as_ref(),
                    };
                    match merge_with_retries(
                        &worktree,
                        &repo,
                        &job,
                        &lock,
                        &others,
                        target,
                        &job_wait,
                        pr.as_ref(),
                        &fallback,
                        &commit,
                        &prefix,
                        &db_path,
                    ) {
                        Step::Merged(lines) => Report::Merged(lines),
                        Step::Pr { pr, head } => Report::Pr { pr, head },
                        Step::Waiting(tasks) => Report::Waiting(tasks),
                        Step::Raced(reason) => Report::Raced(reason),
                        Step::Failed(failed) => Report::Failed(failed),
                    }
                }))
                .unwrap_or_else(|_| Report::Failed(Failed::stuck("merge worker panicked")));
                *output.lock().unwrap() = Some(Finished {
                    result,
                    at: wall_time(),
                });
                // A waiter sleeps on this, not on the clock (T-127).
                *progress.0.lock().unwrap() += 1;
                progress.1.notify_all();
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

/// What a merge's result does to its op, its task and the captain's items.
fn settle_report(
    store: &mut Store,
    project: &str,
    config: &ProjectConfig,
    op: &OpRow,
    result: Report,
    at: Ts,
) -> GearResult<Vec<Merged>> {
    let mut merged = Vec::new();
    match result {
        Report::Merged(lines) => {
            store.write(|tx| {
                let current = tx.get_op(project, &op.id)?.unwrap();
                if current.state == OpState::Executing {
                    tx.set_op_state(project, &op.id, OpState::Submitted, None, at)?;
                }
                tx.set_op_state(project, &op.id, OpState::Confirmed, Some(&lines), at)?;
                tx.set_meta(&settled_key(project), &at.to_string())?;
                // T-38's merge time belongs to gate/push completion,
                // independent of both the launching and collecting tick.
                if let (Some(attempt), Some(task)) = (op.attempt, op.task.as_deref()) {
                    tx.set_attempt_time(project, task, attempt, AttemptStep::Merged, at)?;
                }
                Ok(())
            })?;
            merged.push(Merged {
                task: op.task.clone().unwrap_or_default(),
                branch: op.payload["branch"].as_str().unwrap_or("").to_string(),
            });
            // The wait is over and the failure (if any) is settled: the
            // captain's items about them close (T-62, T-92).
            resolve_wait(store, project, &op.task.clone().unwrap_or_default(), at)?;
            resolve_failed(store, project, &op.task.clone().unwrap_or_default(), at)?;
            // A branch the merge had to leave behind holds work that is not
            // on the integration branch: the captain decides, once.
            if let Some(left) = lines.lines().find(|line| line.starts_with("left on ")) {
                let task = op.task.clone().unwrap_or_default();
                let _ = tell_captain(
                    store,
                    project,
                    op.attempt.map(|attempt| (task.as_str(), attempt)),
                    &format!("{task} merged, but a branch stays: {left}. Land what it holds in a task, or delete it."),
                    at,
                );
            }
        }
        Report::Pr { pr, head } => {
            // §11.2 step 3: the merge op waits for the land op, which is
            // the forge's call. Nothing is deleted: the pull request's
            // head is the task's branch until the pull request is merged.
            let result = serde_json::json!({"mode":"pr","pr":pr,"head":head}).to_string();
            let method = merge_method_of(&repo_of_op(store, project, &op, &config)?);
            store.write(|tx| {
                let current = tx.get_op(project, &op.id)?.unwrap();
                if current.state == OpState::Executing {
                    tx.set_op_state(project, &op.id, OpState::Submitted, Some(&result), at)?;
                }
                insert_land_op(tx, project, &op, pr, &head, method, at)
            })?;
        }
        Report::Busy => retry_merge(store, project, op, "merge will retry from the remote", at)?,
        // The merge waits for another task: it is not a failure. The op
        // goes back to Unknown and the captain is told once which task it
        // waits for (T-62).
        Report::Waiting(blocked) => {
            let names: Vec<String> = blocked.iter().map(|(task, _)| task.clone()).collect();
            retry_merge(
                store,
                project,
                op,
                &format!("waits for {}", names.join(", ")),
                at,
            )?;
            tell_captain_waiting(store, project, op, &blocked, at)?;
        }
        // The integration branch moved under every try (T-280): another
        // merge landed each time, so this is not a failure and no
        // captain item. The op goes back to Unknown and runs again from
        // the new tip on the next tick.
        Report::Raced(reason) => {
            let first = reason.lines().next().unwrap_or_default();
            let why = if unreachable(&reason) {
                "the remote could not be read"
            } else {
                "the integration branch moved"
            };
            retry_merge(
                store,
                project,
                op,
                &format!(
                    "{first} — {why} during each of {} tries; \
                     it merges again from the new tip on the next tick",
                    MERGE_RETRIES + 1
                ),
                at,
            )?
        }
        // The worker already retried a reason that can clear (T-61): what
        // comes back is the failure that stands.
        Report::Failed(failed) => fail_merge(store, project, op, &failed.reason, at)?,
        // T-576: a merge a batch or a train sends back is **queued again**, not
        // unknown (Prometheus 2026-11-05: O-7384..O-7390 sat `unknown` with
        // "its batch failed the gate: it is gated again in a smaller batch",
        // no later batch took them, the lost scan called them lost and
        // `- remerge` refused them). Unknown is kept for a push whose outcome
        // is not known; a requeue knows its outcome (nothing was pushed).
        Report::Requeue(reason) => requeue_merge(store, project, op, &reason, at)?,
        Report::Train(results) => {
            for (one, report) in results {
                merged.extend(settle_report(store, project, config, &one, report, at)?);
            }
        }
    }
    Ok(merged)
}

/// Whether a queued merge joins the train of `key`.
enum Member {
    Joins,
    /// The merge will not run: its work is already landed, or its task may
    /// not be merged any more. It was voided as the queue voids it.
    Voided,
    /// It runs on its own turn: another branch, or a pull request.
    Not,
}

/// The checks the queue makes before a merge runs, for a merge behind the
/// head of a train (the same voids, in the same order).
fn train_member(
    store: &mut Store,
    project: &str,
    op: &OpRow,
    config: &ProjectConfig,
    key: &str,
    now: Ts,
) -> GearResult<Member> {
    let task = op.task.clone().unwrap_or_default();
    if store.read(|tx| attempt_already_merged(tx, project, op))? {
        store.write(|tx| void_merged_op(tx, project, &op.id, now))?;
        return Ok(Member::Voided);
    }
    let valid_task = store.read(|tx| {
        Ok(tx
            .get_task(project, &task)?
            .is_some_and(|row| matches!(row.state, TaskState::Verifying | TaskState::Done)))
    })?;
    if !valid_task || still_current(store, project, op)?.is_err() {
        store.write(|tx| neutrino_void(tx, project, &op.id, now))?;
        resolve_wait(store, project, &task, now)?;
        return Ok(Member::Voided);
    }
    let repo = repo_of_op(store, project, op, config)?;
    if worker_key(&repo) != key || pr_claim_is_pr(store, project, op, &repo)? {
        return Ok(Member::Not);
    }
    Ok(Member::Joins)
}

/// Whether the repo of `op` lands through pull requests (a train never does).
fn pr_claim_is_pr(store: &mut Store, project: &str, op: &OpRow, repo: &Repo) -> GearResult<bool> {
    Ok(!matches!(
        pr_claim(store, project, op, repo)?,
        PrClaim::Local
    ))
}

/// Start a merge train's worker: its merges are Executing until it reports.
#[allow(clippy::too_many_arguments)]
fn launch_train(
    store: &mut Store,
    cfg: &EngineCfg,
    project: &str,
    config: &ProjectConfig,
    repo: &Repo,
    key: &str,
    train: Vec<OpRow>,
    worktrees: Vec<gear_herdr::Worktree>,
    workers: &mut BTreeMap<String, Worker>,
    now: Ts,
) -> GearResult<()> {
    let mut members = Vec::new();
    for op in &train {
        let task = op.task.clone().unwrap_or_default();
        if matches!(op.state, OpState::Intended | OpState::Unknown) {
            store.write(|tx| tx.set_op_state(project, &op.id, OpState::Executing, None, now))?;
        }
        let lane = lane_of(store, project, op.task.as_deref())?;
        members.push(TrainOp {
            op: op.clone(),
            others: other_candidates(store, project, &task)?,
            review_fallback: review_branch(
                &config.conventions.review_branch,
                project,
                &lane,
                &task,
            ),
            commit: gear_model::commit::fill(
                &config.conventions.merge_message,
                &[("project", project), ("task", &task)],
            ),
            review_prefix: review_prefix(config, project, &task, &op_attempt(&op)),
        });
    }
    let finished = Arc::new(Mutex::new(None));
    let output = finished.clone();
    let repo = repo.clone();
    let remote_name = config.conventions.remote.clone();
    let remote = cfg.remote.clone();
    let progress = cfg.merges.progress.clone();
    let db_path = cfg.db_path.clone();
    let single_set = cfg.merges.single.clone();
    let single_key = key.to_string();
    let spawned = std::thread::Builder::new()
        .name(format!("merge-train-{project}-{}", repo.name))
        .spawn(move || {
            let single = std::sync::atomic::AtomicBool::new(false);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let target = Target {
                    name: &remote_name,
                    runner: remote.as_ref(),
                };
                run_train(&worktrees, &repo, &members, target, &db_path, &single)
            }))
            .unwrap_or_else(|_| {
                members
                    .iter()
                    .map(|m| (m.op.clone(), Report::Requeue("merge train panicked".into())))
                    .collect()
            });
            if single.load(std::sync::atomic::Ordering::SeqCst) {
                single_set.lock().unwrap().insert(single_key);
            }
            *output.lock().unwrap() = Some(Finished {
                result: Report::Train(result),
                at: wall_time(),
            });
            *progress.0.lock().unwrap() += 1;
            progress.1.notify_all();
        });
    let head = train[0].clone();
    match spawned {
        Ok(_) => {
            workers.insert(key.to_string(), Worker { op: head, finished });
        }
        Err(error) => {
            for op in &train {
                retry_merge(
                    store,
                    project,
                    op,
                    &format!("start merge train: {error}"),
                    wall_time(),
                )?;
            }
        }
    }
    Ok(())
}

/// Start a batch merge's worker (T-547): its merges are Executing until it
/// reports. One worktree and one gate for the whole batch; the cap the batch
/// ends with is kept for this repo's next one.
#[allow(clippy::too_many_arguments)]
fn launch_batch(
    store: &mut Store,
    cfg: &EngineCfg,
    project: &str,
    config: &ProjectConfig,
    repo: &Repo,
    key: &str,
    batch: Vec<OpRow>,
    worktree: gear_herdr::Worktree,
    workers: &mut BTreeMap<String, Worker>,
    now: Ts,
) -> GearResult<()> {
    let mut members = Vec::new();
    for op in &batch {
        let task = op.task.clone().unwrap_or_default();
        if matches!(op.state, OpState::Intended | OpState::Unknown) {
            store.write(|tx| tx.set_op_state(project, &op.id, OpState::Executing, None, now))?;
        }
        let lane = lane_of(store, project, op.task.as_deref())?;
        let commit = gear_model::commit::fill(
            &config.conventions.merge_message,
            &[("project", project), ("task", &task)],
        );
        members.push(TrainOp {
            op: op.clone(),
            others: other_candidates(store, project, &task)?,
            review_fallback: review_branch(
                &config.conventions.review_branch,
                project,
                &lane,
                &task,
            ),
            review_prefix: review_prefix(config, project, &task, &op_attempt(op)),
            commit,
        });
    }
    let finished = Arc::new(Mutex::new(None));
    let output = finished.clone();
    let repo = repo.clone();
    let remote_name = config.conventions.remote.clone();
    let remote = cfg.remote.clone();
    let progress = cfg.merges.progress.clone();
    let db_path = cfg.db_path.clone();
    let single_set = cfg.merges.single.clone();
    let single_key = key.to_string();
    let cap_set = cfg.merges.batch_cap.clone();
    let batch_size = config.conventions.merge_batch.max(1) as usize;
    let cap = {
        let caps = cfg.merges.batch_cap.lock().unwrap();
        caps.get(key).copied().unwrap_or(batch_size)
    };
    let spawned = std::thread::Builder::new()
        .name(format!("merge-batch-{project}-{}", repo.name))
        .spawn(move || {
            let single = std::sync::atomic::AtomicBool::new(false);
            let (result, cap) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let target = Target {
                    name: &remote_name,
                    runner: remote.as_ref(),
                };
                run_batch(
                    &worktree, &repo, &members, target, &db_path, &single, batch_size, cap,
                )
            }))
            .unwrap_or_else(|_| {
                let results: Vec<(OpRow, Report)> = members
                    .iter()
                    .map(|m| (m.op.clone(), Report::Requeue("merge batch panicked".into())))
                    .collect();
                (results, cap)
            });
            if single.load(std::sync::atomic::Ordering::SeqCst) {
                single_set.lock().unwrap().insert(single_key.clone());
            }
            cap_set.lock().unwrap().insert(single_key, cap);
            *output.lock().unwrap() = Some(Finished {
                result: Report::Train(result),
                at: wall_time(),
            });
            *progress.0.lock().unwrap() += 1;
            progress.1.notify_all();
        });
    let head = batch[0].clone();
    match spawned {
        Ok(_) => {
            workers.insert(key.to_string(), Worker { op: head, finished });
        }
        Err(error) => {
            for op in &batch {
                retry_merge(
                    store,
                    project,
                    op,
                    &format!("start merge batch: {error}"),
                    wall_time(),
                )?;
            }
        }
    }
    Ok(())
}

/// A lock in the worktree's git directory also excludes another runtime or a
/// recovering engine. OS locks are released on process death; stale files are
/// harmless. Never wait for a lock on the tick thread.
pub(crate) fn merge_lock(dir: &Path) -> Result<Option<File>, String> {
    let path = merge_lock_path(dir);
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

/// The lock file's path: `<git dir>/gear-merge.lock`, read from the worktree's
/// own `.git` rather than asked of git (T-127: one git process fewer on every
/// merge attempt, including every attempt of a merge that waits).
/// `git rev-parse --git-path gear-merge.lock` answers the same path, and the
/// test `the_merge_lock_is_the_file_git_names` holds the two together. A
/// `.git` this cannot read, or a `GIT_DIR` in the environment, falls back to
/// git itself.
fn merge_lock_path(dir: &Path) -> std::path::PathBuf {
    if std::env::var_os("GIT_DIR").is_none() {
        if let Some(git_dir) = git_dir(dir) {
            return git_dir.join("gear-merge.lock");
        }
    }
    match git(dir, &["rev-parse", "--git-path", "gear-merge.lock"]) {
        Ok(path) => dir.join(path.trim()),
        Err(_) => dir.join("gear-merge.lock"),
    }
}

/// The git directory of the checkout at `dir`: `.git` itself in a plain repo,
/// the path a worktree's `.git` file names in a linked one.
fn git_dir(dir: &Path) -> Option<std::path::PathBuf> {
    let dot_git = dir.join(".git");
    if std::fs::metadata(&dot_git).ok()?.is_dir() {
        return Some(dot_git);
    }
    let file = std::fs::read_to_string(&dot_git).ok()?;
    let named = file.lines().next()?.strip_prefix("gitdir:")?.trim();
    let named = Path::new(named);
    Some(if named.is_absolute() {
        named.to_path_buf()
    } else {
        dir.join(named)
    })
}

/// Put a merge op back in the queue: it runs again on a later tick. The reason
/// says why it did not go through.
/// Put a merge op back in the queue: it runs again on a later tick. The reason
/// says why it did not go through. An op that already waits (Unknown) is
/// refreshed too: the wait's time is what orders the waiting merges fairly
/// (T-62), and a reason that changed must not stay stale.
fn retry_merge(
    store: &mut Store,
    project: &str,
    op: &OpRow,
    reason: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        if tx
            .get_op(project, &op.id)?
            .is_some_and(|row| matches!(row.state, OpState::Executing | OpState::Unknown))
        {
            tx.set_op_state(project, &op.id, OpState::Unknown, Some(reason), now)?;
        }
        Ok(())
    })
}

/// Queue a merge again that a batch or a train sent back (T-576): the op goes
/// back to `intended` with the reason, so the queue takes it in the next round.
fn requeue_merge(
    store: &mut Store,
    project: &str,
    op: &OpRow,
    reason: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        if tx.get_op(project, &op.id)?.is_some_and(|row| {
            matches!(
                row.state,
                OpState::Intended | OpState::Executing | OpState::Unknown
            )
        }) {
            tx.set_op_state(project, &op.id, OpState::Intended, Some(reason), now)?;
        }
        Ok(())
    })
}

/// The reason an op that waits for another task carries (T-62). It is the
/// op's explanation, so the engine can tell a waiting merge from one that
/// waits for the next tick.
const WAITS: &str = "waits for ";

/// The tasks a merge op waits for, if it waits (T-62).
fn waits_for(op: &OpRow) -> Option<&str> {
    op.result.as_deref()?.strip_prefix(WAITS)
}

/// A waiting merge whose tasks have all merged since is waiting no longer: it
/// goes back into the ordinary queue (Prometheus T-482, 2026-11-03: it waited
/// for T-481 nine hours after T-481 landed, because the busy merge line never
/// reached the waiting group to look again). Only the order changes here; the
/// merge itself checks what its candidate carries before it gates.
fn waits_cleared(store: &mut Store, project: &str, ready: Vec<OpRow>) -> GearResult<Vec<OpRow>> {
    store.read(|tx| {
        let mut out = Vec::with_capacity(ready.len());
        for mut op in ready {
            if let Some(list) = waits_for(&op) {
                let mut all_landed = true;
                for task in list.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                    let landed = gear_app::tasks::merged_at(tx, project, task)?.is_some()
                        || tx
                            .get_task(project, task)?
                            .is_some_and(|row| row.state == TaskState::Cancelled);
                    if !landed {
                        all_landed = false;
                        break;
                    }
                }
                if all_landed {
                    op.result = None;
                }
            }
            out.push(op);
        }
        Ok(out)
    })
}

/// The merges of tasks the plan marks `now` run before the others, each group
/// in its queue order (Prometheus, 2026-11-03: the user's top-priority test
/// gate waited behind older ordinary merges on the shared merge line).
/// The ready merges in the order they run (T-128, T-574): the captain's
/// `- merge-first` task first, then the `now` tasks, then the rest.
fn now_first(store: &mut Store, project: &str, ready: Vec<OpRow>) -> GearResult<Vec<OpRow>> {
    store.read(|tx| {
        // T-574: the one merge the captain put first runs before every other,
        // `now` ones included.
        let first = tx
            .get_meta(&gear_app::plan::merge_first_key(project))?
            .unwrap_or_default();
        let mut marked = Vec::new();
        let mut now = Vec::new();
        let mut rest = Vec::new();
        for op in ready {
            if !first.is_empty() && op.task.as_deref() == Some(first.as_str()) {
                marked.push(op);
                continue;
            }
            let urgent = match op.task.as_deref() {
                Some(task) => tx
                    .get_task(project, task)?
                    .is_some_and(|row| row.priority == "now"),
                None => false,
            };
            if urgent {
                now.push(op);
            } else {
                rest.push(op);
            }
        }
        marked.extend(now);
        marked.extend(rest);
        Ok(marked)
    })
}

/// The ready merges in the order they run: a merge that waits for another task
/// goes last (T-62). The task it waits for must be able to land first, or the
/// two would hold each other.
///
/// Among the waiting merges the one that has waited longest goes first: its
/// `updated` moves on every retry, so the group is a round robin and no waiting
/// task starves behind another (F2 of the T-62 review).
fn merge_order(ready: Vec<OpRow>) -> Vec<OpRow> {
    let (mut waiting, running): (Vec<OpRow>, Vec<OpRow>) =
        ready.into_iter().partition(|op| waits_for(op).is_some());
    waiting.sort_by_key(|op| op.updated);
    running.into_iter().chain(waiting).collect()
}

/// A push rejected because the integration branch moved meanwhile.
const RACE: &str = "push race: ";
/// A fetch of the remote that failed.
const FETCH: &str = "fetch remote failed: ";
/// How many times a merge that failed for a reason that can clear is retried
/// before it fails (T-61).
const MERGE_RETRIES: u32 = 3;
/// The wait before the first retry. It doubles after every attempt; the
/// engine's [`RetryWait`] is what sleeps it (T-127).
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
        Failed {
            reason: reason.into(),
            blocker: Blocker::Clear,
        }
    }

    /// A reason that stands: the captain is rung.
    fn stuck(reason: impl Into<String>) -> Failed {
        Failed {
            reason: reason.into(),
            blocker: Blocker::Stuck,
        }
    }
}

/// What one merge attempt did.
#[derive(Debug, Clone)]
enum Step {
    /// Merged and pushed: the result line.
    Merged(String),
    /// The gate passed and the pull request's head is pushed (design §11.2
    /// step 2): the pull request's number and the head the gate checked.
    Pr { pr: u64, head: String },
    /// The merge waits: the candidate carries the work of other tasks that
    /// have not landed yet. Each task is named with whether it has passed.
    Waiting(Vec<(String, bool)>),
    /// The merge lost the push race on every try (T-280): the integration
    /// branch moved under each attempt.
    Raced(String),
    /// The merge failed.
    Failed(Failed),
}

/// Run the merge and retry it while the reason can clear. The wait grows after
/// every attempt; after the last one the failure stands and the captain hears
/// about it once (T-61).
fn merge_with_retries(
    worktree: &gear_herdr::Worktree,
    repo: &Repo,
    op: &OpRow,
    lock: &File,
    others: &[(String, String, bool)],
    remote: Target<'_>,
    wait: &RetryWait,
    pr: Option<&PrTarget>,
    review_fallback: &str,
    commit: &str,
    review_prefix: &str,
    db_path: &str,
) -> Step {
    let mut attempt = 0;
    loop {
        // T-407: each attempt logs its steps from the op's durable intent, and
        // keeps their boundaries in the op's meta.
        let mut probe = Probe::new(
            op.task.as_deref().unwrap_or("-"),
            op.created,
            db_path,
            &op.id,
        );
        match merge_one_locked(
            worktree,
            repo,
            op,
            lock,
            others,
            remote,
            pr,
            review_fallback,
            commit,
            review_prefix,
            &mut probe,
        ) {
            Step::Merged(lines) => {
                probe.done();
                return Step::Merged(lines);
            }
            Step::Pr { pr, head } => {
                probe.done();
                return Step::Pr { pr, head };
            }
            Step::Waiting(tasks) => return Step::Waiting(tasks),
            Step::Raced(reason) => {
                probe.done();
                return Step::Raced(reason);
            }
            Step::Failed(mut failed) => {
                if failed.blocker != Blocker::Clear || attempt >= MERGE_RETRIES {
                    if attempt > 0 {
                        failed.reason = format!(
                            "{}\n(retried {attempt} time(s), each with a growing wait)",
                            failed.reason
                        );
                    }
                    // A push race that ran through every retry is not a
                    // failure (T-280): the remote moved each time, so the
                    // merge goes back to the queue. A "cannot lock ref"
                    // failure is not that — it can be a stale local lock —
                    // and PR mode keeps its Failed end (T-280 §3).
                    // A remote that could not be read through every retry
                    // (the network or the host refused) says nothing about
                    // the work: it goes back to the queue too (2026-11-04,
                    // GitHub refused SSH for minutes and T-675 failed).
                    if pr.is_none()
                        && (lost_the_race(&failed.reason) || unreachable(&failed.reason))
                    {
                        probe.done();
                        return Step::Raced(failed.reason);
                    }
                    probe.done();
                    return Step::Failed(failed);
                }
                attempt += 1;
                // The wait is the config's: the engine sleeps on the clock,
                // the tests' hook does not (T-127).
                wait.wait(attempt);
            }
        }
    }
}

/// The wait before the next attempt: `RETRY_BACKOFF_S` doubled after every
/// attempt, up to 30 s. Only [`RetryWait::backoff`] uses it: the tests never
/// sleep (T-127).
fn backoff(attempt: u32) -> std::time::Duration {
    let shift = attempt.saturating_sub(1).min(6);
    let seconds = RETRY_BACKOFF_S.saturating_mul(1u64 << shift);
    std::time::Duration::from_secs(seconds.min(RETRY_BACKOFF_MAX_S))
}

/// Fail a merge op once and tell the captain once.
fn fail_merge(
    store: &mut Store,
    project: &str,
    op: &OpRow,
    reason: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        let from = tx
            .get_op(project, &op.id)?
            .map(|op| op.state)
            .unwrap_or(OpState::Intended);
        if from == OpState::Intended {
            tx.set_op_state(project, &op.id, OpState::Executing, None, now)?;
        }
        // Unknown from a restart goes straight to Failed.
        tx.set_op_state(project, &op.id, OpState::Failed, Some(reason), now)?;
        tx.set_meta(&settled_key(project), &now.to_string())
    })?;
    tell_captain_failed(store, project, op, reason, now)
}

/// The one place a merge touches the remote (T-107): the push of the
/// integration branch and the delete of the branches it consumed.
///
/// Production is [`GitRemote`]. The engine's tests replace it with
/// [`FetchRemote`], which moves the refs from the remote's own side, so no test
/// in this crate runs `git push`. Every call names the remote (T-262): the
/// project's `[conventions].remote`, which a team may call `upstream`.
pub trait Remote: Send + Sync + std::fmt::Debug {
    /// Push `dir`'s HEAD to `dst` on `dir`'s `remote`.
    fn push(&self, dir: &Path, remote: &str, dst: &str) -> Result<(), String>;
    /// Delete `branch` on `dir`'s `remote`; a branch already gone counts as
    /// deleted.
    fn delete(&self, dir: &Path, remote: &str, branch: &str) -> bool;
}

/// The remote a merge talks to: the project's `[conventions].remote` and the
/// runner that touches it (T-107, T-262). The name comes from the config, the
/// runner from the engine's own config.
#[derive(Debug, Clone, Copy)]
struct Target<'a> {
    name: &'a str,
    runner: &'a dyn Remote,
}

impl<'a> Target<'a> {
    /// The remote of `config`, touched by `runner`.
    fn of(config: &'a ProjectConfig, runner: &'a dyn Remote) -> Target<'a> {
        Target {
            name: &config.conventions.remote,
            runner,
        }
    }

    fn push(&self, dir: &Path, dst: &str) -> Result<(), String> {
        self.runner.push(dir, self.name, dst)
    }

    fn delete(&self, dir: &Path, branch: &str) -> bool {
        self.runner.delete(dir, self.name, branch)
    }
}

/// The wait between two attempts of a merge that failed for a reason that can
/// clear (T-61). Time is injected (T-127): the engine's own wait sleeps,
/// doubling from 2 s up to 30 s; the engine's tests hand in a hook that never
/// sleeps on the clock — it can also do something instead, such as the lane's
/// push that arrives between two attempts.
#[derive(Clone)]
pub struct RetryWait(std::sync::Arc<dyn Fn(u32) + Send + Sync>);

impl RetryWait {
    /// The engine's wait: `attempt` 1 waits `RETRY_BACKOFF_S`, every later
    /// attempt twice as long, up to 30 s.
    pub fn backoff() -> RetryWait {
        RetryWait(std::sync::Arc::new(|attempt| {
            std::thread::sleep(backoff(attempt))
        }))
    }

    /// No wait at all: what a test's config uses.
    pub fn immediate() -> RetryWait {
        RetryWait(std::sync::Arc::new(|_| ()))
    }

    /// Do `then` instead of waiting, with the attempt about to run.
    pub fn instead(then: impl Fn(u32) + Send + Sync + 'static) -> RetryWait {
        RetryWait(std::sync::Arc::new(then))
    }

    /// Wait before attempt `attempt` (1 for the first retry).
    pub fn wait(&self, attempt: u32) {
        (self.0)(attempt)
    }
}

impl std::fmt::Debug for RetryWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetryWait")
    }
}

impl Default for RetryWait {
    fn default() -> Self {
        RetryWait::backoff()
    }
}

/// The production runner: `git push`, exactly as the merge always did.
#[derive(Debug, Clone, Copy, Default)]
pub struct GitRemote;

impl Remote for GitRemote {
    fn push(&self, dir: &Path, remote: &str, dst: &str) -> Result<(), String> {
        git(dir, &["push", remote, &format!("HEAD:{dst}")]).map(|_| ())
    }

    fn delete(&self, dir: &Path, remote: &str, branch: &str) -> bool {
        git(dir, &["push", remote, "--delete", branch]).is_ok()
    }
}

/// The runner the engine's tests use (T-107): no `git push` at all. It learns
/// the origin's path from the worktree and moves the ref from the remote's own
/// side — `git -C <origin> fetch <dir> HEAD:refs/heads/<dst>` — and deletes
/// with `update-ref -d`. The fetch refuses a non-fast-forward update exactly
/// like a push, so the race tests still race.
#[derive(Debug, Clone, Copy, Default)]
pub struct FetchRemote;

impl Remote for FetchRemote {
    fn push(&self, dir: &Path, remote: &str, dst: &str) -> Result<(), String> {
        let origin = origin_path(dir, remote)?;
        let work = std::fs::canonicalize(dir)
            .map_err(|e| format!("fetch remote: {}: {e}", dir.display()))?;
        git(
            dir,
            &[
                "-C",
                &origin.to_string_lossy(),
                "fetch",
                &work.to_string_lossy(),
                &format!("HEAD:refs/heads/{dst}"),
            ],
        )?;
        Ok(())
    }

    fn delete(&self, dir: &Path, remote: &str, branch: &str) -> bool {
        let Ok(origin) = origin_path(dir, remote) else {
            return false;
        };
        git(
            dir,
            &[
                "-C",
                &origin.to_string_lossy(),
                "update-ref",
                "-d",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok()
    }
}

/// The remote `remote` of `dir` as a path: a local bare repo in the engine's
/// tests.
fn origin_path(dir: &Path, remote: &str) -> Result<std::path::PathBuf, String> {
    // Remembered per checkout and remote (T-127): a merge asks for the remote
    // once per branch it pushes and closes, and a checkout's remote does not
    // change while a test runs. Only the tests' runner asks this; the
    // production runner lets `git push` resolve the same remote itself.
    static KNOWN: std::sync::OnceLock<
        Mutex<BTreeMap<(std::path::PathBuf, String), std::path::PathBuf>>,
    > = std::sync::OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    let key = (dir.to_path_buf(), remote.to_string());
    if let Some(path) = known.lock().unwrap().get(&key).cloned() {
        return Ok(path);
    }
    let url = git(dir, &["remote", "get-url", remote])?;
    let url = url.trim();
    let path = Path::new(url);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        dir.join(path)
    };
    known.lock().unwrap().insert(key, path.clone());
    Ok(path)
}

/// The thread of the item that says a merge failed (T-92). One item per task:
/// the captain is told once per failure and it closes when the merge goes
/// through.
fn failed_thread(task: &str) -> String {
    format!("merge-failed:{task}")
}

/// Tell the captain that a merge failed, naming the plan line that retries it
/// (T-92). One open item per task: the same failure again changes nothing, a
/// new failure replaces the open item with a fresh one that rings. The
/// item names the task and attempt it is about, so the captain's home page can
/// suggest the verdict that settles it (T-90).
fn tell_captain_failed(
    store: &mut Store,
    project: &str,
    op: &OpRow,
    reason: &str,
    now: Ts,
) -> GearResult<()> {
    let task = op.task.clone().unwrap_or_default();
    if task.is_empty() {
        return tell_captain(
            store,
            project,
            None,
            &format!("merge failed: {reason}\nnothing was pushed"),
            now,
        );
    }
    let body = format!(
        "merge of {task} failed: {reason}\nnothing was pushed\n\
         to retry it: `- remerge {task}` under `# Staff` in the plan, then `gear plan apply` \
         (only while the candidate is not on the integration branch)"
    );
    store.write(|tx| {
        let captain = tx.captain_of(project)?.ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!("project {project} has no captain"),
            )
        })?;
        let thread = failed_thread(&task);
        let open = tx
            .thread_messages(project, &thread)?
            .into_iter()
            .find(|message| message.state == MsgState::Open);
        if let Some(item) = open {
            if item.body == body {
                return Ok(());
            }
            // A new failure is new news: the captain may have seen the old
            // item already, and a seen item does not ring again. Close it and
            // open a fresh one so the doorbell names it.
            tx.update_message_state(
                project,
                &item.id,
                MsgState::Resolved,
                Some("a newer failure of this merge replaced it"),
                None,
                None,
                now,
            )?;
        }
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.to_string(),
            thread,
            task: Some(task.clone()),
            attempt: op.attempt,
            sender: captain.id.clone(),
            sender_gen: 1,
            recipient: captain.id,
            kind: MsgKind::Question,
            urgent: false,
            body,
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        Ok(())
    })
}

/// Close the item that says a merge failed (T-92): the merge went through, by
/// a retry or otherwise.
fn resolve_failed(store: &mut Store, project: &str, task: &str, now: Ts) -> GearResult<()> {
    if task.is_empty() {
        return Ok(());
    }
    store.write(|tx| {
        let thread = failed_thread(task);
        for item in tx.thread_messages(project, &thread)? {
            if item.state == MsgState::Open {
                tx.update_message_state(
                    project,
                    &item.id,
                    MsgState::Resolved,
                    Some("the merge went through"),
                    None,
                    None,
                    now,
                )?;
            }
        }
        Ok(())
    })
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
            return Ok(Err(format!("a newer attempt {} was handed in", latest.n)));
        }
        let candidate = op.payload["candidate"].as_str().unwrap_or("");
        if !latest
            .candidate
            .as_deref()
            .is_some_and(|stored| gear_app::tasks::same_candidate(stored, candidate))
        {
            return Ok(Err("the attempt's candidate changed".to_string()));
        }
        Ok(Ok(()))
    })
}

/// Void a queued merge because its task may not be merged any more.
fn neutrino_void(tx: &gear_store::Tx<'_>, project: &str, id: &str, now: Ts) -> GearResult<()> {
    let from = tx
        .get_op(project, id)?
        .map(|op| op.state)
        .unwrap_or(OpState::Intended);
    match from {
        OpState::Intended => tx.set_op_state(
            project,
            id,
            OpState::Void,
            Some("the task may not be merged"),
            now,
        )?,
        OpState::Unknown => {
            tx.set_op_state(
                project,
                id,
                OpState::Failed,
                Some("the task may not be merged"),
                now,
            )?;
            tx.set_meta(&settled_key(project), &now.to_string())?;
        }
        _ => {}
    }
    Ok(())
}

/// Void a queued merge whose work is already on the integration branch
/// (design §10.7, row B03-01): the attempt carries the merge time of the
/// merge that landed it, so this merge has nothing left to do.
///
/// A merge that is already running (Executing) is left alone: its worker
/// finishes on its own and the completion is kept.
fn void_merged_op(tx: &gear_store::Tx<'_>, project: &str, id: &str, now: Ts) -> GearResult<()> {
    void_op(tx, project, id, "the work is already merged", now)
}

/// A queued merge of a task that is carried by another (`carried-by:` set
/// after the merge was queued) never runs on its own: its work lands with the
/// carrier's merge. The op is closed, and the task's waits with it
/// (Prometheus T-574/T-577, 2026-11-03).
fn void_carried(
    store: &mut Store,
    project: &str,
    ready: Vec<OpRow>,
    now: Ts,
) -> GearResult<Vec<OpRow>> {
    let mut keep = Vec::with_capacity(ready.len());
    for op in ready {
        let carried = match op.task.as_deref() {
            Some(task) => store.read(|tx| tx.task_carried_by(project, task))?,
            None => None,
        };
        let Some(by) = carried else {
            keep.push(op);
            continue;
        };
        let reason = format!("the work lands with {by}'s merge (carried-by)");
        store.write(|tx| void_op(tx, project, &op.id, &reason, now))?;
        if let Some(task) = op.task.as_deref() {
            resolve_wait(store, project, task, now)?;
        }
    }
    Ok(keep)
}

/// A merge waits while one of its task's `deps:` has not landed: the captain
/// keeps an accepted task out of the merge line until the fix it needs is on
/// the integration branch (Prometheus 2026-11-05: T-748 refused three rounds,
/// and their batchmates with it, for a cause T-762 fixes). A cancelled dep, or
/// one of another project, does not hold it.
fn deps_landed(store: &mut Store, project: &str, ready: Vec<OpRow>) -> GearResult<Vec<OpRow>> {
    store.read(|tx| {
        let mut keep = Vec::with_capacity(ready.len());
        for op in ready {
            let Some(task) = op.task.as_deref() else {
                keep.push(op);
                continue;
            };
            let Some(row) = tx.get_task(project, task)? else {
                keep.push(op);
                continue;
            };
            let mut waits = false;
            for dep in &row.deps {
                if dep.contains(':') {
                    continue;
                }
                let cancelled = tx
                    .get_task(project, dep)?
                    .is_none_or(|dep| dep.state == TaskState::Cancelled);
                if !cancelled && !gear_app::tasks::work_landed(tx, project, dep)? {
                    waits = true;
                    break;
                }
            }
            if !waits {
                keep.push(op);
            }
        }
        Ok(keep)
    })
}

/// Close a queued merge op that will not run: Intended becomes Void, Unknown
/// (a restart's leftover) becomes Failed with the reason.
fn void_op(
    tx: &gear_store::Tx<'_>,
    project: &str,
    id: &str,
    reason: &str,
    now: Ts,
) -> GearResult<()> {
    let from = tx
        .get_op(project, id)?
        .map(|op| op.state)
        .unwrap_or(OpState::Intended);
    match from {
        OpState::Intended => tx.set_op_state(project, id, OpState::Void, Some(reason), now)?,
        OpState::Unknown => {
            tx.set_op_state(project, id, OpState::Failed, Some(reason), now)?;
            tx.set_meta(&settled_key(project), &now.to_string())?;
        }
        _ => {}
    }
    Ok(())
}

/// Has the attempt of this merge op already landed on the integration branch?
/// (design §10.7, row B03-01: the attempt carries the merge time of the
/// merge that landed it.)
fn attempt_already_merged(tx: &gear_store::Tx<'_>, project: &str, op: &OpRow) -> GearResult<bool> {
    let (Some(task), Some(attempt)) = (op.task.as_deref(), op.attempt) else {
        return Ok(false);
    };
    Ok(tx.attempt_times(project, task, attempt)?.merged.is_some())
}

/// The tasks of the project whose **current** attempt has a passing verdict:
/// (task, lane, candidate, attempt, the attempt's candidate, whether the pass
/// was the engine's mechanical one).
///
/// An attempt that already carries a merge time is left out (design §10.7, row
/// B03-01): its work is on the integration branch — it landed with another
/// task's merge — so the engine must not merge it again.
fn passed_tasks(
    store: &mut Store,
    project: &str,
) -> GearResult<Vec<(String, String, String, i64, String, bool)>> {
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
            // The work is already on the integration branch (it landed with
            // another task's merge): no merge of its own.
            if tx
                .attempt_times(project, &task.id, latest.n)?
                .merged
                .is_some()
            {
                continue;
            }
            // `carried-by: T-m`: the work lands with T-m's merge, never alone.
            if tx.task_carried_by(project, &task.id)?.is_some() {
                continue;
            }
            let Some(verdict) = tx
                .verdicts_of_task(project, &task.id)?
                .into_iter()
                .rev()
                .find(|verdict| {
                    verdict.passed
                        && verdict.attempt == latest.n
                        && verdict.contract_version == task.contract_version
                        && latest.candidate.as_deref().is_some_and(|stored| {
                            gear_app::tasks::same_candidate(&verdict.candidate, stored)
                        })
                })
            else {
                continue;
            };
            // T-405: a mechanical pass is the engine's own verdict (T-273),
            // and its note says so: such a task has no reviewer, so no review
            // branch, and its merge must not wait for one.
            let mechanical = settled_mechanically(&verdict);
            passed.push((
                task.id.clone(),
                lane,
                verdict.candidate,
                verdict.attempt,
                latest.candidate.clone().unwrap_or_default(),
                mechanical,
            ));
        }
        Ok(passed)
    })
}

/// T-405: whether the pass is the engine's own mechanical settlement (T-273)
/// rather than a reviewer's or the captain's: the verdict whose note starts
/// `mechanical:`. Such a task never had a reviewer, so it never has a review
/// branch, and its merge must not require one.
fn settled_mechanically(verdict: &gear_store::VerdictRow) -> bool {
    verdict.passed
        && verdict.by_seat == "engine"
        && verdict
            .evidence
            .as_deref()
            .is_some_and(|note| note.starts_with("mechanical:"))
}

/// The other tasks of the project with a candidate: (task, candidate, whether
/// the task's latest attempt has a passing verdict), **one entry per attempt**.
/// A merge waits for the ones whose work its candidate carries (T-62).
///
/// Every attempt counts, not only the latest (F1 of the T-62 review): a task
/// that reworked hands in a new candidate, and the task built on the older
/// branch carries that older, rejected work. Its verdict was rejected, but its
/// commits are still in the branch.
/// Whether `row`'s `carried-by:` chain, within `project`, reaches `task`.
fn carried_into(tx: &gear_store::Tx<'_>, project: &str, row: &str, task: &str) -> GearResult<bool> {
    let mut at = row.to_string();
    // A chain longer than the project's tasks is a loop.
    for _ in 0..64 {
        let Some(by) = tx.task_carried_by(project, &at)? else {
            return Ok(false);
        };
        let (carrier_project, carrier) = gear_store::Tx::carrier_of(project, &by);
        if carrier_project != project {
            return Ok(false);
        }
        if carrier == task {
            return Ok(true);
        }
        at = carrier.to_string();
    }
    Ok(false)
}

fn other_candidates(
    store: &mut Store,
    project: &str,
    task: &str,
) -> GearResult<Vec<(String, String, bool)>> {
    store.read(|tx| {
        // The state of each task's latest merge op (by creation, then number).
        let mut last_merge: std::collections::BTreeMap<String, (Ts, u64, OpState)> =
            std::collections::BTreeMap::new();
        for op in tx.ops_of_kind_in_states(project, OpKind::Merge, &[])? {
            let Some(of) = op.task.clone() else {
                continue;
            };
            let number = op
                .id
                .strip_prefix("O-")
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0);
            let entry = (op.created, number, op.state);
            if last_merge
                .get(&of)
                .is_none_or(|seen| (seen.0, seen.1) < (entry.0, entry.1))
            {
                last_merge.insert(of, entry);
            }
        }
        let mut others = Vec::new();
        for row in tx.tasks_of_project(project)? {
            if row.id == task {
                continue;
            }
            // A cancelled task never lands: work of it that this candidate
            // carries is this candidate's own, never a task to wait for.
            if row.state == gear_model::TaskState::Cancelled {
                continue;
            }
            // A task carried by this one lands with this merge, and so does a
            // task carried by a task this one carries (T-517 by T-614 by
            // T-675: the merge of T-675 waited for T-517, which waited for it).
            if carried_into(tx, project, &row.id, task)? {
                continue;
            }
            let attempts = tx.attempts_of_task(project, &row.id)?;
            let Some(latest) = attempts.last() else {
                continue;
            };
            let passed = tx
                .verdicts_of_task(project, &row.id)?
                .into_iter()
                .rev()
                .any(|verdict| {
                    verdict.passed
                        && verdict.attempt == latest.n
                        && verdict.contract_version == row.contract_version
                        && latest.candidate.as_deref().is_some_and(|stored| {
                            gear_app::tasks::same_candidate(&verdict.candidate, stored)
                        })
                });
            // A reviewed task whose own merge failed cannot land alone: a
            // candidate that carries it (with the fix) is the way it lands,
            // and that candidate's gate runs on both. Waiting for it would
            // deadlock the two merges.
            if passed
                && last_merge
                    .get(&row.id)
                    .is_some_and(|(_, _, state)| *state == OpState::Failed)
            {
                continue;
            }
            for attempt in attempts {
                if let Some(candidate) = attempt.candidate {
                    others.push((row.id.clone(), candidate, passed));
                }
            }
        }
        Ok(others)
    })
}

/// The pull-request merges that landed (design §11.2 step 5): the merge op
/// waits for the land op, and the land op's answer is the merge.
///
/// The merge op is `Submitted` while the forge merges the pull request. When
/// the land op is confirmed, the merge op is confirmed too (with the merge
/// commit the pull request names), the task's branches are closed, and the
/// waits and failures of the task are settled. When the land op failed, the
/// merge op fails with the forge's own message and the captain hears about it
/// (design §11.3, row 2).
fn confirm_pr_merges(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    project: &str,
    config: &ProjectConfig,
    now: Ts,
) -> GearResult<Vec<Merged>> {
    let mut merged = Vec::new();
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    let mut ops = Vec::new();
    for seat in &seats {
        ops.extend(store.read(|tx| tx.ops_of_seat_kind(&seat.id, OpKind::Merge))?);
    }
    for op in ops {
        if op.state != OpState::Submitted {
            continue;
        }
        // Only a pull-request merge waits for a land op.
        let Some(pr) = pr_number_of(op.result.as_deref()) else {
            continue;
        };
        let task = op.task.clone().unwrap_or_default();
        let Some(land) = land_op_of(store, project, &task)? else {
            continue;
        };
        match land.state {
            OpState::Confirmed => {
                let commit = merge_commit_of(land.result.as_deref()).unwrap_or_default();
                let repo = repo_of_op(store, project, &op, config)?;
                let line = format!(
                    "merged {task} into {} by PR #{pr}: {commit}",
                    repo.integration
                );
                store.write(|tx| {
                    tx.set_op_state(project, &op.id, OpState::Confirmed, Some(&line), now)?;
                    tx.set_meta(&settled_key(project), &now.to_string())?;
                    if let (Some(attempt), Some(task)) = (op.attempt, op.task.as_deref()) {
                        tx.set_attempt_time(project, task, attempt, AttemptStep::Merged, now)?;
                    }
                    Ok(())
                })?;
                // The branches the merge consumed are closed now: the pull
                // request is merged (design §11.2 step 5).
                close_pr_branches(store, herdr, cfg, project, &config, &op, &repo, now)?;
                // The wait is over and the failure (if any) is settled.
                resolve_wait(store, project, &task, now)?;
                resolve_failed(store, project, &task, now)?;
                merged.push(Merged {
                    task,
                    branch: op.payload["branch"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            OpState::Failed => {
                // §11.3, row 2: the forge refused the merge. The captain hears
                // about it, with the plan line that retries it.
                let reason = land
                    .result
                    .as_deref()
                    .unwrap_or("the pull request was not merged")
                    .to_string();
                fail_merge(store, project, &op, &reason, now)?;
            }
            _ => {}
        }
    }
    Ok(merged)
}

/// The pull request a merge op waits for: the number in its result
/// (`{"mode":"pr","pr":<n>,"head":"<sha>"}`, design §11.2 step 3).
fn pr_number_of(result: Option<&str>) -> Option<u64> {
    let result = result?;
    let value = serde_json::from_str::<serde_json::Value>(result).ok()?;
    if value.get("mode").and_then(|mode| mode.as_str()) != Some("pr") {
        return None;
    }
    value.get("pr").and_then(|number| number.as_u64())
}

/// The land op of a task (design §11.2 step 3): the forge op that merges its
/// pull request, the newest one.
fn land_op_of(store: &mut Store, project: &str, task: &str) -> GearResult<Option<OpRow>> {
    let seat = format!("{project}:forge");
    let ops = store.read(|tx| tx.ops_of_seat_kind(&seat, OpKind::Forge))?;
    Ok(ops
        .into_iter()
        .filter(|op| {
            op.payload.get("forge").and_then(|forge| forge.as_str()) == Some("land")
                && op.task.as_deref() == Some(task)
        })
        .next_back())
}

/// The commit a land op names: the merge commit the pull request carries.
fn merge_commit_of(result: Option<&str>) -> Option<String> {
    let result = result?;
    let value = serde_json::from_str::<serde_json::Value>(result).ok()?;
    value
        .get("merge_commit")
        .and_then(|commit| commit.as_str())
        .map(|commit| commit.to_string())
}

/// The branches a pull-request merge consumed (design §11.2 step 5): the
/// task's own branch (the pull request's head, merged already) and the
/// review's branch.
fn close_pr_branches(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    project: &str,
    config: &ProjectConfig,
    op: &OpRow,
    repo: &Repo,
    now: Ts,
) -> GearResult<()> {
    let worktree = match integration_worktree(store, herdr, repo, project, config, &cfg.merges, now)
    {
        Ok(worktree) => worktree,
        // Without the worktree the branches stay: they hold no work that is
        // lost, and the captain sees them on the next landing.
        Err(_) => return Ok(()),
    };
    let dir = Path::new(&worktree.path);
    // The project's own remote (T-262).
    let remote = Target::of(config, cfg.remote.as_ref());
    let task = op.task.clone().unwrap_or_default();
    let task_branch = op.payload["branch"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let review = op
        .payload
        .get("review")
        .and_then(|review| review.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let lane = lane_of(store, project, op.task.as_deref()).unwrap_or_default();
            review_branch(&config.conventions.review_branch, project, &lane, &task)
        });
    // T-582: a feature's own branch stays when its integration task lands.
    let keep_branch = op
        .payload
        .get("keep_branch")
        .and_then(|keep| keep.as_bool())
        .unwrap_or(false);
    if !task_branch.is_empty() && !keep_branch {
        delete_remote(dir, &task_branch, remote);
    }
    if !review.is_empty() {
        delete_remote(dir, &review, remote);
    }
    Ok(())
}

/// The branch a lane cut for a task (T-253): the project's
/// `[conventions].task_branch` template, `<lane>/t<n>` by default (17 §3).
fn branch_of(template: &str, project: &str, lane: &str, task: &str) -> String {
    gear_model::branch::expand(template, project, lane, task)
}

/// The reviewers' branch of a task (T-253): the project's
/// `[conventions].review_branch` template, `review/t<n>` by default.
fn review_branch(template: &str, project: &str, lane: &str, task: &str) -> String {
    gear_model::branch::expand(template, project, lane, task)
}

/// The lane an op's task belongs to: the seat that did the work. An op that
/// names no task (or a task with no lane) has none.
fn lane_of(store: &mut Store, project: &str, task: Option<&str>) -> GearResult<String> {
    let Some(task) = task else {
        return Ok(String::new());
    };
    Ok(store
        .read(|tx| tx.get_task(project, task))?
        .and_then(|row| row.lane)
        .unwrap_or_default())
}

/// The pull request a task lands through (design §11.2).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PrTarget {
    /// The pull request's number: the forge names it.
    number: u64,
    /// The branch the pull request's head is: the task's own branch. The
    /// engine pushes the gated head there (a fast-forward of it).
    branch: String,
}

/// What a merge in a pull-request mode finds (design §11.2, §11.3).
#[derive(Debug, Clone, PartialEq, Eq)]
enum PrClaim {
    /// Not a pull-request merge: the engine merges locally, as today.
    Local,
    /// The claim opened a pull request: this is it.
    Pr(PrTarget),
    /// The claim pull request was never opened (design §11.3, row 1): the
    /// merge fails and says so.
    NoPullRequest,
    /// The claim is there but not confirmed yet: the pull request may still
    /// come, so the merge waits for it.
    Pending,
}

/// The claim pull request of a task (design §11.2 step 3): the forge op that
/// opened it, and what it says.
fn claim_pr(store: &mut Store, project: &str, task: &str) -> GearResult<PrClaim> {
    let seat = format!("{project}:forge");
    let key = format!("{task}:claim");
    let ops = store.read(|tx| tx.ops_of_seat_kind(&seat, OpKind::Forge))?;
    let claims: Vec<&OpRow> = ops
        .iter()
        .filter(|op| op.payload.get("key").and_then(|key| key.as_str()) == Some(key.as_str()))
        .collect();
    // The newest confirmed claim names the pull request.
    if let Some(op) = claims
        .iter()
        .rev()
        .find(|op| op.state == OpState::Confirmed)
    {
        let number = op
            .result
            .as_deref()
            .and_then(|result| serde_json::from_str::<serde_json::Value>(result).ok())
            .and_then(|value| value.get("number").and_then(|number| number.as_u64()));
        if let Some(number) = number {
            return Ok(PrClaim::Pr(PrTarget {
                number,
                branch: op
                    .payload
                    .get("branch")
                    .and_then(|branch| branch.as_str())
                    .unwrap_or_default()
                    .to_string(),
            }));
        }
    }
    // A claim that is there but not confirmed: the pull request may still
    // come. A claim that failed will not: the pull request was never opened.
    if claims.iter().any(|op| {
        matches!(
            op.state,
            OpState::Intended | OpState::Executing | OpState::Submitted | OpState::Unknown
        )
    }) {
        return Ok(PrClaim::Pending);
    }
    Ok(PrClaim::NoPullRequest)
}

/// The pull request a merge in a pull-request mode lands through, if the repo
/// is in that mode (design §11.2).
fn pr_claim(store: &mut Store, project: &str, op: &OpRow, repo: &Repo) -> GearResult<PrClaim> {
    let Some(github) = repo.github.as_ref() else {
        return Ok(PrClaim::Local);
    };
    if github.merge != gear_app::config::MergeMode::Pr {
        return Ok(PrClaim::Local);
    }
    let task = op.task.clone().unwrap_or_default();
    let mut claim = claim_pr(store, project, &task)?;
    if let PrClaim::Pr(mut target) = claim {
        // The pull request's head is the task's own branch (the claim opened
        // it): the engine pushes the gated head there.
        if target.branch.is_empty() {
            target.branch = op.payload["branch"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        claim = PrClaim::Pr(target);
    }
    Ok(claim)
}

/// The merge op of a task, if one is queued, running or done (a Failed or
/// Void one does not block a retry).
/// The (task, attempt) of every merge op of `project`, in any state (see
/// [`merge_op_of`]): Failed and Void count too.
/// How often the lost-merge scan (step 1b) runs when no merge op settles: it
/// reads every done task of the project, so not on every tick (T-516).
const LOST_SCAN_EVERY_S: i64 = 300;

/// Accepted work lands within this time, carried or not, or it is lost
/// (user, 2026-11-04).
const LAND_WITHIN_S: i64 = 2 * 3600;

/// When the lost-merge scan last ran for a project.
fn lost_scan_key(project: &str) -> String {
    format!("engine.lost_scan.{project}")
}

/// When a merge op of the project last reached a final state (`Confirmed` or
/// `Failed`), written where it settles: the lost-merge scan runs at once in the
/// tick after one does (T-516, F2 of the review of attempt 1 — the step used to
/// re-read every settled op instead).
fn settled_key(project: &str) -> String {
    format!("engine.last_settled.{project}")
}

fn merge_keys(
    store: &mut Store,
    project: &str,
) -> GearResult<std::collections::HashSet<(String, i64)>> {
    // T-523: the keys of the project's merge ops, read in SQL and without the
    // payload — every state counts, and this used to parse every op of the
    // store in seven states to keep the merge ones.
    let keys = store.read(|tx| tx.op_keys_of_kind(project, OpKind::Merge))?;
    Ok(keys.into_iter().collect())
}

#[cfg_attr(not(test), allow(dead_code))]
fn merge_op_of(
    store: &mut Store,
    project: &str,
    task: &str,
    attempt: i64,
) -> GearResult<Option<OpRow>> {
    // Every state counts, Failed and Void too: a merge that failed waits for
    // the captain (one item) and is never re-queued tick after tick — a new
    // passing attempt gets its own merge. T-523: the merge ops of the project
    // are read by kind, not out of every op of the store.
    store.read(|tx| {
        Ok(tx
            .ops_of_kind_in_states(project, OpKind::Merge, &[])?
            .into_iter()
            .find(|op| op.task.as_deref() == Some(task) && op.attempt == Some(attempt)))
    })
}

/// The merge ops that may run now: Intended, or Unknown after a restart.
///
/// T-395: both states run in one queue, oldest first. Every Intended op before
/// any Unknown one starved a merge that lost its push race (it stayed Unknown)
/// while newer merges kept arriving.
fn ready_merges(store: &mut Store, project: &str) -> GearResult<Vec<OpRow>> {
    store.read(|tx| {
        // T-523: the merge ops in those two states, read by kind.
        let mut ready = tx.ops_of_kind_in_states(
            project,
            OpKind::Merge,
            &[OpState::Intended, OpState::Unknown],
        )?;
        // The order of ops: the time they were created, then their number —
        // not their id as a string, where O-999 sorts after O-1000 (T-268).
        let op_number = |id: &str| -> i64 {
            id.split_once('-')
                .and_then(|(_, number)| number.parse().ok())
                .unwrap_or(0)
        };
        ready.sort_by_key(|op| (op.created, op_number(&op.id)));
        Ok(ready)
    })
}

/// The engine's integration worktree of one repo: `<project>/merge`, created once
/// from that repo's integration branch and reopened after that (17 §4:
/// worktrees are Herdr's).
///
/// One worktree per repo (T-53): a project of several repos merges each of
/// them in its own worktree, on its own integration branch, with its own gate.
fn integration_worktree(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    repo: &Repo,
    project: &str,
    config: &ProjectConfig,
    merges: &MergeWorkers,
    now: Ts,
) -> Result<gear_herdr::Worktree, String> {
    stage_worktree(store, herdr, repo, project, config, merges, now, 0)
}

/// The repo's integration worktree, opened the way every merge opens it (seat
/// `merge`, `<project>/merge/home`), for a step that is not a merge op (T-585:
/// a landing merges the captain's records there). It is the same worktree the
/// merge queue uses, so the caller takes [`merge_lock`] before it touches it.
pub(crate) fn open_integration_worktree(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    repo: &Repo,
    project: &str,
    config: &ProjectConfig,
    now: Ts,
) -> Result<gear_herdr::Worktree, String> {
    let merges = MergeWorkers::default();
    stage_worktree(store, herdr, repo, project, config, &merges, now, 0)
}

/// The worktree of one stage of a merge train: stage 0 is the integration
/// worktree, stage `n` its sibling `<label>-<n+1>` on seat `merge-<n+1>`.
#[allow(clippy::too_many_arguments)]
fn stage_worktree(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    repo: &Repo,
    project: &str,
    config: &ProjectConfig,
    merges: &MergeWorkers,
    now: Ts,
    stage: usize,
) -> Result<gear_herdr::Worktree, String> {
    let (label, seat) = if stage == 0 {
        (merge_label(project, repo), "merge".to_string())
    } else {
        (
            format!("{}-{}", merge_label(project, repo), stage + 1),
            format!("merge-{}", stage + 1),
        )
    };
    // The merge worktree is seat `merge` (T-253): its branch is the project's
    // `[conventions].seat_branch` template with the project's voyage,
    // `<project>/merge/<voyage>` by default (T-584: project / seat / voyage,
    // like project / lane / task).
    let voyage = store
        .read(|tx| gear_app::plan::current_voyage(tx, project))
        .map_err(|e| e.detail)?;
    let branch =
        gear_model::branch::expand_seat(&config.conventions.seat_branch, project, &seat, &voyage);
    // T-565: the merge seat has no seat row, so no apply and no lane pass moves
    // its branch — it is settled here, before the worktree is opened or
    // created. A ref of the previous default (`p/merge/between-voyages`, T-584) is
    // renamed, whether it stands in the way or merely beside the new name, and
    // a ref of another name in the way fails the merge naming it, never both.
    let old = gear_app::seat_branch::previous_default(project, &seat);
    if let Err(reason) =
        gear_app::seat_branch::settle(&repo.path, &old, &branch, &config.conventions.remote)
    {
        // Named as the merge worktree's, so the wait files under its worktree
        // cause (T-566).
        return Err(format!("the merge worktree's branch {branch}: {reason}"));
    }
    let worktree = match herdr.worktree_open(&repo.path, &branch, &label) {
        Outcome::Ok(worktree) => worktree,
        Outcome::Refused { code, .. } if code == "worktree_not_found" => {
            match herdr.worktree_create(&repo.path, &branch, &repo.integration, &label) {
                Outcome::Ok(worktree) => worktree,
                other => return Err(step_failure("worktree_create", &other)),
            }
        }
        other => return Err(step_failure("worktree_open", &other)),
    };
    // T-253: the worktree must belong to this repo. Herdr places worktrees by
    // the repo folder's name and the branch, so two repos with the same folder
    // name came back with one directory. Another repository's checkout is not
    // used; the captain hears about it on thread `worktree:merge`.
    crate::onboard::check_worktree(store, project, "merge", &repo.path, &worktree.path, now)?;
    // T-107: an older engine's pre-push hook is unset here too, so the merge
    // worktree carries no allow-list either. Once per worktree: the clean is
    // idempotent and costs a git process on every attempt otherwise (T-127).
    if !merges.hook_cleaned(&worktree.path) {
        crate::onboard::forget_gear_hooks(std::path::Path::new(&worktree.path))?;
        merges.hook_cleaned_now(&worktree.path);
    }
    Ok(worktree)
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
///
/// T-582: a task carries the branch it is merged into and the gate its merge
/// runs. A member of a feature (`base` = the feature's branch) merges there, with
/// the feature's own check instead of the project gate; the integration task
/// merges the feature's branch into the integration branch with the full gate.
fn repo_of_op(
    store: &mut Store,
    project: &str,
    op: &OpRow,
    config: &ProjectConfig,
) -> GearResult<Repo> {
    let task = match op.task.as_deref() {
        Some(task) => store.read(|tx| tx.get_task(project, task))?,
        None => None,
    };
    let mut repo = task
        .as_ref()
        .map(|row| config.repo_of_task(row))
        .unwrap_or_else(|| config.root_repo());
    if let Some(row) = &task {
        if let Some(base) = row
            .base
            .as_deref()
            .map(str::trim)
            .filter(|base| !base.is_empty())
        {
            repo.integration = base.to_string();
        }
        if let Some(gate) = row
            .gate
            .as_deref()
            .map(str::trim)
            .filter(|gate| !gate.is_empty())
        {
            repo.gate = gate.to_string();
        }
    }
    Ok(repo)
}

/// The key of a merge worker: one worker per repo (T-53), so a merge in one
/// repo does not wait for the merge of another.
/// The key of a repo's merge worker: the repo's real path and its
/// integration branch. Another project with a checkout of the same repo
/// that lands on the same branch shares the key.
fn worker_key(repo: &Repo) -> String {
    let path = std::fs::canonicalize(&repo.path)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| repo.path.clone());
    format!("{path}\u{1f}{}", repo.integration)
}

/// Merge one op's task branches and run the gate, once. `Ok(lines)` = merged
/// and pushed; `Err(failed)` = nothing was pushed and the worktree is clean
/// again.
#[cfg(test)]
fn merge_one(worktree: &gear_herdr::Worktree, repo: &Repo, op: &OpRow) -> Result<String, Failed> {
    merge_one_with_db(worktree, repo, op, "")
}

/// [`merge_one`] on an engine store whose path the probe can keep the step
/// boundaries in (T-407).
#[cfg(test)]
fn merge_one_with_db(
    worktree: &gear_herdr::Worktree,
    repo: &Repo,
    op: &OpRow,
    db_path: &str,
) -> Result<String, Failed> {
    // T-107: the tests' runner moves the refs from the remote's own side, so
    // no engine test runs `git push`.
    let review = {
        let task = op.task.clone().unwrap_or_default();
        gear_model::branch::expand(gear_model::branch::REVIEW_BRANCH, "", "", &task)
    };
    // The tests' default message: the project's default, with `{task}` filled
    // once as the engine fills it (T-296).
    let commit = gear_model::commit::fill(
        gear_model::commit::MERGE_MESSAGE,
        &[("task", op.task.as_deref().unwrap_or_default())],
    );
    // The tests' default review message, as the engine fills it (T-564).
    let prefix = test_review_prefix(op);
    let mut probe = Probe::new(
        op.task.as_deref().unwrap_or("-"),
        op.created,
        db_path,
        &op.id,
    );
    let step = merge_one_locked(
        worktree,
        repo,
        op,
        &lock_of(worktree)?,
        &[],
        // The fixtures name their remote `origin` (T-262).
        Target {
            name: "origin",
            runner: &FetchRemote,
        },
        None,
        &review,
        &commit,
        &prefix,
        &mut probe,
    );
    // T-407: the operation's own completion line, as the retry loop writes it.
    if !matches!(step, Step::Waiting(_)) {
        probe.done();
    }
    step_result(step)
}

/// A review commit's subject prefix for the fixtures (T-564): the default
/// `review(<task>): ` message, cut at `{what}`.
#[cfg(test)]
fn test_review_prefix(op: &OpRow) -> String {
    let task = op.task.as_deref().unwrap_or_default();
    gear_model::commit::fill(gear_model::commit::REVIEW_MESSAGE, &[("task", task)])
        .split("{what}")
        .next()
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
fn lock_of(worktree: &gear_herdr::Worktree) -> Result<File, Failed> {
    merge_lock(Path::new(&worktree.path))
        .map_err(|reason| Failed::stuck(reason))?
        .ok_or_else(|| Failed::stuck("merge worktree is busy"))
}

/// The step of a merge, as the tests read it: the line, or the failure.
#[cfg(test)]
fn step_result(step: Step) -> Result<String, Failed> {
    match step {
        Step::Merged(line) => Ok(line),
        Step::Pr { pr, head } => Ok(format!("pr {pr} head {head}")),
        Step::Waiting(tasks) => Err(Failed::stuck(format!(
            "waits for {}",
            tasks
                .iter()
                .map(|(task, _)| task.clone())
                .collect::<Vec<String>>()
                .join(", ")
        ))),
        Step::Raced(reason) => Err(Failed::clear(reason)),
        Step::Failed(failed) => Err(failed),
    }
}

/// Whether a push failure lost the race because the remote moved (T-280):
/// another merge landed on the integration branch between the fetch and the
/// push. A "cannot lock ref" failure is not that: it can be a stale local
/// lock, so it keeps the failed end.
/// Whether a merge failed only because its fetch could not read the remote.
fn unreachable(reason: &str) -> bool {
    reason.starts_with(FETCH)
}

fn lost_the_race(reason: &str) -> bool {
    reason.starts_with(RACE)
        && (reason.contains("non-fast-forward") || reason.contains("fetch first"))
}

fn merge_one_locked(
    worktree: &gear_herdr::Worktree,
    repo: &Repo,
    op: &OpRow,
    lock: &File,
    others: &[(String, String, bool)],
    remote: Target<'_>,
    pr: Option<&PrTarget>,
    review_fallback: &str,
    commit: &str,
    review_prefix: &str,
    probe: &mut Probe,
) -> Step {
    let prepared = match prepare_merge(
        worktree,
        repo,
        op,
        lock,
        others,
        remote,
        review_fallback,
        commit,
        review_prefix,
        probe,
        None,
    ) {
        Ok(prepared) => prepared,
        Err(step) => return step,
    };
    let tasks = vec![op.task.clone().unwrap_or_default()];
    let round = gate_round(op);
    // T-573: a merge that carries only reports skips the gate (the user,
    // 2026-11-05: a merge holding one review record still paid a four-to-five
    // minute CI round). The conflict check ran above and the push below is the
    // same; only the gate step is skipped, and the merge result says so.
    let dir = Path::new(&worktree.path);
    let git = |dir: &Path, args: &[&str]| git_with_lock(dir, args, Some(lock));
    let mut prepared = prepared;
    prepared.no_gate = reports_only(
        &changed_paths(&git, dir, &prepared.base, "HEAD"),
        &repo.voyage_dir,
        &op.project,
    );
    if !prepared.no_gate {
        if let Err(step) = run_gate(
            worktree,
            repo,
            lock,
            remote,
            &op.project,
            &round,
            &tasks,
            probe,
        ) {
            return step;
        }
    }
    land_merge(worktree, repo, lock, remote, pr, commit, probe, &prepared)
}

/// A merge built and checked for conflicts, not yet gated or landed: what
/// the land step needs.
#[derive(Debug)]
struct Prepared {
    task: String,
    sha: String,
    branch: String,
    review: String,
    review_ref: String,
    has_review: bool,
    pushed: Pushed,
    /// What the merge was built on: the remote integration branch, or the
    /// commit of the train's stage ahead (T-573 reads the changed files from
    /// it).
    base: String,
    /// T-573: the merge carries only reports, so no gate ran for it.
    no_gate: bool,
    /// T-582: the branch is the feature's own, so the close keeps it.
    keep_branch: bool,
}

/// The attempt the merge op names, as the review message's `{attempt}`.
fn op_attempt(op: &OpRow) -> String {
    op.attempt
        .map(|attempt| attempt.to_string())
        .unwrap_or_default()
}

/// The prefix a review commit's subject starts with (T-564): the project's
/// `[conventions].review_message` filled with the task and the attempt, cut at
/// `{what}` — what the commit carries is the reviewer's to write. The default
/// gives `review(T-7): `.
fn review_prefix(config: &ProjectConfig, project: &str, task: &str, attempt: &str) -> String {
    let filled = gear_model::commit::fill(
        &config.conventions.review_message,
        &[("project", project), ("task", task), ("attempt", attempt)],
    );
    match filled.find("{what}") {
        Some(at) => filled[..at].to_string(),
        None => filled,
    }
}

/// Steps 1 to 7 of a merge: a clean base (the remote integration branch, or
/// `base`: the commit of the train's stage ahead), the approved candidate and
/// its review branch merged onto it, no conflict. `Err` is the merge's end.
#[allow(clippy::too_many_arguments)]
fn prepare_merge(
    worktree: &gear_herdr::Worktree,
    repo: &Repo,
    op: &OpRow,
    lock: &File,
    others: &[(String, String, bool)],
    remote: Target<'_>,
    review_fallback: &str,
    commit: &str,
    review_prefix: &str,
    probe: &mut Probe,
    base: Option<&str>,
) -> Result<Prepared, Step> {
    let git = |dir: &Path, args: &[&str]| git_with_lock(dir, args, Some(lock));
    let dir = Path::new(&worktree.path);
    let integration = repo.integration.clone();
    // The project's own remote (T-262): every fetch, ref and message of this
    // merge names it.
    let remote_name = remote.name;
    // What the candidate is merged onto: the remote integration branch, or
    // the commit of the train's stage ahead.
    let base_ref = match base {
        Some(base) => base.to_string(),
        None => format!("{remote_name}/{integration}"),
    };
    let branch = op.payload["branch"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // The op may come from `tasks::settle_review`, which names the branch and
    // the candidate but not the review branch: derive it, and merge it only
    // when it exists (F5 of the T-12 review).
    let task = op.task.clone().unwrap_or_default();
    let review = op
        .payload
        .get("review")
        .and_then(|review| review.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| review_fallback.to_string());

    // 1. a clean base: the remote integration branch, whatever an interrupted
    //    earlier run left behind.
    //    The fetch names every branch of the remote: the approved candidate
    //    may sit on any of them (T-61), and a clone that restricts its refspec
    //    would not bring it in. A fetch that fails may clear (the network,
    //    the remote): it is retried.
    probe.begin("fetch");
    match base {
        None => {
            if let Err(reason) = git(
                dir,
                &[
                    "fetch",
                    "--prune",
                    remote_name,
                    &format!("+refs/heads/*:refs/remotes/{remote_name}/*"),
                ],
            ) {
                probe.step("fetch");
                return Err(Step::Failed(Failed::clear(format!(
                    "{FETCH}{remote_name}: {reason}"
                ))));
            }
            if let Err(reason) = git(
                dir,
                &["reset", "--hard", &format!("{remote_name}/{integration}")],
            ) {
                probe.step("fetch");
                return Err(Step::Failed(Failed::stuck(format!(
                    "reset to {remote_name}/{integration}: {reason}"
                ))));
            }
        }
        // A stage of a merge train: the base is the commit the stage ahead of
        // it lands; the refs were fetched once for the whole train.
        Some(base) => {
            if let Err(reason) = git(dir, &["reset", "--hard", base]) {
                probe.step("fetch");
                return Err(Step::Failed(Failed::stuck(format!(
                    "reset to the train's base {base}: {reason}"
                ))));
            }
        }
    }
    probe.step("fetch");

    probe.begin("merge");
    // 2. the approved candidate, by commit. P0: what merges is the commit the
    //    review approved, never what a branch points at now. The branch the
    //    lane named may hold it, hold more than it, or be gone (T-61).
    let candidate = op.payload["candidate"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    if candidate.is_empty() {
        return Err(Step::Failed(Failed::stuck(format!(
            "{task}: the merge op names no candidate"
        ))));
    }
    // 2. the commit must be known here. An op names its candidate by full sha
    //    (it is pinned to it), so this costs no git process (T-127); a payload
    //    that names something else is resolved once, and pinned by sha too.
    let sha = match git_sha(&candidate) {
        Some(sha) => sha,
        None => match git(dir, &["rev-parse", "--verify", &format!("{candidate}^{{commit}}")]) {
            Ok(out) => out.trim().to_string(),
            Err(reason) => {
                return Err(Step::Failed(Failed::clear(format!(
                    "{task}: the approved candidate {candidate} is not fetchable from {remote_name} yet\n{reason}\n\
                     it may not be pushed yet: the merge retries and asks the captain if it stays"
                ))))
            }
        },
    };

    // 3. the candidate must be here and reachable from a pushed ref: work that
    //    was never pushed (or whose branch is gone) is not merged, and the
    //    captain is told why. The branch may have any name: the task's, a
    //    rework's (`engine/t53-v2`), or another one. One process answers both
    //    (T-127): `for-each-ref --contains` fails when the commit is not here
    //    at all — the "not fetchable yet" of step 2, for a sha.
    //    The listing names each ref and its tip, so step 9 closes the branches
    //    it needs from the same answer instead of asking again (T-127).
    let pushed = match git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname:short) %(objectname)",
            &format!("--contains={sha}"),
            &format!("refs/remotes/{remote_name}/"),
        ],
    ) {
        Ok(pushed) => pushed,
        Err(reason) => {
            return Err(Step::Failed(Failed::clear(format!(
            "{task}: the approved candidate {candidate} is not fetchable from {remote_name} yet\n{reason}\n\
                 it may not be pushed yet: the merge retries and asks the captain if it stays"
            ))))
        }
    };
    let pushed = Pushed::read(&pushed, &sha);
    if pushed.refs.is_empty() {
        // The candidate may not be pushed yet: the lane pushes it and the
        // merge retries. Only the bounded retry decides whether it was never
        // pushed (F2 of the T-61 review).
        return Err(Step::Failed(Failed::clear(format!(
            "{task}: the approved candidate {sha} is on no pushed ref of {remote_name}\n\
             it was never pushed, or the branch that held it is gone"
        ))));
    }

    // 4. the work the candidate carries (T-62): the commits of other tasks
    //    that are in this candidate and are not on the integration branch yet.
    //    They must land first: a task that waits for another one must not
    //    carry its unreviewed commits into the integration branch.
    //
    //    One `rev-list` names them all (T-127): the merge used to pay up to
    //    three git processes per other task per attempt, and this attempt runs
    //    on every tick while it waits. The range's members are exactly the
    //    commits the candidate adds to the integration branch, which is
    //    "carried and not landed" — and an empty list is step 5's answer too.
    let ahead = git(dir, &["rev-list", &format!("{base_ref}..{sha}")]).ok();
    let carried = match &ahead {
        Some(ahead) => carried_tasks(dir, &git, ahead, &sha, others),
        None => Vec::new(),
    };
    if !carried.is_empty() {
        return Err(Step::Waiting(carried));
    }

    // 5. the candidate may already be on the integration branch: a restart
    //    after the push, or another task that carried it in.
    let already = ahead.is_some_and(|ahead| ahead.trim().is_empty());

    // 6. the merges, in order (the approved candidate, then the review
    //    branch); the candidate is pinned by sha, so a branch that moves later
    //    cannot slip an unreviewed commit in.
    //    The review branch is *required* for a reviewed merge: a merge that
    //    publishes the task without its review is a partial merge (F6 of the
    //    T-12 review). A mechanically settled attempt has no reviewer, so no
    //    review branch (T-405): its op carries `"mechanical": true`, and the
    //    pinned candidate merges alone. The integration branch is not touched:
    //    the merge fails before it, so dev stays where it was. A candidate
    //    that is already merged needs no merge commit; its review branch still
    //    merges with it.
    let mechanical = op
        .payload
        .get("mechanical")
        .and_then(|mechanical| mechanical.as_bool())
        .unwrap_or(false);
    // T-582: the branch is a feature's own (`p/feature/f-1`): the close at the
    // end of the merge keeps it, since the captain deletes it at landing.
    let keep_branch = op
        .payload
        .get("keep_branch")
        .and_then(|keep| keep.as_bool())
        .unwrap_or(false);
    // T-532: a remerge carries the captain's conflict fix on the review
    // branch (decision 03:55): the review branch merged the integration branch
    // and the conflict was resolved there.
    let remerge = op
        .payload
        .get("remerge")
        .and_then(|remerge| remerge.as_bool())
        .unwrap_or(false);
    let review_ref = format!("{remote_name}/{review}");
    let review_found = git(dir, &["rev-parse", "--verify", &review_ref]);
    let has_review = review_found.is_ok() && !mechanical;
    // T-558: a review leaves no branch of its own on the remote — the reviewer
    // pushes its review commit onto the TASK branch, and that branch is what
    // merges. A task already in review before this change still has the old
    // review branch on the remote: that one merges exactly as before
    // (`has_review`), and the sweep and `close_branches` still delete it.
    let task_ref = format!("{remote_name}/{branch}");
    let has_task_branch =
        !branch.is_empty() && git(dir, &["rev-parse", "--verify", &task_ref]).is_ok();
    // A restart after the push finds none of them and the candidate on the
    // integration branch: that merge already happened (`already`).
    if !has_review && !mechanical && !has_task_branch && !already {
        let reason = if branch.is_empty() {
            "the merge op names no task branch".to_string()
        } else {
            format!(
                "{}: the task branch {task_ref} is missing on {remote_name}",
                git(dir, &["rev-parse", "--verify", &task_ref])
                    .err()
                    .unwrap_or_default()
            )
        };
        return Err(Step::Failed(Failed::stuck(format!(
            "{task}: no branch to merge from on {remote_name}\n{reason}\n\
             a merge publishes the approved candidate and the review commit pushed on its task branch"
        ))));
    }
    // Where the task branch is, when it is there: a restart after the push may
    // still see the last one locally, at the candidate itself. T-564: the ref
    // is resolved to one sha here and that sha is what merges below, so a push
    // that lands after this read cannot slip in unreviewed.
    let task_tip = if has_task_branch {
        git(dir, &["rev-parse", &task_ref])
            .ok()
            .map(|tip| tip.trim().to_string())
    } else {
        None
    };
    // The task branch must hold the approved candidate: the review commit is
    // pushed on top of it, never instead of it.
    if !has_review
        && task_tip
            .as_deref()
            .is_some_and(|tip| git(dir, &["merge-base", "--is-ancestor", &sha, tip]).is_err())
    {
        return Err(Step::Failed(Failed::stuck(format!(
            "{task}: the task branch {task_ref} does not hold the approved candidate {sha}\n\
             the review commit is pushed on top of the candidate, so the merge would publish work nobody reviewed"
        ))));
    }
    // T-564: the task branch holds the approved candidate, the review commit
    // and nothing else. A lane commit pushed after the hand-in (prometheus
    // 2026-11-04: hull's fix on T-562) would land unreviewed: each non-merge
    // commit after the candidate must be the review, whose subject starts with
    // the project's review message up to `{what}`. A merge commit on top of it
    // is no way out either — its parents are in the range as well.
    //
    // Only the commits the merge newly brings in are judged (T-564 review F1):
    // a task branch that merged the integration branch into it — the
    // documented conflict fix (course.md: merge dev into the task branch,
    // resolve, push, remerge) — carries dev's own commits, and those are
    // reviewed work already on the branch. `--not <base>` leaves them out.
    if !has_review && !mechanical && task_tip.is_some() {
        let tip = task_tip.as_deref().unwrap_or_default();
        let log = git(
            dir,
            &[
                "log",
                "--no-merges",
                "--format=%H%x09%s",
                &format!("{sha}..{tip}"),
                "--not",
                &base_ref,
            ],
        )
        .map_err(|reason| {
            Step::Failed(Failed::clear(format!(
                "{task}: the commits of the task branch {task_ref} could not be read: {reason}"
            )))
        })?;
        let foreign: Vec<String> = log
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| {
                let (commit, subject) = line.split_once('\t')?;
                (!subject.starts_with(review_prefix)).then(|| format!("{commit} {subject}"))
            })
            .collect();
        if !foreign.is_empty() {
            return Err(Step::Failed(Failed::stuck(format!(
                "{task}: the task branch holds commits after the approved candidate that are not the review: {}; a change after hand-in needs a new attempt",
                foreign.join(", ")
            ))));
        }
    }
    // What merges, and the source the merge commit names for it: as today, the
    // candidate by sha and then the review branch by its name; without a review
    // branch, the task branch alone, which holds both.
    let mut merges: Vec<(String, String)> = Vec::new();
    if has_review {
        if !already {
            merges.push((sha.clone(), sha.clone()));
        }
        merges.push((review_ref.clone(), review_ref.clone()));
    } else if !mechanical {
        // Nothing to merge when the candidate is already on the integration
        // branch and the task branch is exactly it, or is gone with it.
        let only_the_candidate =
            already && task_tip.as_deref().map_or(true, |tip| tip == sha.as_str());
        if !only_the_candidate {
            // The pinned sha merges; the merge commit still names the branch.
            merges.push((
                task_tip.clone().unwrap_or_else(|| task_ref.clone()),
                task_ref.clone(),
            ));
        }
    } else if !already {
        merges.push((sha.clone(), sha.clone()));
    }
    // T-532: a remerge whose review branch contains the candidate — the captain
    // merged the integration branch into it and resolved the conflict there —
    // merges only that commit: the candidate is inside it, and merging the
    // candidate alone would conflict again. The review ref is resolved once,
    // so a branch that moves later cannot slip an unreviewed commit in, and the
    // merge commit still names the review ref as its source.
    if remerge
        && has_review
        && git(dir, &["merge-base", "--is-ancestor", &sha, &review_ref]).is_ok()
    {
        if let Ok(out) = git(
            dir,
            &["rev-parse", "--verify", &format!("{review_ref}^{{commit}}")],
        ) {
            let review_sha = out.trim().to_string();
            if !review_sha.is_empty() {
                merges = vec![(review_sha, review_ref.clone())];
            }
        }
    }
    if merges.is_empty() {
        // The candidate is on the integration branch and its review commit is
        // gone with the branch: a restart after the push. Nothing to merge,
        // nothing to push.
        return Err(Step::Merged(format!(
            "{task} was already merged: {sha} is on {remote_name}/{integration}"
        )));
    }
    let head_before = git(dir, &["rev-parse", "HEAD"]).ok();
    for (target, source) in &merges {
        if let Err(reason) = git(
            dir,
            &[
                "merge",
                "--no-ff",
                target,
                "-m",
                &gear_model::commit::fill(commit, &[("source", source)]),
            ],
        ) {
            // Abort and leave the worktree exactly at the integration branch.
            let _ = git(dir, &["merge", "--abort"]);
            let _ = git(
                dir,
                &["reset", "--hard", &format!("{remote_name}/{integration}")],
            );
            probe.step("merge");
            return Err(Step::Failed(Failed::stuck(format!(
                "conflict merging {target}: {reason}"
            ))));
        }
    }
    // Every target was already on the branch ("Already up to date"): the work
    // landed with another merge, and a gate would refuse a candidate that
    // carries nothing new (Prometheus T-745/T-746, 2026-11-04).
    if head_before.is_some() && git(dir, &["rev-parse", "HEAD"]).ok() == head_before {
        probe.step("merge");
        return Err(Step::Merged(format!(
            "{task} was already merged: its work is on {remote_name}/{integration}"
        )));
    }

    // 7. conflict markers that a merge left behind without reporting a
    //    conflict (a file committed with markers in it).
    if git(dir, &["grep", "-n", "-e", "^<<<<<<<", "-e", "^>>>>>>>"]).is_ok() {
        let _ = git(
            dir,
            &["reset", "--hard", &format!("{remote_name}/{integration}")],
        );
        probe.step("merge");
        return Err(Step::Failed(Failed::stuck(
            "conflict markers in the merged tree",
        )));
    }
    probe.step("merge");

    Ok(Prepared {
        task,
        sha,
        branch,
        review,
        review_ref,
        has_review,
        pushed,
        base: base_ref,
        no_gate: false,
        keep_branch: keep_branch.clone(),
    })
}

/// Step 8 of a merge: the gate of the repo, in the worktree, before anything
/// is pushed. `Err` is the merge's end; the worktree is clean again.
fn run_gate(
    worktree: &gear_herdr::Worktree,
    repo: &Repo,
    lock: &File,
    remote: Target<'_>,
    project: &str,
    round: &str,
    tasks: &[String],
    probe: &mut Probe,
) -> Result<(), Step> {
    let git = |dir: &Path, args: &[&str]| git_with_lock(dir, args, Some(lock));
    let dir = Path::new(&worktree.path);
    let integration = repo.integration.clone();
    let remote_name = remote.name;
    // 8. the gate, before anything is pushed: the gate of the repo the work
    //    is in (T-53).
    probe.begin("gate");
    let gate = repo.gate.clone();
    // The empty lock file is inherited as stdin (normally /dev/null for a
    // gate). Keeping the same open file description in the subprocess means
    // it still excludes recovery if the engine is killed before wait returns.
    let stdin = match lock.try_clone() {
        Ok(stdin) => stdin,
        Err(error) => return Err(Step::Failed(Failed::clear(format!("gate lock: {error}")))),
    };
    let tempfail = gate_tempfail_path(dir);
    let _ = std::fs::remove_file(&tempfail);
    // T-548: the failed-paths file belongs to this run, too: a line left by an
    // earlier gate must not name a culprit here. It is not removed after a
    // failure: the red batch reads it.
    let _ = std::fs::remove_file(gate_failed_path(dir));
    let output = match gate_command(&gate, dir, project, &integration, round, tasks)
        .stdin(stdin)
        .output()
    {
        Ok(output) => output,
        // The gate could not run at all: the environment is broken, not the
        // work. That can clear (a toolchain coming back), so it is retried.
        Err(error) => {
            return Err(Step::Failed(Failed::clear(format!(
                "gate could not run ({gate}): {error}"
            ))))
        }
    };
    if !output.status.success() {
        // Both streams: cargo reports test failures on stdout, a shell's own
        // errors on stderr.
        let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
        // T-563: a gate that pushes (or a push that ran beside it) leaves the
        // remote's chatter in the output; the captain's item and the board
        // must lead with the gate's own refusal.
        let combined = without_push_noise(&combined);
        let tail = tail(&combined, 12);
        // T-219: the tail is often slow-test timings, and the captain cannot
        // see what failed in it. The lines that name a failure come first.
        let failures = failure_lines(&combined, 20);
        let _ = git(
            dir,
            &["reset", "--hard", &format!("{remote_name}/{integration}")],
        );
        // 126 and 127 are the shell's own "cannot run that" (126: not
        // executable, 127: not found): the toolchain is missing or broken,
        // not the work. 75 (EX_TEMPFAIL) is the gate's own "I could not judge
        // this time" (a CI service refusing a request, say). All three can
        // clear, so they are retried; every other exit status is the
        // project's own gate saying no.
        let status = output.status.code().unwrap_or(0);
        let asked_retry = tempfail.exists();
        let _ = std::fs::remove_file(&tempfail);
        let mut reason = format!("gate failed ({gate}):\n");
        if !failures.is_empty() {
            reason.push_str(&failures.join("\n"));
            reason.push('\n');
        }
        reason.push_str(&tail);
        probe.step("gate");
        return Err(Step::Failed(
            if asked_retry || matches!(status, 75 | 126 | 127) {
                Failed::clear(reason)
            } else {
                Failed::stuck(reason)
            },
        ));
    }
    probe.step("gate");
    Ok(())
}

/// Steps 9 and 10 of a merge: the push of the gated commit (or the pull
/// request's head), then the close of the branches the merge consumed.
#[allow(clippy::too_many_arguments)]
fn land_merge(
    worktree: &gear_herdr::Worktree,
    repo: &Repo,
    lock: &File,
    remote: Target<'_>,
    pr: Option<&PrTarget>,
    commit: &str,
    probe: &mut Probe,
    prepared: &Prepared,
) -> Step {
    let git = |dir: &Path, args: &[&str]| git_with_lock(dir, args, Some(lock));
    let dir = Path::new(&worktree.path);
    let integration = repo.integration.clone();
    let remote_name = remote.name;
    let Prepared {
        sha,
        review_ref,
        has_review,
        ..
    } = prepared;
    let has_review = *has_review;
    // 9. PR mode (design §11.2 step 2): the integration branch is not pushed.
    //    The review's commits go onto the pull request's head, and the pull
    //    request is what lands the task.
    probe.begin("push");
    if let Some(pr) = pr {
        let step = pr_head(dir, &git, &sha, &review_ref, has_review, pr, remote, commit);
        probe.step("push");
        return step;
    }

    // 9b. push, then close the branches the merge consumed.
    // A push the remote rejects because the integration branch moved (the
    // captain or another merge pushed meanwhile) is a race, not a failure: the
    // merge starts again from the new tip.
    if let Err(reason) = remote.push(dir, &integration) {
        probe.step("push");
        if reason.contains("non-fast-forward")
            || reason.contains("fetch first")
            || reason.contains("cannot lock ref")
        {
            return Step::Failed(Failed::clear(format!("{RACE}{reason}")));
        }
        // Any other push failure can clear too: the network, the remote, a
        // hook that rejects the push once (F1 of the T-61 review).
        return Step::Failed(Failed::clear(format!("push failed: {reason}")));
    }
    probe.step("push");
    probe.begin("close");
    let step = closed_and_line(
        dir,
        repo,
        remote,
        &integration,
        remote_name,
        prepared,
        None,
        |dir, args| git(dir, args),
    );
    probe.step("close");
    step
}

/// Step 10 of a merge: close the branches the merge consumed and write the
/// `merged <task> into <integration>: ...` line (T-547: a single merge and a
/// batch, which pushes once for all of its merges, say it the same way).
fn closed_and_line(
    dir: &Path,
    repo: &Repo,
    remote: Target<'_>,
    integration: &str,
    remote_name: &str,
    prepared: &Prepared,
    commit: Option<&str>,
    git: impl Fn(&Path, &[&str]) -> Result<String, String>,
) -> Step {
    let Prepared {
        task,
        branch,
        review,
        pushed,
        keep_branch,
        ..
    } = prepared;
    let (closed, left, moved) =
        close_branches(dir, repo, branch, review, remote, pushed, *keep_branch);

    // The merge commit this line is about: a batch's op names the one it built
    // (T-547, F2 of the review), a single merge its own HEAD.
    let head = match commit {
        Some(commit) => git(dir, &["log", "-1", "--oneline", commit]).unwrap_or_default(),
        None => git(dir, &["log", "-1", "--oneline"]).unwrap_or_default(),
    };
    let mut line = format!("merged {task} into {integration}: {head}");
    // T-573: the merge says why no gate ran.
    if prepared.no_gate {
        line.push_str("\nno gate: reports only");
    }
    if !closed.is_empty() {
        line.push_str(&format!("\nclosed on {remote_name}: {}", closed.join(", ")));
    }
    if !moved.is_empty() {
        line.push_str(&format!(
            "\nclosed on {remote_name} (their commits after the approved candidate were not merged): {}",
            moved.join(", ")
        ));
    }
    if !left.is_empty() {
        line.push_str(&format!(
            "\nleft on {remote_name} (they still hold the approved commit): {}",
            left.join(", ")
        ));
    }
    Step::Merged(line)
}

/// One merge of a train, with what its tick resolved for it.
#[derive(Debug, Clone)]
struct TrainOp {
    op: OpRow,
    others: Vec<(String, String, bool)>,
    review_fallback: String,
    commit: String,
    review_prefix: String,
}

/// A merge train (`[conventions] merge_parallel` above 1): each merge is
/// built on the commit the merge ahead of it lands, so every gate checks the
/// integration branch as it will be; the gates run side by side; the merges
/// land in order, each a fast-forward of the one before. A merge whose gate
/// says no fails as it would alone; the ones behind it were gated on top of
/// it, so they go back to the queue and run again from the new tip. A reason
/// that can clear stops the train the same way, and `single` asks for the
/// next merge of this branch to run alone (with its bounded retries).
#[allow(clippy::too_many_arguments)]
fn run_train(
    worktrees: &[gear_herdr::Worktree],
    repo: &Repo,
    ops: &[TrainOp],
    remote: Target<'_>,
    db_path: &str,
    single: &std::sync::atomic::AtomicBool,
) -> Vec<(OpRow, Report)> {
    use std::sync::atomic::Ordering;
    let mut locks = Vec::new();
    for worktree in worktrees {
        match merge_lock(Path::new(&worktree.path)) {
            Ok(Some(lock)) => locks.push(lock),
            Ok(None) => return ops.iter().map(|t| (t.op.clone(), Report::Busy)).collect(),
            Err(reason) => {
                return ops
                    .iter()
                    .map(|t| {
                        (
                            t.op.clone(),
                            Report::Requeue(format!("merge lock: {reason}")),
                        )
                    })
                    .collect()
            }
        }
    }
    let mut reports: Vec<Option<Report>> = vec![None; ops.len()];
    // The stages built: (op index, worktree index, the merge, its probe).
    let mut stages: Vec<(usize, usize, Prepared, Probe)> = Vec::new();
    let mut base: Option<String> = None;
    for (index, train_op) in ops.iter().enumerate() {
        let slot = stages.len();
        if slot >= worktrees.len() {
            break;
        }
        let mut probe = Probe::new(
            train_op.op.task.as_deref().unwrap_or("-"),
            train_op.op.created,
            db_path,
            &train_op.op.id,
        );
        match prepare_merge(
            &worktrees[slot],
            repo,
            &train_op.op,
            &locks[slot],
            &train_op.others,
            remote,
            &train_op.review_fallback,
            &train_op.commit,
            &train_op.review_prefix,
            &mut probe,
            base.as_deref(),
        ) {
            Ok(prepared) => {
                let dir = Path::new(&worktrees[slot].path);
                match git_with_lock(dir, &["rev-parse", "HEAD"], Some(&locks[slot])) {
                    Ok(head) => {
                        base = Some(head.trim().to_string());
                        stages.push((index, slot, prepared, probe));
                    }
                    Err(reason) => {
                        reports[index] = Some(Report::Requeue(format!(
                            "the train could not read its stage: {reason}"
                        )));
                    }
                }
            }
            // A merge that cannot be built steps aside: the next one is built
            // on the same base, so it does not wait behind a conflict.
            Err(step) => {
                probe.done();
                reports[index] = Some(match step {
                    Step::Failed(failed) if failed.blocker == Blocker::Clear => {
                        single.store(true, Ordering::SeqCst);
                        Report::Requeue(failed.reason)
                    }
                    Step::Merged(line) if base.is_none() => Report::Merged(line),
                    Step::Merged(_) => {
                        Report::Requeue("its work is in a merge ahead of it in the train".into())
                    }
                    other => step_report(other),
                });
            }
        }
    }
    // The gates, side by side.
    let gates: Vec<Result<(), Step>> = std::thread::scope(|scope| {
        let handles: Vec<_> = stages
            .iter_mut()
            .map(|(index, slot, _, probe)| {
                let worktree = &worktrees[*slot];
                let lock = &locks[*slot];
                // This stage's own merge names the gate run (T-555).
                let train_op = &ops[*index];
                let project = train_op.op.project.clone();
                let round = gate_round(&train_op.op);
                let tasks = vec![train_op.op.task.clone().unwrap_or_default()];
                scope.spawn(move || {
                    run_gate(
                        worktree, repo, lock, remote, &project, &round, &tasks, probe,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err(Step::Failed(Failed::stuck("gate thread panicked"))))
            })
            .collect()
    });
    // The landings, in order: each one is a fast-forward of the one before.
    let mut broken = false;
    for ((index, slot, prepared, mut probe), gate) in stages.into_iter().zip(gates) {
        if broken {
            reports[index] = Some(Report::Requeue(
                "the merge ahead of it in the train did not land: it merges again from the new tip"
                    .into(),
            ));
            continue;
        }
        if let Err(step) = gate {
            broken = true;
            probe.done();
            reports[index] = Some(match step {
                Step::Failed(failed) if failed.blocker == Blocker::Clear => {
                    single.store(true, Ordering::SeqCst);
                    Report::Requeue(failed.reason)
                }
                other => step_report(other),
            });
            continue;
        }
        let step = land_merge(
            &worktrees[slot],
            repo,
            &locks[slot],
            remote,
            None,
            &ops[index].commit,
            &mut probe,
            &prepared,
        );
        probe.done();
        reports[index] = Some(match step {
            Step::Merged(line) => Report::Merged(line),
            Step::Failed(failed) => {
                broken = true;
                if !lost_the_race(&failed.reason) {
                    single.store(true, Ordering::SeqCst);
                }
                Report::Requeue(failed.reason)
            }
            other => {
                broken = true;
                step_report(other)
            }
        });
    }
    ops.iter()
        .zip(reports)
        .map(|(train_op, report)| {
            (
                train_op.op.clone(),
                report.unwrap_or_else(|| Report::Requeue("it waits for the next train".into())),
            )
        })
        .collect()
}

/// A batch merge (T-547, `[conventions] merge_batch` above 1): the passed
/// merges of one project are merged one after the other into the integration
/// worktree, each onto the commit the one ahead of it built, and the gate runs
/// once for all of them. Green: one push lands all of them, then each closes
/// its own branches. Red: when the gate's reason can clear, every built op
/// goes back to the queue and the next merge of this branch runs alone; a
/// single built op fails as it would alone; a batch of several is retried in a
/// smaller one — the cap comes back halved, and `min(merge_batch, cap * 2)`
/// after a batch that landed. A merge that cannot be built steps aside on the
/// base the merges before it left behind, as it does in a train.
///
/// The returned cap is what this repo's next batch starts with.
#[allow(clippy::too_many_arguments)]
fn run_batch(
    worktree: &gear_herdr::Worktree,
    repo: &Repo,
    ops: &[TrainOp],
    remote: Target<'_>,
    db_path: &str,
    single: &std::sync::atomic::AtomicBool,
    batch: usize,
    cap: usize,
) -> (Vec<(OpRow, Report)>, usize) {
    use std::sync::atomic::Ordering;
    let dir = Path::new(&worktree.path);
    let collect = |reports: Vec<Option<Report>>| -> Vec<(OpRow, Report)> {
        ops.iter()
            .zip(reports)
            .map(|(batch_op, report)| {
                (
                    batch_op.op.clone(),
                    report.unwrap_or_else(|| Report::Requeue("it waits for the next batch".into())),
                )
            })
            .collect()
    };
    let lock = match merge_lock(dir) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            return (
                ops.iter().map(|t| (t.op.clone(), Report::Busy)).collect(),
                cap,
            )
        }
        Err(reason) => {
            return (
                ops.iter()
                    .map(|t| {
                        (
                            t.op.clone(),
                            Report::Requeue(format!("merge lock: {reason}")),
                        )
                    })
                    .collect(),
                cap,
            )
        }
    };
    let git = |dir: &Path, args: &[&str]| git_with_lock(dir, args, Some(&lock));
    let integration = repo.integration.clone();
    let remote_name = remote.name;
    let first_base = format!("{remote_name}/{integration}");
    let mut reports: Vec<Option<Report>> = vec![None; ops.len()];
    // The merges that built, in order: the op's index, its prepared merge, the
    // paths its commits added, and its probe. The next task names the culprit
    // of a red batch from these paths (T-548).
    let mut built: Vec<(usize, Prepared, Vec<String>, Probe)> = Vec::new();
    // The commit each built merge left at the head, in the same order: its own
    // line names it (F2 of the review).
    let mut heads: Vec<String> = Vec::new();
    let mut base: Option<String> = None;
    // T-573: the members that carry only reports merge first. They need no
    // gate, so a gated member's red gate never holds them back; `changed_paths`
    // before a build reads the candidate against the remote integration branch,
    // and a candidate that is not here yet counts as gated, never as a report.
    let order: Vec<usize> = {
        let mut only = Vec::new();
        let mut gated = Vec::new();
        for (index, batch_op) in ops.iter().enumerate() {
            let candidate = batch_op.op.payload["candidate"]
                .as_str()
                .unwrap_or_default();
            let paths = if candidate.is_empty() {
                Vec::new()
            } else {
                changed_paths(&git, dir, &first_base, candidate)
            };
            if reports_only(&paths, &repo.voyage_dir, &batch_op.op.project) {
                only.push(index);
            } else {
                gated.push(index);
            }
        }
        only.extend(gated);
        only
    };
    for index in order {
        let batch_op = &ops[index];
        let mut probe = Probe::new(
            batch_op.op.task.as_deref().unwrap_or("-"),
            batch_op.op.created,
            db_path,
            &batch_op.op.id,
        );
        let before = base.clone().unwrap_or_else(|| first_base.clone());
        match prepare_merge(
            worktree,
            repo,
            &batch_op.op,
            &lock,
            &batch_op.others,
            remote,
            &batch_op.review_fallback,
            &batch_op.commit,
            &batch_op.review_prefix,
            &mut probe,
            base.as_deref(),
        ) {
            Ok(prepared) => {
                let head = match git(dir, &["rev-parse", "HEAD"]) {
                    Ok(head) => head.trim().to_string(),
                    Err(reason) => {
                        probe.done();
                        reports[index] = Some(Report::Requeue(format!(
                            "the batch could not read its merge: {reason}"
                        )));
                        let _ = git(dir, &["reset", "--hard", &before]);
                        continue;
                    }
                };
                let changed: Vec<String> = git(dir, &["diff", "--name-only", &before, &head])
                    .map(|paths| {
                        paths
                            .lines()
                            .map(str::trim)
                            .filter(|path| !path.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                heads.push(head.clone());
                base = Some(head);
                let mut prepared = prepared;
                // T-573: the merge's own changed files, its review commit
                // included, decide whether a gate is needed at all.
                prepared.no_gate = reports_only(&changed, &repo.voyage_dir, &batch_op.op.project);
                built.push((index, prepared, changed, probe));
            }
            // A merge that cannot be built steps aside (as in a train): the
            // next one is built on the same base, so it does not wait behind a
            // conflict.
            Err(step) => {
                probe.done();
                reports[index] = Some(match step {
                    Step::Failed(failed) if failed.blocker == Blocker::Clear => {
                        single.store(true, Ordering::SeqCst);
                        Report::Requeue(failed.reason)
                    }
                    Step::Merged(line) if base.is_none() => Report::Merged(line),
                    Step::Merged(_) => {
                        Report::Requeue("its work is in a merge ahead of it in the batch".into())
                    }
                    other => step_report(other),
                });
                // Back to where the last good merge left the worktree: a
                // failed prepare may have reset it to the remote.
                let _ = git(dir, &["reset", "--hard", &before]);
            }
        }
    }
    if built.is_empty() {
        return (collect(reports), cap);
    }
    let built_count = built.len();
    // One gate for the whole batch, named by its first merge and judging every
    // merge it built (T-555).
    let project = ops[0].op.project.clone();
    let round = gate_round(&ops[0].op);
    let tasks: Vec<String> = built
        .iter()
        .map(|(index, ..)| ops[*index].op.task.clone().unwrap_or_default())
        .collect();
    // T-573: no gated member, no gate: the batch pushes the way a report-only
    // single merge does.
    let gated = built.iter().any(|(_, prepared, ..)| !prepared.no_gate);
    if gated {
        if let Err(step) = run_gate(
            worktree,
            repo,
            &lock,
            remote,
            &project,
            &round,
            &tasks,
            &mut built[0].3,
        ) {
            let clear = matches!(&step, Step::Failed(failed) if failed.blocker == Blocker::Clear);
            if clear {
                single.store(true, Ordering::SeqCst);
            }
            // T-548: a red batch names its culprit when the gate says which paths
            // failed. The gate may write the repo-relative files or directories
            // whose tests failed to `$GEAR_GATE_FAILED`, one per line; a built merge
            // whose changed paths hold one of them is a suspect. Exactly one
            // suspect fails with the gate's reason and the others merge again
            // without it; with no suspect, several, or no file the batch is halved
            // as before. The file is read once and removed, so a line an earlier
            // gate left is never read here.
            let failed_paths = if clear {
                Vec::new()
            } else {
                gate_failed_paths(dir)
            };
            let suspects: Vec<(usize, Vec<String>)> = if failed_paths.is_empty() {
                Vec::new()
            } else {
                built
                    .iter()
                    .filter_map(|(index, _, changed, _)| {
                        let matching = matching_paths(&failed_paths, changed);
                        (!matching.is_empty()).then_some((*index, matching))
                    })
                    .collect()
            };
            if built_count > 1 && suspects.len() == 1 {
                let (culprit, matching) = &suspects[0];
                let culprit = *culprit;
                let task = ops[culprit].op.task.clone().unwrap_or_default();
                let (gate_reason, blocker) = match &step {
                    Step::Failed(failed) => (failed.reason.clone(), failed.blocker),
                    _ => (String::new(), Blocker::Stuck),
                };
                let reason = format!(
                    "{gate_reason}\nthe batch's failed paths match only {task}'s changes: {}",
                    matching.join(", ")
                );
                for (index, _, _, mut probe) in built {
                    probe.done();
                    reports[index] = Some(if index == culprit {
                        Report::Failed(Failed {
                            reason: reason.clone(),
                            blocker,
                        })
                    } else {
                        Report::Requeue(format!(
                            "its batch failed on {task}'s changes: it merges again without it"
                        ))
                    });
                }
                // The culprit is known: the batch is not halved.
                return (collect(reports), cap);
            }
            let red = if clear {
                match &step {
                    Step::Failed(failed) => failed.reason.clone(),
                    _ => "the batch could not be gated".to_string(),
                }
            } else {
                "its batch failed the gate: it is gated again in a smaller batch".to_string()
            };
            // A single built merge fails as it would alone (the gate's own end).
            let alone = step_report(step);
            for (index, _, _, mut probe) in built {
                probe.done();
                reports[index] = Some(if clear || built_count > 1 {
                    Report::Requeue(red.clone())
                } else {
                    // A single built merge fails as it would alone.
                    alone.clone()
                });
            }
            let cap = if clear || built_count == 1 {
                cap
            } else {
                built_count.div_ceil(2)
            };
            return (collect(reports), cap);
        }
    }
    // Green: one push lands the whole batch, as step 9b of a single merge.
    for (_, _, _, probe) in built.iter_mut() {
        probe.begin("push");
    }
    if let Err(reason) = remote.push(dir, &integration) {
        // A push the remote rejects because the integration branch moved is a
        // race, read exactly as step 9b of `land_merge` reads it (F1 of the
        // review): the batch goes back to the queue with the race reason and
        // the next merge of this branch still batches.
        let reason = if reason.contains("non-fast-forward")
            || reason.contains("fetch first")
            || reason.contains("cannot lock ref")
        {
            format!("{RACE}{reason}")
        } else {
            format!("push failed: {reason}")
        };
        if !lost_the_race(&reason) {
            single.store(true, Ordering::SeqCst);
        }
        for (index, _, _, mut probe) in built {
            probe.step("push");
            probe.done();
            reports[index] = Some(Report::Requeue(reason.clone()));
        }
        return (collect(reports), cap);
    }
    // Green: each merge of the batch closes its branches and says so.
    for ((index, prepared, _changed, mut probe), head) in built.into_iter().zip(heads) {
        probe.step("push");
        probe.begin("close");
        let step = closed_and_line(
            dir,
            repo,
            remote,
            &integration,
            remote_name,
            &prepared,
            Some(&head),
            |dir, args| git(dir, args),
        );
        probe.step("close");
        probe.done();
        reports[index] = Some(step_report(step));
    }
    (collect(reports), batch.min(cap.saturating_mul(2)))
}

/// A merge's end, as the worker reports it.
fn step_report(step: Step) -> Report {
    match step {
        Step::Merged(line) => Report::Merged(line),
        Step::Pr { pr, head } => Report::Pr { pr, head },
        Step::Waiting(tasks) => Report::Waiting(tasks),
        Step::Raced(reason) => Report::Raced(reason),
        Step::Failed(failed) => Report::Failed(failed),
    }
}

/// The tasks whose work the candidate carries (T-62): their candidates are
/// ancestors of this candidate and are not on the integration branch yet. They
/// must land first: a task that was built on another task's branch must not
/// carry that task's unreviewed commits into the integration branch.
///
/// Each task comes with whether it has passed its review: a task that has
/// not passed is the captain's business, a task that has passed is only
/// waiting for its own merge.
fn carried_tasks(
    dir: &Path,
    git: &impl Fn(&Path, &[&str]) -> Result<String, String>,
    ahead: &str,
    candidate: &str,
    others: &[(String, String, bool)],
) -> Vec<(String, bool)> {
    let ahead: std::collections::BTreeSet<&str> = ahead
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let mut carried: Vec<(String, bool)> = Vec::new();
    for (task, other, passed) in others {
        // One entry per task: a task that handed in several attempts is one
        // task to wait for, not three (F1 of the T-62 review).
        if carried.iter().any(|(seen, _)| seen == task) {
            continue;
        }
        // The commit of the other task, as this repo knows it. A candidate of
        // another repo is not here at all: git cannot place it, and it cannot
        // be carried either. A full sha — what an op carries — needs no git
        // process to be read (T-127).
        let other_sha = match git_sha(other) {
            Some(sha) => sha,
            None => match git(
                dir,
                &["rev-parse", "--verify", &format!("{other}^{{commit}}")],
            ) {
                Ok(out) => out.trim().to_string(),
                Err(_) => continue,
            },
        };
        // The same commit is not carried work: two tasks may hand in the same
        // one, and neither of them waits for the other.
        if other_sha == candidate {
            continue;
        }
        // Carried and not landed is exactly what `ahead` holds: if the commit
        // is not there, it is either not in this candidate at all (not
        // carried) or already on the integration branch (landed).
        if !ahead.contains(other_sha.as_str()) {
            continue;
        }
        carried.push((task.clone(), *passed));
    }
    carried
}

/// `text` when it is already a full commit sha: an op names its candidate by
/// one, and reading it costs no git process (T-127).
fn git_sha(text: &str) -> Option<String> {
    let text = text.trim();
    (text.len() == 40 && text.chars().all(|c| c.is_ascii_hexdigit())).then(|| text.to_string())
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
/// What `for-each-ref --contains=<candidate>` answered about the remote: the refs
/// that hold the candidate, and those whose tip it is. One git process answers
/// the "is it pushed" question and gives the branch-closing step its list
/// (T-127).
#[derive(Debug, Default)]
struct Pushed {
    refs: Vec<String>,
    tips: Vec<String>,
}

impl Pushed {
    /// Read the `%(refname:short) %(objectname)` lines of the listing.
    fn read(listing: &str, candidate: &str) -> Pushed {
        let mut pushed = Pushed::default();
        for line in listing.lines() {
            let mut parts = line.split_whitespace();
            let (Some(name), Some(tip)) = (parts.next(), parts.next()) else {
                continue;
            };
            pushed.refs.push(name.to_string());
            if tip == candidate {
                pushed.tips.push(name.to_string());
            }
        }
        pushed
    }
}

/// The pull request's head (design §11.2 step 2): the candidate with the
/// review's commits on it, pushed to the task's branch. The push is a
/// fast-forward of the pull request's head: the head is the candidate plus
/// the review, and the candidate is what the branch held.
///
/// Nothing of the integration branch is touched: the pull request is merged
/// by the forge, into the integration branch of the pull request.
fn pr_head(
    dir: &Path,
    git: &impl Fn(&Path, &[&str]) -> Result<String, String>,
    candidate: &str,
    review_ref: &str,
    has_review: bool,
    pr: &PrTarget,
    remote: Target<'_>,
    commit: &str,
) -> Step {
    // The candidate is the base of the head: the review's commits go on top
    // of it, and the push is a fast-forward of the branch the claim opened.
    if let Err(reason) = git(dir, &["reset", "--hard", candidate]) {
        return Step::Failed(Failed::stuck(format!(
            "reset to the candidate {candidate}: {reason}"
        )));
    }
    // The review branch is merged only when it holds commits the candidate
    // does not have (design §11.2 step 2).
    if has_review && !is_ancestor(dir, review_ref, candidate) {
        if let Err(reason) = git(
            dir,
            &[
                "merge",
                "--no-ff",
                review_ref,
                "-m",
                &gear_model::commit::fill(commit, &[("source", review_ref)]),
            ],
        ) {
            let _ = git(dir, &["merge", "--abort"]);
            return Step::Failed(Failed::stuck(format!(
                "conflict merging {review_ref}: {reason}"
            )));
        }
    }
    let head = match git(dir, &["rev-parse", "HEAD"]) {
        Ok(head) => head.trim().to_string(),
        Err(reason) => {
            return Step::Failed(Failed::clear(format!(
                "the head of the pull request: {reason}"
            )))
        }
    };
    // A push that was made and not confirmed (the engine stopped, the answer
    // was lost) leaves the branch with a head that is the engine's own merge
    // of the candidate and the review, but with another sha (the commit's
    // timestamp is the clock). Such a head is the head the gate checked: it
    // is reused, and nothing is pushed (a push would not be a fast-forward).
    // A head that is anything else is refused: the branch moved on, and the
    // engine never forces a push.
    let branch_ref = format!("{}/{branch}", remote.name, branch = pr.branch);
    if let Some(pushed) = git(dir, &["rev-parse", "--verify", &branch_ref]).ok() {
        let pushed = pushed.trim().to_string();
        if pushed != head && is_own_head(dir, &pushed, &head) {
            return Step::Pr {
                pr: pr.number,
                head: pushed,
            };
        }
    }
    // The head goes onto the pull request's branch (a fast-forward of it).
    if let Err(reason) = remote.push(dir, &pr.branch) {
        if reason.contains("non-fast-forward")
            || reason.contains("fetch first")
            || reason.contains("cannot lock ref")
        {
            return Step::Failed(Failed::clear(format!("{RACE}{reason}")));
        }
        return Step::Failed(Failed::clear(format!("push failed: {reason}")));
    }
    Step::Pr {
        pr: pr.number,
        head,
    }
}

/// Is the commit `pushed` the engine's own merge that `head` is? The two
/// must have the same parents and the same tree: the engine's merge of the
/// candidate and the review, built twice (the commit's timestamp is the
/// clock, so the two shas differ). A head that holds anything else — another
/// commit on the branch — is not the engine's and is never reused.
fn is_own_head(dir: &Path, pushed: &str, head: &str) -> bool {
    let parents = |commit: &str| -> Vec<String> {
        git(dir, &["rev-list", "--parents", "-n", "1", commit])
            .ok()
            .map(|line| {
                line.split_whitespace()
                    .skip(1)
                    .map(|parent| parent.to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    let tree = |commit: &str| -> String {
        git(dir, &["rev-parse", &format!("{commit}^{{tree}}")])
            .map(|tree| tree.trim().to_string())
            .unwrap_or_default()
    };
    let pushed_parents = parents(pushed);
    let head_parents = parents(head);
    !pushed_parents.is_empty() && pushed_parents == head_parents && tree(pushed) == tree(head)
}

/// Is `a` an ancestor of `b`? (`git merge-base --is-ancestor`.)
fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    git(dir, &["merge-base", "--is-ancestor", a, b]).is_ok()
}

/// The method a pull request is merged with (design §11.1): the repo's
/// `merge_method`, `squash` by default.
fn merge_method_of(repo: &Repo) -> &'static str {
    repo.github
        .as_ref()
        .map(|github| github.merge_method.as_str())
        .unwrap_or("squash")
}

/// The land op of a task (design §11.2 step 3): the forge op that merges the
/// pull request, with the head the gate checked.
fn insert_land_op(
    tx: &gear_store::Tx<'_>,
    project: &str,
    op: &OpRow,
    pr: u64,
    head: &str,
    method: &str,
    now: Ts,
) -> GearResult<()> {
    let task = op.task.clone().unwrap_or_default();
    let attempt = op.attempt.unwrap_or(1);
    let key = format!("{task}:{attempt}:land");
    let seat = format!("{project}:forge");
    // One land op per task and attempt: the op the sync runs is the one the
    // engine inserted. A failed or void land op is followed by a new one: the
    // captain's `- remerge` retries the landing (design §11.3, row 2), and
    // the new op carries the head and the method of this attempt.
    if tx.ops_of_seat_kind(&seat, OpKind::Forge)?.iter().any(|op| {
        op.payload.get("key").and_then(|key| key.as_str()) == Some(key.as_str())
            && !matches!(op.state, OpState::Failed | OpState::Void)
    }) {
        return Ok(());
    }
    tx.insert_op(&NewOp {
        project: project.to_string(),
        seat,
        generation: 0,
        kind: OpKind::Forge,
        task: Some(task),
        attempt: Some(attempt),
        ref_msg: None,
        plan_version: None,
        payload: serde_json::json!({
            "forge": "land",
            "key": key,
            "pr": pr,
            "head": head,
            "method": method,
            "attempts": 1,
        }),
        created: now,
    })?;
    Ok(())
}

fn close_branches(
    dir: &Path,
    repo: &Repo,
    task_branch: &str,
    review: &str,
    remote: Target<'_>,
    pushed: &Pushed,
    keep_branch: bool,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut closed = Vec::new();
    let mut left = Vec::new();
    // T-474: the task's own branches closed although their commits past the
    // approved candidate were not merged; the commits are named to the
    // captain with them.
    let mut moved = Vec::new();
    // The review branch: it holds the review of this task and nothing else.
    // T-483: it is closed whatever its tip holds. A review branch left by an
    // earlier attempt of the task does not contain the candidate, and the
    // merge that lands this attempt still consumed the review; the earlier
    // attempt's review stays in that attempt's record.
    if delete_remote(dir, review, remote) {
        closed.push(review.to_string());
    }
    // The branches whose tip is the candidate: the work the review approved.
    for name in &pushed.tips {
        let name = name
            .trim()
            .trim_start_matches(&format!("{}/", remote.name))
            .to_string();
        if name.is_empty() || name == "HEAD" {
            continue;
        }
        if name == repo.integration || name == repo.release {
            continue;
        }
        // T-582: the feature's branch is not a task branch to close.
        if keep_branch && name == task_branch {
            continue;
        }
        if name != task_branch && !is_task_branch(&name, task_branch) {
            // Not this task's branch: it stays, and the captain is told.
            left.push(name);
            continue;
        }
        if delete_remote(dir, &name, remote) || !branch_on_remote(dir, remote, &name) {
            // T-540: a delete that failed because the branch was already gone
            // (another merge, the captain) is a closed branch, not one left.
            if !closed.contains(&name) {
                closed.push(name);
            }
        } else if !left.contains(&name) {
            // It is still there: the captain hears of it.
            left.push(name);
        }
    }
    // The task's own branch whose tip moved past the candidate (a push after
    // the hand-in): when every commit of it is on the merged integration
    // branch it is closed too; when its commits past the candidate write
    // nothing but reports it is closed as well (a lane often pushes its report
    // after the hand-in, T-402); otherwise T-474 closes it anyway — a merged
    // task leaves no branch behind — and names its commits past the candidate
    // to the captain (the commit is kept by the remote's history until it is
    // pruned, and the lane's own branch has it).
    let task_ref = format!("{}/{task_branch}", remote.name);
    if !keep_branch
        && !closed.iter().any(|name| name == task_branch)
        && !left.iter().any(|name| name == task_branch)
        && pushed.refs.iter().any(|name| name == &task_ref)
    {
        if git(dir, &["merge-base", "--is-ancestor", &task_ref, "HEAD"]).is_ok() {
            if delete_remote(dir, task_branch, remote)
                || !branch_on_remote(dir, remote, task_branch)
            {
                closed.push(task_branch.to_string());
            } else {
                // The delete did not go through: the branch stays, and the
                // captain hears of it.
                left.push(task_branch.to_string());
            }
        } else {
            let changed = git(dir, &["diff", "--name-only", &format!("HEAD...{task_ref}")])
                .unwrap_or_default();
            let files: Vec<&str> = changed
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect();
            let ahead =
                git(dir, &["log", "--oneline", &format!("HEAD..{task_ref}")]).unwrap_or_default();
            let commits: Vec<&str> = ahead
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            if delete_remote(dir, task_branch, remote) {
                if !files.is_empty() && files.iter().all(|file| is_report_path(file)) {
                    closed.push(task_branch.to_string());
                } else {
                    moved.push(format!(
                        "{task_branch} (commits after the approved candidate, not merged: {})",
                        commits.join("; ")
                    ));
                }
            } else if !branch_on_remote(dir, remote, task_branch) {
                // T-540: the branch was already gone; the delete failed because
                // there was nothing to delete.
                closed.push(task_branch.to_string());
            } else {
                // The delete did not go through: the branch stays, and the
                // captain hears of the commits it holds.
                left.push(format!(
                    "{task_branch} (commits after the approved candidate, not merged: {})",
                    commits.join("; ")
                ));
            }
        }
    }
    (closed, left, moved)
}

/// The files a merge changes between `base` and `head` (T-573).
fn changed_paths(
    git: &impl Fn(&Path, &[&str]) -> Result<String, String>,
    dir: &Path,
    base: &str,
    head: &str,
) -> Vec<String> {
    git(dir, &["diff", "--name-only", base, head])
        .map(|paths| {
            paths
                .lines()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// T-573: does this merge change nothing but reports? Every changed file must
/// lie under the project's voyage folder (`[conventions].voyage_dir` with
/// `{project}` filled and `{milestone}` any folder), and a merge that changes
/// nothing at all is not a report-only merge.
fn reports_only(paths: &[String], voyage_dir: &str, project: &str) -> bool {
    !paths.is_empty()
        && paths
            .iter()
            .all(|path| is_under_voyage(path, voyage_dir, project))
}

/// Is `path` under the project's voyage folder, with `{milestone}` matching any
/// one folder? The templates GEAR validates name `{milestone}`, so a template
/// without it is no voyage folder at all.
fn is_under_voyage(path: &str, voyage_dir: &str, project: &str) -> bool {
    let template = voyage_dir.replace("{project}", project);
    let Some(at) = template.find("{milestone}") else {
        return false;
    };
    let prefix = &template[..at];
    let suffix = &template[at + "{milestone}".len()..];
    let Some(rest) = path.strip_prefix(prefix) else {
        return false;
    };
    let Some((milestone, _)) = rest.split_once('/') else {
        return false;
    };
    if milestone.is_empty() {
        return false;
    }
    // `{milestone}` is one folder, never a prefix of another: what follows it
    // is the template's own suffix, then the file.
    let Some(tail) = rest[milestone.len()..].strip_prefix(suffix) else {
        return false;
    };
    tail.starts_with('/') && tail.len() > 1
}

/// A file a lane writes its report to: it lies in a `reports/` directory, a
/// path segment `/reports/` (T-402). Commits past the approved candidate that
/// change nothing but such files hold no work the review did not see.
fn is_report_path(path: &str) -> bool {
    path.contains("/reports/")
}

/// A branch of a task: the task's own branch, or that branch with a suffix
/// (`t53-v2`, a rework round). Both are the **whole** branch expanded from the
/// project's template (T-253), never a segment of it: with
/// `task_branch = "tasks/{n}/work"` the branch `tasks/2/work` is another
/// task's branch, and only the prefix can tell it from `tasks/1/work` (F1 of
/// the T-253 review — a segment match deleted it). Anything else is retained.
fn is_task_branch(branch: &str, task_branch: &str) -> bool {
    branch == task_branch || branch.starts_with(&format!("{task_branch}-"))
}

/// Close a branch on the remote. A branch that is already gone is closed too.
fn delete_remote(dir: &Path, branch: &str, remote: Target<'_>) -> bool {
    remote.delete(dir, branch)
}

/// Whether the remote still has `branch` (T-540): asked before a failed delete
/// is reported as a branch left on the remote.
fn branch_on_remote(dir: &Path, remote: Target<'_>, branch: &str) -> bool {
    git(
        dir,
        &["ls-remote", "--exit-code", "--heads", remote.name, branch],
    )
    .is_ok()
}

/// The gate of the project: its build and test command, in `dir`.
///
/// The gate runs without `GEAR_DB` and without `GEAR_CTX`: it is the
/// project's own build and test command, and it must never see the store of
/// this machine (L-59). A test that inherited `GEAR_DB` would write to the
/// production store; the tests give themselves a scratch home instead.
fn gate_command(
    gate: &str,
    dir: &Path,
    project: &str,
    integration: &str,
    round: &str,
    tasks: &[String],
) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(gate)
        .current_dir(dir)
        .env_remove(gear_model::ctx::ENV_DB)
        .env_remove(gear_model::ctx::ENV_CTX)
        .env(GATE_TEMPFAIL_ENV, gate_tempfail_path(dir))
        .env(GATE_FAILED_ENV, gate_failed_path(dir))
        .env(GATE_PROJECT_ENV, project)
        .env(GATE_INTEGRATION_ENV, integration)
        .env(GATE_ROUND_ENV, round)
        .env(GATE_TASKS_ENV, tasks.join(" "));
    cmd
}

/// The variable naming the file a gate creates when it could not judge this
/// time (a CI service refused a request): the merge is retried, not failed.
/// It reaches the engine through any wrapper (`make` turns a recipe's exit 75
/// into its own exit 2), so it is the bridge for gates run through one.
pub const GATE_TEMPFAIL_ENV: &str = "GEAR_GATE_TEMPFAIL";

/// The file of [`GATE_TEMPFAIL_ENV`] for a gate run in `dir`.
fn gate_tempfail_path(dir: &Path) -> std::path::PathBuf {
    dir.join(".git-gear-gate-tempfail")
}

/// The variable naming the file a gate writes the repo-relative paths of the
/// files or directories whose tests failed to, one per line (T-548): a red
/// batch merge reads it to name the task whose changes match them, and `fn
/// run_gate` removes it before the gate runs, so only this run's lines are
/// read. A gate that writes nothing leaves no file, which is fine.
pub const GATE_FAILED_ENV: &str = "GEAR_GATE_FAILED";

/// The file of [`GATE_FAILED_ENV`] for a gate run in `dir`.
fn gate_failed_path(dir: &Path) -> std::path::PathBuf {
    dir.join(".git-gear-gate-failed")
}

/// The variable naming the project whose merge this gate run judges (T-555):
/// a project's gate can name the CI branch it uses, and read the run it is.
pub const GATE_PROJECT_ENV: &str = "GEAR_GATE_PROJECT";

/// The variable naming the integration branch this gate run merges into: a
/// CI that compares a candidate with the integration branch reads it here, so
/// projects that share a repository with their own branches are each judged
/// against their own.
pub const GATE_INTEGRATION_ENV: &str = "GEAR_GATE_INTEGRATION";

/// The variable naming this gate run (T-555): the id of its first merge op,
/// lowercase without the dash — `O-6543` is `o6543` — so every gate run of a
/// project has a name of its own to give its CI work.
pub const GATE_ROUND_ENV: &str = "GEAR_GATE_ROUND";

/// The variable naming the tasks whose merges this gate run judges (T-555),
/// space-separated in merge order (`T-637 T-646 T-662`): a single merge names
/// one task, a batch every merge it built.
pub const GATE_TASKS_ENV: &str = "GEAR_GATE_TASKS";

/// The round of the gate run that starts at `op` (T-555): its id, lowercase
/// without the dash.
fn gate_round(op: &OpRow) -> String {
    op.id.to_lowercase().replace('-', "")
}

/// The paths a failed gate named (T-548): the repo-relative files or
/// directories whose tests failed, one per line, as the gate wrote them to the
/// file [`GATE_FAILED_ENV`] names — trimmed, empty lines skipped, a trailing
/// `/` dropped. The file belongs to the run that wrote it: it is read once and
/// removed here, so a line an earlier gate left behind is never read.
fn gate_failed_paths(dir: &Path) -> Vec<String> {
    let path = gate_failed_path(dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let _ = std::fs::remove_file(&path);
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.strip_suffix('/').unwrap_or(line).to_string())
        .filter(|path| !path.is_empty())
        .collect()
}

/// The listed paths that name one of `changed` (T-548): a listed path is a
/// changed path itself, or the directory one of them is in.
fn matching_paths(listed: &[String], changed: &[String]) -> Vec<String> {
    listed
        .iter()
        .filter(|listed| {
            changed
                .iter()
                .any(|changed| *changed == **listed || changed.starts_with(&format!("{listed}/")))
        })
        .cloned()
        .collect()
}

/// Run one git command in `dir`; the output tail comes back as the error.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_with_lock(dir, args, None)
}

pub(crate) fn git_with_lock(
    dir: &Path,
    args: &[&str],
    lock: Option<&File>,
) -> Result<String, String> {
    let mut command = Command::new("git");
    // The engine is not a seat: its git runs — and any hook a repository
    // brings — must not see the seat identity of whoever started the engine
    // (T-97). The engine's own push is checked in this process (`push_allowed`)
    // before it runs.
    command
        .args(args)
        .current_dir(dir)
        .env_remove("GEAR_CTX")
        .env_remove("GEAR_DB")
        .env_remove("GEAR_ADMIN");
    // A connection the host stops answering ends within about a minute
    // instead of holding the merge for ever (2026-11-04: a fetch hung on
    // GitHub SSH for 7 minutes). A user's own ssh command is kept.
    if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
        command.env(
            "GIT_SSH_COMMAND",
            "ssh -o ConnectTimeout=30 -o ServerAliveInterval=15 -o ServerAliveCountMax=4",
        );
    }
    if let Some(lock) = lock {
        command.stdin(
            lock.try_clone()
                .map_err(|error| format!("git lock: {error}"))?,
        );
    }
    let output = command
        .output()
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

/// T-563: a gate's output without the remote's chatter: the lines git prints
/// for a push — `remote: …` (the remote's own messages), `To …` (the target),
/// and the branch lines ` * [new branch] …` / ` - [deleted] …`. A failed gate
/// keeps its own refusal first instead of leading with lines nobody can act on.
fn without_push_noise(text: &str) -> String {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !(line.starts_with("remote:")
                || line.starts_with("To ")
                || trimmed.starts_with("* [new branch]")
                || trimmed.starts_with("- [deleted]"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The lines of a gate's output that name a failure (T-219). A line counts
/// when it starts (after spaces) with `FAIL`, `TIMEOUT` or `SIGSEGV`, or when
/// it holds `panicked at`, `error[` or `error:`. The lines keep their order,
/// and at most `max` of them come back: a gate that fails on everything names
/// nothing in particular.
fn failure_lines(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let starts = trimmed.starts_with("FAIL")
            || trimmed.starts_with("TIMEOUT")
            || trimmed.starts_with("SIGSEGV");
        let holds =
            line.contains("panicked at") || line.contains("error[") || line.contains("error:");
        if (starts || holds) && out.len() < max {
            out.push(line.to_string());
        }
    }
    out
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
            meta: None,
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

/// The cause of a merge wait, as one item per class (T-566).
///
/// A merge that cannot start retries every tick, and each retry used to open
/// its own item: 47 of them for one worktree that git could not create. One
/// item per project and cause class says the same thing once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeWaitCause {
    /// The merge worktree: it could not be created, opened, or it belongs to
    /// another repository.
    Worktree,
    /// Anything else the merge step named.
    Other,
}

impl MergeWaitCause {
    /// The class a reason belongs to: the worktree steps name themselves in
    /// their reason (`worktree_create: refused …`, `the worktree … belongs to
    /// another repository …`).
    fn of(reason: &str) -> MergeWaitCause {
        if reason.to_lowercase().contains("worktree") {
            MergeWaitCause::Worktree
        } else {
            MergeWaitCause::Other
        }
    }

    /// The thread's last part.
    fn as_str(self) -> &'static str {
        match self {
            MergeWaitCause::Worktree => "worktree",
            MergeWaitCause::Other => "other",
        }
    }

    /// Every class: closing a wait closes all of them, since a merge that
    /// starts has left every cause behind.
    fn all() -> [MergeWaitCause; 2] {
        [MergeWaitCause::Worktree, MergeWaitCause::Other]
    }
}

/// The thread of a project's merge wait for one cause class (T-566):
/// `merge-wait:<project>:worktree`.
fn merge_wait_thread(project: &str, cause: MergeWaitCause) -> String {
    format!("merge-wait:{project}:{}", cause.as_str())
}

/// The open item of a thread, if there is one.
fn open_item(
    tx: &gear_store::Tx<'_>,
    project: &str,
    thread: &str,
) -> GearResult<Option<gear_store::MessageRow>> {
    Ok(tx
        .thread_messages(project, thread)?
        .into_iter()
        .find(|message| matches!(message.state, MsgState::Open | MsgState::Waiting)))
}

/// Tell the captain that the merge waits, once per project and cause class
/// (T-566): the body follows the latest reason while the cause lasts, and the
/// item closes when a merge of the project next starts.
fn tell_captain_merge_wait(
    store: &mut Store,
    project: &str,
    reason: &str,
    now: Ts,
) -> GearResult<()> {
    let cause = MergeWaitCause::of(reason);
    let body = format!("merge waits: {reason}");
    store.write(|tx| {
        let captain = tx.captain_of(project)?.ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!("project {project} has no captain"),
            )
        })?;
        let thread = merge_wait_thread(project, cause);
        if let Some(item) = open_item(tx, project, &thread)? {
            if item.body != body {
                tx.update_message_body(project, &item.id, &body)?;
            }
            return Ok(());
        }
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.to_string(),
            thread,
            task: None,
            attempt: None,
            sender: captain.id.clone(),
            sender_gen: 1,
            recipient: captain.id,
            kind: MsgKind::Question,
            urgent: false,
            body,
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        Ok(())
    })
}

/// Why a merge wait's item closes: a merge of the project started (T-566).
const MERGE_WAIT_OVER: &str = "a merge of this project started, so the wait is over";

/// Close the project's merge-wait items: a merge op started, so whatever held
/// it is gone (T-566).
fn close_merge_waits(store: &mut Store, project: &str, now: Ts) -> GearResult<()> {
    store.write(|tx| {
        for cause in MergeWaitCause::all() {
            let thread = merge_wait_thread(project, cause);
            let Some(item) = open_item(tx, project, &thread)? else {
                continue;
            };
            tx.update_message_state(
                project,
                &item.id,
                MsgState::Resolved,
                Some(MERGE_WAIT_OVER),
                None,
                None,
                now,
            )?;
        }
        Ok(())
    })
}

/// How often the branch sweep asks the remote for its branches: it is a
/// network call, so not on every tick.
const BRANCH_SWEEP_EVERY_S: i64 = 600;

/// When the branch sweep last asked the remote of a project.
fn branch_sweep_key(project: &str) -> String {
    format!("engine.branch_sweep.{project}")
}

/// When the captain was last told about a branch that is too old: told again
/// only after another `branch_max_age_min`.
fn branch_told_key(project: &str, branch: &str) -> String {
    format!("engine.branch_told.{project}.{branch}")
}

/// A branch on the remote that belongs to a task of the project: the task's
/// own branch (or a suffixed one) or its review branch.
#[derive(Debug, Clone)]
struct TaskBranch {
    name: String,
    sha: String,
    task: String,
    state: TaskState,
    /// When the task's first attempt started: the branch is that old.
    since: Option<Ts>,
    /// A review branch (the reviewers'), not a branch a lane cut.
    review: bool,
    /// The task's voyage is still sailing: one of its tasks is not terminal.
    open_voyage: bool,
}

/// When the captain was last told that the open voyages hold too many task
/// branches, and how many: told again when the count grows, or after another
/// `branch_max_age_min`.
fn branch_count_told_key(project: &str) -> String {
    format!("engine.branch_count_told.{project}")
}

/// The branch sweep: a branch of a voyage is meant to be merged within the
/// project's `branch_max_age_min` (120 by default). A branch of a cancelled
/// task is deleted on the remote, since nothing will merge it (the captain is
/// told its commit, so it can be restored); a branch older than the limit is
/// named to the captain, who decides what to do with its task.
fn sweep_branches(
    store: &mut Store,
    cfg: &EngineCfg,
    project: &str,
    config: &ProjectConfig,
    now: Ts,
) -> GearResult<()> {
    let last = store
        .read(|tx| tx.get_meta(&branch_sweep_key(project)))?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if now - last < BRANCH_SWEEP_EVERY_S {
        return Ok(());
    }
    store.write(|tx| tx.set_meta(&branch_sweep_key(project), &now.to_string()))?;
    let tasks = store.read(|tx| tx.tasks_of_project(project))?;
    let mut repos: Vec<Repo> = Vec::new();
    for task in &tasks {
        let repo = config.repo_of_task(task);
        if !repos.iter().any(|known| known.path == repo.path) {
            repos.push(repo);
        }
    }
    let remote = Target::of(config, cfg.remote.as_ref());
    for repo in repos {
        let dir = Path::new(&repo.path);
        if !dir.is_dir() {
            continue;
        }
        let Ok(heads) = git(dir, &["ls-remote", "--heads", remote.name]) else {
            continue;
        };
        let found = task_branches(store, project, config, &tasks, &repo, &heads)?;
        // T-474: the integration branch's tip, so a done task's branches can be
        // told apart from the ones that still hold work: fetched (a merged
        // branch's commits are in its history, so the ancestry test finds
        // them). An unreadable integration leaves the done branches listed.
        let integration = git(dir, &["fetch", "--quiet", remote.name, &repo.integration])
            .ok()
            .and_then(|_| git(dir, &["rev-parse", "FETCH_HEAD"]).ok())
            .map(|sha| sha.trim().to_string())
            .filter(|sha| !sha.is_empty());
        sweep_in(
            store,
            project,
            config,
            dir,
            remote,
            found,
            now,
            &repo.integration,
            integration.as_deref(),
        )?;
    }
    Ok(())
}

/// The branches of `heads` (the output of `git ls-remote --heads`) that
/// belong to a task of `repo`.
fn task_branches(
    store: &mut Store,
    project: &str,
    config: &ProjectConfig,
    tasks: &[gear_store::TaskRow],
    repo: &Repo,
    heads: &str,
) -> GearResult<Vec<TaskBranch>> {
    let mut names: Vec<(String, String, &gear_store::TaskRow)> = Vec::new();
    for task in tasks {
        if config.repo_of_task(task).path != repo.path {
            continue;
        }
        // T-474: every lane that held the task, not only the one it names now:
        // a task moved to another lane leaves its old lane's branch behind
        // (another project's T-199: `api/t199`, fully an ancestor of dev).
        let mut lanes: Vec<String> = Vec::new();
        if let Some(lane) = task.lane.as_deref() {
            lanes.push(lane.to_string());
        }
        for attempt in store.read(|tx| tx.attempts_of_task(project, &task.id))? {
            let Some(lane) = attempt.seat.strip_prefix(&format!("{project}:")) else {
                continue;
            };
            if lane.is_empty() || lanes.iter().any(|known| known == lane) {
                continue;
            }
            lanes.push(lane.to_string());
        }
        for lane in lanes {
            names.push((
                branch_of(&config.conventions.task_branch, project, &lane, &task.id),
                review_branch(&config.conventions.review_branch, project, &lane, &task.id),
                task,
            ));
        }
    }
    let mut open = std::collections::HashSet::new();
    for task in tasks {
        if let Some(milestone) = task.milestone.as_deref() {
            if !task.state.is_terminal() {
                open.insert(milestone.to_string());
            }
        }
    }
    let mut found = Vec::new();
    for line in heads.lines() {
        let Some((sha, name)) = line.split_once('\t') else {
            continue;
        };
        let Some(name) = name.trim().strip_prefix("refs/heads/") else {
            continue;
        };
        let Some((_, review, task)) = names
            .iter()
            .find(|(own, review, _)| is_task_branch(name, own) || is_task_branch(name, review))
        else {
            continue;
        };
        let is_review = is_task_branch(name, review);
        let open_voyage = task
            .milestone
            .as_deref()
            .is_some_and(|milestone| open.contains(milestone));
        let since = store
            .read(|tx| tx.attempts_of_task(project, &task.id))?
            .iter()
            .map(|attempt| attempt.started)
            .min();
        found.push(TaskBranch {
            name: name.to_string(),
            sha: sha.trim().to_string(),
            task: task.id.clone(),
            state: task.state,
            since,
            review: is_review,
            open_voyage,
        });
    }
    Ok(found)
}

/// Delete the branches of cancelled tasks and name the branches older than
/// the project's limit, in one item to the captain.
fn sweep_in(
    store: &mut Store,
    project: &str,
    config: &ProjectConfig,
    dir: &Path,
    remote: Target<'_>,
    found: Vec<TaskBranch>,
    now: Ts,
    integration: &str,
    integration_sha: Option<&str>,
) -> GearResult<()> {
    let limit = i64::from(config.conventions.branch_max_age_min.max(1)) * 60;
    // The open voyages' task branches (not the reviewers'), oldest first.
    let mut live: Vec<&TaskBranch> = found
        .iter()
        .filter(|branch| {
            branch.open_voyage && !branch.review && branch.state != TaskState::Cancelled
        })
        .collect();
    live.sort_by_key(|branch| branch.since.unwrap_or(now));
    let cap = config.lanes.len().max(1) * config.conventions.branch_max_per_lane.max(1) as usize;
    let mut crowded: Vec<String> = Vec::new();
    if live.len() > cap {
        let key = branch_count_told_key(project);
        let told = store.read(|tx| tx.get_meta(&key))?;
        let (count, at) = told
            .as_deref()
            .and_then(|value| value.split_once(':'))
            .and_then(|(count, at)| Some((count.parse::<usize>().ok()?, at.parse::<i64>().ok()?)))
            .unwrap_or((0, 0));
        if live.len() > count || now - at >= limit {
            store.write(|tx| tx.set_meta(&key, &format!("{}:{now}", live.len())))?;
            crowded = live
                .iter()
                .map(|branch| {
                    let age = branch.since.map(|since| (now - since) / 60).unwrap_or(0);
                    format!(
                        "{} ({}, {:?}, open {age} min)",
                        branch.name, branch.task, branch.state
                    )
                })
                .collect();
        }
    }
    let mut deleted = Vec::new();
    let mut old = Vec::new();
    // T-474: the branches of done tasks that still hold commits the integration
    // branch does not have; named to the captain, who decides.
    let mut left = Vec::new();
    for branch in found {
        let short = &branch.sha[..branch.sha.len().min(10)];
        if branch.state == TaskState::Cancelled {
            if remote.delete(dir, &branch.name) {
                deleted.push(format!(
                    "{} at {short} ({}, cancelled)",
                    branch.name, branch.task
                ));
            }
            continue;
        }
        // T-474: a done task leaves no branch behind: one whose tip is on the
        // integration branch is deleted, and one whose commits the integration
        // branch does not have is named to the captain with them.
        if branch.state == TaskState::Done {
            let on_integration = integration_sha.is_some_and(|sha| {
                git(dir, &["merge-base", "--is-ancestor", &branch.sha, sha]).is_ok()
            });
            if on_integration {
                if remote.delete(dir, &branch.name) {
                    deleted.push(format!(
                        "{} at {short} ({}, done)",
                        branch.name, branch.task
                    ));
                }
                continue;
            }
            // A done task whose merge is queued or running is landing, not
            // lost: its branch is not news to the captain.
            let landing = store.read(|tx| {
                // A carried task lands with its carrier's merge: until the
                // carrier has merged, its branches are the work in flight.
                if let Some(by) = tx.task_carried_by(project, &branch.task)? {
                    let (carrier_project, carrier) = gear_store::Tx::carrier_of(project, &by);
                    if gear_app::tasks::merged_at(tx, carrier_project, carrier)?.is_none() {
                        return Ok(true);
                    }
                }
                // T-553: `Unknown` too — that is where a merge that waits for
                // another task's merge sits (`Report::Waiting`), and a merge
                // queued behind a dependency lands as surely as one running
                // (2026-11-04: the captain was told "nothing will merge them"
                // for T-548 while its merge waited for T-547).
                for state in [
                    OpState::Intended,
                    OpState::Executing,
                    OpState::Submitted,
                    OpState::Unknown,
                ] {
                    if tx.ops_in_state(project, state)?.iter().any(|op| {
                        op.kind == OpKind::Merge && op.task.as_deref() == Some(branch.task.as_str())
                    }) {
                        return Ok(true);
                    }
                }
                Ok(false)
            })?;
            if landing {
                continue;
            }
            let key = branch_told_key(project, &branch.name);
            let told = store
                .read(|tx| tx.get_meta(&key))?
                .and_then(|value| value.parse::<i64>().ok());
            if told.is_some_and(|at| now - at < limit) {
                continue;
            }
            store.write(|tx| tx.set_meta(&key, &now.to_string()))?;
            let commits = integration_sha
                .map(|sha| {
                    git(
                        dir,
                        &["log", "--oneline", &format!("{sha}..{}", branch.sha)],
                    )
                    .unwrap_or_default()
                })
                .unwrap_or_default();
            let commits: Vec<&str> = commits
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            left.push(format!(
                "{} ({}, done, commits not on {integration}: {})",
                branch.name,
                branch.task,
                if commits.is_empty() {
                    "none readable".to_string()
                } else {
                    commits.join("; ")
                }
            ));
            continue;
        }
        let Some(since) = branch.since else { continue };
        let age = now - since;
        if age < limit {
            continue;
        }
        let key = branch_told_key(project, &branch.name);
        let told = store
            .read(|tx| tx.get_meta(&key))?
            .and_then(|value| value.parse::<i64>().ok());
        if told.is_some_and(|at| now - at < limit) {
            continue;
        }
        store.write(|tx| tx.set_meta(&key, &now.to_string()))?;
        old.push(format!(
            "{} ({}, {:?}, open {} min, tip {short})",
            branch.name,
            branch.task,
            branch.state,
            age / 60
        ));
    }
    if deleted.is_empty() && old.is_empty() && crowded.is_empty() && left.is_empty() {
        return Ok(());
    }
    let mut body = String::new();
    if !crowded.is_empty() {
        body.push_str(&format!(
            "{} task branches of the open voyage(s) are on the remote, over the limit of {cap} ({} lanes × `[conventions] branch_max_per_lane` {}): work is piling up unmerged and branches rot. Clean up: get the oldest reviewed and merged, finish the reworks, cancel what will not land. Oldest first:\n",
            crowded.len(),
            config.lanes.len().max(1),
            config.conventions.branch_max_per_lane.max(1)
        ));
        for line in &crowded {
            body.push_str(&format!("- {line}\n"));
        }
    }
    if !old.is_empty() {
        body.push_str(&format!(
            "{} branch(es) open longer than {} min (`[conventions] branch_max_age_min`): a branch of a voyage is meant to merge within that time, so its task is stuck. For each, find why and act: get it reviewed and merged, send it back with a smaller rework, or cancel the task.\n",
            old.len(),
            limit / 60
        ));
        for line in &old {
            body.push_str(&format!("- {line}\n"));
        }
    }
    if !left.is_empty() {
        body.push_str(&format!(
            "{} branch(es) of done tasks still hold commits the {integration} branch does not have (the task is done, so nothing will merge them): merge or restore them by hand, or delete the branch if the commits write only reports.\n",
            left.len()
        ));
        for line in &left {
            body.push_str(&format!("- {line}\n"));
        }
    }
    if !deleted.is_empty() {
        body.push_str(&format!(
            "{} branch(es) of cancelled or done tasks deleted on {} (restore one with `git push {} <commit>:refs/heads/<branch>`):\n",
            deleted.len(),
            remote.name,
            remote.name
        ));
        for line in &deleted {
            body.push_str(&format!("- {line}\n"));
        }
    }
    let urgent = !old.is_empty() || !crowded.is_empty();
    store.write(|tx| {
        let Some(captain) = tx.captain_of(project)? else {
            return Ok(());
        };
        let message = tx.insert_message(&NewMessage {
            meta: None,
            project: project.to_string(),
            thread: format!("branches:{now}"),
            task: None,
            attempt: None,
            sender: captain.id.clone(),
            sender_gen: 1,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent,
            body: body.trim_end().to_string(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        // T-568: this sweep's report replaces the ones before it (its thread is
        // named after the tick, so they are found by their prefix): the
        // captain's inbox holds one branch-sweep item, the newest, whose body
        // already lists what is still true.
        for item in tx.messages_in_threads_starting(project, "branches:")? {
            if item.id == message.id {
                continue;
            }
            if !matches!(item.state, MsgState::Open | MsgState::Waiting) {
                continue;
            }
            tx.update_message_state(
                project,
                &item.id,
                MsgState::Resolved,
                Some(&format!("superseded by {}", message.id)),
                None,
                None,
                now,
            )?;
        }
        Ok(())
    })
}

/// The thread of the item that says the merge of a task waits for another one
/// (T-62). One item per waiting merge: the captain is told once, not every
/// tick.
fn wait_thread(task: &str) -> String {
    format!("merge-wait:{task}")
}

/// The key that remembers that the captain was told that the merge of a
/// task's attempt waits. The notice is told once per attempt (T-220): a
/// resolved item does not bring the same news back.
fn merge_wait_told_key(project: &str, task: &str, attempt: i64) -> String {
    format!("engine.merge_wait_told.{project}.{task}.{attempt}")
}

/// Tell the captain that a merge waits for another task (T-62): one open item
/// per merge, naming the tasks it waits for. The item closes when the merge
/// goes through, or when the tasks it waits for have passed and only their
/// own merge is left.
fn tell_captain_waiting(
    store: &mut Store,
    project: &str,
    op: &OpRow,
    blocked: &[(String, bool)],
    now: Ts,
) -> GearResult<()> {
    let task = op.task.clone().unwrap_or_default();
    let names: Vec<String> = blocked.iter().map(|(task, _)| task.clone()).collect();
    let unpassed: Vec<String> = blocked
        .iter()
        .filter(|(_, passed)| !passed)
        .map(|(task, _)| task.clone())
        .collect();
    let body = format!(
        "the merge of {task} waits for {}: their candidate is in {task}'s commits \
         and is not on the integration branch yet",
        names.join(", ")
    );
    // The line names the tasks that have not passed: that is the captain's
    // business. The others only wait for their own merge.
    let body = if unpassed.is_empty() {
        body
    } else {
        format!(
            "{body}\n{}: {}",
            unpassed.join(", "),
            if unpassed.len() == 1 {
                "has not passed its review yet"
            } else {
                "have not passed their review yet"
            }
        )
    };
    store.write(|tx| {
        let captain = tx.captain_of(project)?.ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!("project {project} has no captain"),
            )
        })?;
        let thread = wait_thread(&task);
        // One open item per merge: the captain is told once, not every tick.
        let open = tx
            .thread_messages(project, &thread)?
            .into_iter()
            .find(|message| message.state == MsgState::Open);
        if let Some(item) = open {
            if item.body != body {
                tx.update_message_body(project, &item.id, &body)?;
            }
            return Ok(());
        }
        // Every task it waits for has passed: the wait is only about their own
        // merge, which is a moment. The captain has nothing to do.
        if unpassed.is_empty() {
            return Ok(());
        }
        // And once per attempt: the captain was told about this wait already
        // (T-220). A resolved item does not bring the same news back on the
        // next tick.
        let told_key = merge_wait_told_key(project, &task, op.attempt.unwrap_or(0));
        if tx.get_meta(&told_key)?.is_some() {
            return Ok(());
        }
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.to_string(),
            thread,
            task: Some(task.clone()),
            attempt: op.attempt,
            sender: captain.id.clone(),
            sender_gen: 1,
            recipient: captain.id,
            kind: MsgKind::Question,
            urgent: false,
            body,
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        // The notice is told once per attempt: remember it, so a later tick
        // does not add it again after the captain resolved the item.
        tx.set_meta(&told_key, &now.to_string())?;
        Ok(())
    })
}

/// The thread of the item that says an accepted attempt of a task is merged by
/// nothing (T-397). One item per task: the word is given once per attempt.
fn lost_thread(task: &str) -> String {
    format!("merge-lost:{task}")
}

/// The key that remembers that the captain was told that an accepted attempt
/// of a task is merged by nothing (T-397): the word is given once per attempt,
/// so a resolved item does not bring the same news back on the next tick.
fn merge_lost_told_key(project: &str, task: &str, attempt: i64) -> String {
    format!("engine.merge_lost_told.{project}.{task}.{attempt}")
}

/// The key that remembers that a candidate is already on the integration branch
/// (T-397). A commit stays an ancestor once it is one, so the answer is asked of
/// git once per attempt instead of on every tick.
fn merge_lost_landed_key(project: &str, task: &str, attempt: i64) -> String {
    format!("engine.merge_lost_landed.{project}.{task}.{attempt}")
}

/// The word that closes a lost-merge item whose task the scan no longer
/// checks (T-408).
const LOST_SKIPPED: &str = "no longer checked: its voyage is finished or its candidate is gone";
/// The word an open merge-lost item closes with once the work landed.
const LOST_LANDED: &str = "its work landed";
/// The word an open merge-lost item closes with once a merge of it is queued.
const LOST_QUEUED: &str = "a merge of it is queued again";

/// The accepted attempt the lost-merge scan looks at (T-397, T-408): what it
/// needs to name the work, and why it may leave it alone.
struct Lost {
    task: String,
    attempt: i64,
    candidate: String,
    repo: Repo,
    /// The word about this attempt was given already (T-397): the item is not
    /// opened again, and only the T-408 skips can still close it.
    told: bool,
    /// The task's voyage still has a task that is not terminal (T-408).
    voyage: bool,
    /// An item about this attempt is still open (T-458): the skip rule is
    /// checked for it before the old filters below.
    open_item: bool,
    /// The old filters of T-397 that keep a task out of a **new** item: its
    /// merge landed, a merge op is queued or running, or its merge-failed item
    /// is open.
    settled: bool,
    /// The attempt carries a merge time: its work landed, by its own merge or
    /// with another task's (`integrated-by`).
    merged: bool,
    /// A merge op of the task is queued, running or being recovered.
    in_flight: bool,
}

/// Is the task's voyage open (T-408)? A voyage is its milestone, and it is open
/// while one of its tasks is not terminal. A task of no milestone has no
/// voyage to be finished.
fn voyage_open(open: &std::collections::HashSet<String>, task: &gear_store::TaskRow) -> bool {
    task.milestone
        .as_deref()
        .is_none_or(|milestone| open.contains(milestone))
}

/// Does the task's repository still hold the candidate (T-408)?
///
/// `git cat-file -e <sha>^{commit}` answers: `Some(false)` — the repository no
/// longer has the commit, and the item would name work that nobody can remerge.
/// `None` — there is no repository to ask, and the scan does not skip work on a
/// question git could not put (a repo path that is not there is the project's
/// trouble, not evidence that the commit is gone).
fn candidate_held(repo: &Repo, candidate: &str) -> Option<bool> {
    let dir = Path::new(&repo.path);
    if !dir.is_dir() {
        return None;
    }
    Some(git(dir, &["cat-file", "-e", &format!("{candidate}^{{commit}}")]).is_ok())
}

/// Mark merged the tasks carried by another (`carried-by: T-m`) once T-m's
/// merge has landed and the carried candidate is on the integration branch:
/// the attempt takes T-m's merge time, as an `integrated-by` pass would give
/// it. A carried candidate that is not on the branch stays unmerged, and the
/// lost scan tells the captain.
fn land_carried(store: &mut Store, project: &str, config: &ProjectConfig) -> GearResult<bool> {
    let mut marked = false;
    let pending = store.read(|tx| {
        let mut pending = Vec::new();
        for task in tx.tasks_of_project(project)? {
            if task.state != TaskState::Done {
                continue;
            }
            let Some(by) = tx.task_carried_by(project, &task.id)? else {
                continue;
            };
            let (carrier_project, carrier) = gear_store::Tx::carrier_of(project, &by);
            let Some(merged_at) = gear_app::tasks::merged_at(tx, carrier_project, carrier)? else {
                continue;
            };
            let Some(latest) = tx.attempts_of_task(project, &task.id)?.into_iter().last() else {
                continue;
            };
            if latest.state != gear_model::AttemptState::Accepted
                || tx
                    .attempt_times(project, &task.id, latest.n)?
                    .merged
                    .is_some()
            {
                continue;
            }
            let Some(candidate) = latest.candidate.clone() else {
                continue;
            };
            pending.push((
                task.id.clone(),
                task.lane.clone(),
                latest.n,
                candidate,
                by,
                merged_at,
            ));
        }
        Ok(pending)
    })?;
    for (task, lane, attempt, candidate, by, merged_at) in pending {
        let repo = match lane.as_deref() {
            Some(lane) => config.repo_of_lane(lane),
            None => config.root_repo(),
        };
        let dir = Path::new(&repo.path);
        let reference = format!("{}/{}", config.conventions.remote, repo.integration);
        let _ = git(
            dir,
            &[
                "fetch",
                "--quiet",
                &config.conventions.remote,
                &repo.integration,
            ],
        );
        if !is_ancestor(dir, &candidate, &reference) {
            continue;
        }
        store.write(|tx| {
            gear_app::tasks::mark_integrated_by(tx, project, &task, attempt, &by, merged_at)
        })?;
        marked = true;
    }
    Ok(marked)
}

/// Close the merge-failed items of work that landed after all: the attempt
/// the item names has a merge time (its own later merge, or another task's
/// merge that carried it, `integrated-by`). Such an item asks for a remerge of
/// work that is already on the integration branch, and a reply cannot close it
/// (Lawrence M-182, 2026-11-03: T-14 landed with T-26).
fn close_landed_failures(store: &mut Store, project: &str, now: Ts) -> GearResult<()> {
    store.write(|tx| {
        for task in tx.merged_done_tasks(project)? {
            let thread = failed_thread(&task);
            let open: Vec<_> = tx
                .thread_messages(project, &thread)?
                .into_iter()
                .filter(|item| matches!(item.state, MsgState::Open | MsgState::Waiting))
                .collect();
            if open.is_empty() {
                continue;
            }
            let merged: Vec<i64> = tx
                .attempt_times_of_task(project, &task)?
                .into_iter()
                .filter(|(_, times)| times.merged.is_some())
                .map(|(n, _)| n)
                .collect();
            for item in open {
                let landed = match item.attempt {
                    Some(attempt) => merged.contains(&attempt),
                    None => !merged.is_empty(),
                };
                if landed {
                    tx.update_message_state(
                        project,
                        &item.id,
                        MsgState::Resolved,
                        Some(LOST_LANDED),
                        None,
                        None,
                        now,
                    )?;
                }
            }
        }
        Ok(())
    })
}

/// Close the captain's items about a task that is over (T-569): a note on a
/// settled task rings for hours otherwise (M-10318, a lane's note on T-715,
/// still ringing after T-715 was done; five such items open, and seven
/// merge-failed/merge-lost items naming tasks whose work had merged).
///
/// Two kinds close here, in the same periodic scan as [`close_landed_failures`]:
/// - every open item on a task thread — `T-<n>:<attempt>` and the captain's
///   own `T-<n>:cap` — while the task is done or cancelled, with `the task is
///   <state>`;
/// - every open `merge-failed:<task>` and `merge-lost:<task>` item of a task
///   whose attempt has a merge time, with `its work landed`.
///
/// An item of a task still running stays open: it is the captain's to act on.
fn close_settled_task_items(store: &mut Store, project: &str, now: Ts) -> GearResult<()> {
    let unrest = |state: MsgState| matches!(state, MsgState::Open | MsgState::Waiting);
    store.write(|tx| {
        for task in tx.tasks_of_project(project)? {
            if matches!(task.state, TaskState::Done | TaskState::Cancelled) {
                let why = format!("the task is {}", task.state.as_str());
                let mut threads: Vec<String> = tx
                    .attempts_of_task(project, &task.id)?
                    .into_iter()
                    .map(|attempt| format!("{}:{}", task.id, attempt.n))
                    .collect();
                // The captain's own decision item hangs on `T-<n>:cap`, and an
                // overrun of a finished task says nothing any more
                // (Prometheus M-8886, overrun:T-609:rework).
                threads.push(format!("{}:cap", task.id));
                for step in ["dev", "review", "rework", "direction"] {
                    threads.push(format!("overrun:{}:{step}", task.id));
                }
                for thread in threads {
                    for item in tx.thread_messages(project, &thread)? {
                        if !unrest(item.state) {
                            continue;
                        }
                        tx.update_message_state(
                            project,
                            &item.id,
                            MsgState::Resolved,
                            Some(&why),
                            None,
                            None,
                            now,
                        )?;
                    }
                }
            }
            // The work landed: an item that says the merge failed or was lost
            // asks for a remerge of work that is already on the branch.
            // Its own merge or its carrier's (Prometheus T-588, T-563).
            let landed = gear_app::tasks::work_landed(tx, project, &task.id)?;
            if !landed {
                continue;
            }
            for thread in [failed_thread(&task.id), lost_thread(&task.id)] {
                for item in tx.thread_messages(project, &thread)? {
                    if !unrest(item.state) {
                        continue;
                    }
                    tx.update_message_state(
                        project,
                        &item.id,
                        MsgState::Resolved,
                        Some(LOST_LANDED),
                        None,
                        None,
                        now,
                    )?;
                }
            }
        }
        Ok(())
    })
}

/// Close the lost-merge item of a task the scan no longer checks (T-408): the
/// captain can no longer act on it.
fn resolve_lost(store: &mut Store, project: &str, task: &str, now: Ts) -> GearResult<()> {
    resolve_lost_with(store, project, task, LOST_SKIPPED, now)
}

/// Close the open merge-lost items of a task with `why`.
fn resolve_lost_with(
    store: &mut Store,
    project: &str,
    task: &str,
    why: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        let thread = lost_thread(task);
        for item in tx.thread_messages(project, &thread)? {
            if item.state == MsgState::Open {
                tx.update_message_state(
                    project,
                    &item.id,
                    MsgState::Resolved,
                    Some(why),
                    None,
                    None,
                    now,
                )?;
            }
        }
        Ok(())
    })
}

/// Name the accepted attempts that nothing will merge (T-397): a Done task
/// whose latest accepted attempt has a candidate and no merge time, with no
/// merge op queued or running and no open `merge-failed` item — the work sat
/// accepted and unmerged with no word about it.
///
/// One item per attempt, and a candidate that is already an ancestor of the
/// integration branch is left alone: the engine names only what is lost.
///
/// A task is looked at only while its voyage is open, and only while its repo
/// still holds the candidate (T-408): a finished voyage settled its lost merges
/// with it, and a candidate that is no longer an object in the repository
/// cannot be remerged. An open item about such a task closes with that word.
fn tell_captain_lost(
    store: &mut Store,
    project: &str,
    config: &ProjectConfig,
    now: Ts,
) -> GearResult<()> {
    // One scan of the ops and one of the tasks per tick, like the merge keys of
    // the queue step: a project with no lost task pays two reads.
    let (lost, carried_active) = store.read(|tx| {
        let mut carried_active: Vec<(String, String)> = Vec::new();
        let mut in_flight = std::collections::HashSet::new();
        for state in [
            OpState::Intended,
            OpState::Executing,
            OpState::Submitted,
            OpState::Unknown,
        ] {
            for op in tx.ops_in_state(project, state)? {
                if op.kind == OpKind::Merge {
                    if let Some(task) = op.task {
                        in_flight.insert(task);
                    }
                }
            }
        }
        let tasks = tx.tasks_of_project(project)?;
        // T-408: a voyage is its milestone, and it is open while one of its
        // tasks is not terminal, or one settled less than LAND_WITHIN_S ago:
        // the last task of a voyage is the one most likely to sit unmerged,
        // and it is the one that holds the final candidate (Prometheus T-779).
        let mut open_voyages = std::collections::HashSet::new();
        for task in &tasks {
            if let Some(milestone) = task.milestone.as_deref() {
                if !task.state.is_terminal() || now - task.updated < LAND_WITHIN_S {
                    open_voyages.insert(milestone.to_string());
                }
            }
        }
        let mut lost: Vec<Lost> = Vec::new();
        for task in &tasks {
            // Only a done task is settled: the others may still be handed in.
            if task.state != TaskState::Done {
                continue;
            }
            let Some(latest) = tx.attempts_of_task(project, &task.id)?.into_iter().last() else {
                continue;
            };
            if latest.state != gear_model::AttemptState::Accepted {
                continue;
            }
            let Some(candidate) = latest.candidate.clone() else {
                continue;
            };
            // A carried task waits for its carrier's merge only while that
            // merge is queued or running. A carrier whose merge failed, or
            // that has none in flight, lands nothing: the carried work is
            // lost work (user, 2026-11-04: 41 carried tasks sat unlanded for
            // hours, unreported).
            // A carrier that is still being worked on (ready, running,
            // verifying, or its merge in flight) is waited for too, but only
            // for LAND_WITHIN_S after the acceptance: accepted work that has
            // not landed by then is lost whatever carries it.
            if let Some(by) = tx.task_carried_by(project, &task.id)? {
                let (carrier_project, carrier) = gear_store::Tx::carrier_of(project, &by);
                let accepted_at = tx
                    .attempt_times(project, &task.id, latest.n)?
                    .verdict
                    .unwrap_or(now);
                let carrier_active = in_flight.contains(carrier)
                    || tx.get_task(carrier_project, carrier)?.is_some_and(|row| {
                        matches!(
                            row.state,
                            TaskState::Ready
                                | TaskState::Running
                                | TaskState::Verifying
                                | TaskState::Waiting
                        )
                    });
                if gear_app::tasks::merged_at(tx, carrier_project, carrier)?.is_none()
                    && carrier_active
                    && now - accepted_at < LAND_WITHIN_S
                {
                    carried_active.push((task.id.clone(), by.clone()));
                    continue;
                }
            }
            // The old filters of T-397 say only whether a **new** item is
            // opened. They are kept here rather than skipping the task
            // (T-458): a task they drop may still hold an open merge-lost item
            // that the skip rule has to close (T-408 review F1).
            // The attempt carries a merge time: its work landed (design §10.7,
            // row B03-01).
            let merged = tx
                .attempt_times(project, &task.id, latest.n)?
                .merged
                .is_some();
            // A merge op will merge it: queued, running or being recovered.
            let in_flight = in_flight.contains(&task.id);
            // Told and settled already: a resolved item does not come back.
            // Such a task is still carried, so the T-408 skips can close an
            // item that is still open.
            let told = tx
                .get_meta(&merge_lost_told_key(project, &task.id, latest.n))?
                .is_some()
                || tx
                    .get_meta(&merge_lost_landed_key(project, &task.id, latest.n))?
                    .is_some();
            // An item about this attempt is still open (T-458): the skip rule
            // is checked for it before the old filters. Only a told attempt
            // can have one — the item is opened with that word.
            let open_item = told
                && tx
                    .thread_messages(project, &lost_thread(&task.id))?
                    .into_iter()
                    .any(|message| message.state == MsgState::Open);
            // F1 of the T-397 review: the task's own snapshot says where its
            // work lands — a task keeps the repo and the base it was created
            // against even when its lane moves (design, `repo_of_task`), and
            // merge execution asks the same question (see `repo_of_op`).
            let repo = config.repo_of_task(task);
            lost.push(Lost {
                task: task.id.clone(),
                attempt: latest.n,
                candidate,
                repo,
                told,
                voyage: voyage_open(&open_voyages, task),
                open_item,
                // An open merge-failed item is not a merge: the work is
                // still lost until it lands (user, 2026-11-04).
                settled: merged || in_flight,
                merged,
                in_flight,
            });
        }
        Ok((lost, carried_active))
    })?;
    // A lost item about a task whose carrier is active again is answered.
    for (task, by) in carried_active {
        let open = store.read(|tx| {
            Ok(tx
                .thread_messages(project, &lost_thread(&task))?
                .into_iter()
                .any(|message| message.state == MsgState::Open))
        })?;
        if open {
            resolve_lost_with(
                store,
                project,
                &task,
                &format!("carried by {by}, which is being worked on"),
                now,
            )?;
        }
    }
    for Lost {
        task,
        attempt,
        candidate,
        repo,
        told,
        voyage,
        open_item,
        settled,
        merged,
        in_flight,
    } in lost
    {
        // The work landed (its own merge, or another task's that carried it):
        // an item still open about it is answered.
        if open_item && merged {
            resolve_lost_with(store, project, &task, LOST_LANDED, now)?;
            continue;
        }
        // T-408: a lost merge of a finished voyage was settled with the voyage,
        // and a candidate the repository no longer holds is work that cannot be
        // remerged. Either way the task is not checked, and an item still open
        // about it is closed with that word. T-458: this comes before the old
        // filters, so an item stays closable while a merge is in flight or a
        // merge-failed item is open.
        let checked = open_item || (!told && !settled);
        if checked {
            let skip = !voyage || matches!(candidate_held(&repo, &candidate), Some(false));
            if skip {
                if open_item {
                    resolve_lost(store, project, &task, now)?;
                }
                continue;
            }
        }
        // A merge of it is queued again (a red batch sent it back, or the
        // captain's remerge): the item is answered, the merge will land it
        // (Prometheus T-721/T-723, 2026-11-05).
        if open_item && in_flight {
            resolve_lost_with(store, project, &task, LOST_QUEUED, now)?;
            continue;
        }
        if told {
            continue;
        }
        // The old filters of T-397 keep a settled task out of a new item.
        if settled {
            continue;
        }
        // The candidate's own repo answers, from its remote-tracking ref: no
        // fetch on the tick's path. A ref that lags can only answer "not
        // merged", and the remerge the item names settles that (it fetches).
        let reference = format!(
            "refs/remotes/{}/{}",
            config.conventions.remote, repo.integration
        );
        if is_ancestor(Path::new(&repo.path), &candidate, &reference) {
            store.write(|tx| {
                tx.set_meta(
                    &merge_lost_landed_key(project, &task, attempt),
                    &now.to_string(),
                )
            })?;
            continue;
        }
        let body = format!(
            "{task} attempt {attempt} was accepted but is not merged and nothing will merge it: \
             `- remerge {task}` under `# Staff` retries it"
        );
        store.write(|tx| {
            let captain = tx.captain_of(project)?.ok_or_else(|| {
                GearError::new(
                    ErrorCode::NoTarget,
                    format!("project {project} has no captain"),
                )
            })?;
            tx.insert_message(&NewMessage {
                meta: None,
                project: project.to_string(),
                thread: lost_thread(&task),
                task: Some(task.clone()),
                attempt: Some(attempt),
                sender: captain.id.clone(),
                sender_gen: 1,
                recipient: captain.id,
                kind: MsgKind::Question,
                // Accepted work that will never land is urgent: it rings at once.
                urgent: true,
                body,
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: now,
            })?;
            tx.set_meta(
                &merge_lost_told_key(project, &task, attempt),
                &now.to_string(),
            )?;
            Ok(())
        })?;
    }
    Ok(())
}

/// Close the item that says the merge of a task waits (T-62): the wait is
/// over.
fn resolve_wait(store: &mut Store, project: &str, task: &str, now: Ts) -> GearResult<()> {
    if task.is_empty() {
        return Ok(());
    }
    store.write(|tx| {
        let thread = wait_thread(task);
        for item in tx.thread_messages(project, &thread)? {
            if item.state == MsgState::Open {
                tx.update_message_state(
                    project,
                    &item.id,
                    MsgState::Resolved,
                    Some("the merge went through"),
                    None,
                    None,
                    now,
                )?;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// The engine's config with the tests' remote runner (T-107): a merge
    /// moves refs from origin's own side, so no engine test runs `git push`.
    fn test_cfg() -> EngineCfg {
        EngineCfg {
            remote: std::sync::Arc::new(FetchRemote),
            // A merge that retries does not wait on the clock in a test
            // (T-127).
            merge_retry_wait: RetryWait::immediate(),
            ..Default::default()
        }
    }

    /// Move the worktree's HEAD onto `branch` in its origin, from the origin's
    /// own side (T-107): the engine's tests never run `git push`.
    fn publish(work: &Path, branch: &str) {
        sh(
            work,
            &format!(
                "origin=$(git remote get-url origin); \
                 git -C \"$origin\" fetch -q \"$(pwd)\" +HEAD:refs/heads/{branch} && \
                 git fetch -q origin"
            ),
        );
    }

    /// The shape of a fixture repo: its branches and the commits they hold.
    /// Every shape is built once per test process and copied per test
    /// (conventions "Test time": a git fixture is built once, and a copy
    /// costs a copy instead of twenty git calls).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Shape {
        /// `dev`, then `heat/t5` (appends to README) and `review/t5`.
        Task,
        /// Two tasks where `heat/t2` is cut from `heat/t1`: T-2's candidate
        /// carries T-1's work.
        PairOn,
        /// Two tasks both cut from `dev`: neither carries the other.
        PairFlat,
        /// Three tasks both cut from `dev`: the merge order of T-574.
        TrioFlat,
        /// Two tasks in a chain: T-2 carries T-1.
        Chain2,
        /// Three tasks in a chain.
        Chain3,
        /// A second repo of a project (T-53), named `web`, on `web-dev`.
        ChildWeb,
    }

    impl Shape {
        /// A branch of the shape: the task, the branch it is cut from, the
        /// file its work commit adds, that commit's subject, and the subject of
        /// its review commit (which adds `<review>.txt`).
        fn branches(self) -> Vec<Branch> {
            let chain = |n: &'static str, base: &'static str| Branch {
                task: n,
                base,
                file: match n {
                    "t1" => "t1.txt",
                    "t2" => "t2.txt",
                    _ => "t3.txt",
                },
                subject: n,
                review: match n {
                    "t1" => "r1",
                    "t2" => "r2",
                    _ => "r3",
                },
            };
            match self {
                // The one task of a single-task repo: it appends to the
                // README, so two such branches conflict (the merge's conflict
                // fixture needs that).
                Shape::Task => vec![Branch {
                    task: "t5",
                    base: "dev",
                    file: "README.md",
                    subject: "task",
                    review: "review",
                }],
                Shape::PairOn => vec![
                    Branch {
                        task: "t1",
                        base: "dev",
                        file: "a.txt",
                        subject: "a",
                        review: "r1",
                    },
                    Branch {
                        task: "t2",
                        base: "heat/t1",
                        file: "b.txt",
                        subject: "b",
                        review: "r2",
                    },
                ],
                Shape::PairFlat => vec![
                    Branch {
                        task: "t1",
                        base: "dev",
                        file: "a.txt",
                        subject: "a",
                        review: "r1",
                    },
                    Branch {
                        task: "t2",
                        base: "dev",
                        file: "b.txt",
                        subject: "b",
                        review: "r2",
                    },
                ],
                Shape::TrioFlat => ["t1", "t2", "t3"]
                    .into_iter()
                    .zip(["a.txt", "b.txt", "c.txt"])
                    .zip(["a", "b", "c"])
                    .zip(["r1", "r2", "r3"])
                    .map(|(((task, file), subject), review)| Branch {
                        task,
                        base: "dev",
                        file,
                        subject,
                        review,
                    })
                    .collect(),
                Shape::Chain2 => vec![chain("t1", "dev"), chain("t2", "heat/t1")],
                Shape::Chain3 => vec![
                    chain("t1", "dev"),
                    chain("t2", "heat/t1"),
                    chain("t3", "heat/t2"),
                ],
                Shape::ChildWeb => vec![Branch {
                    task: "t5",
                    base: "web-dev",
                    file: "README.md",
                    subject: "task",
                    review: "review",
                }],
            }
        }
    }

    /// One task branch of a fixture: see [`Shape::branches`].
    struct Branch {
        task: &'static str,
        base: &'static str,
        file: &'static str,
        subject: &'static str,
        review: &'static str,
    }

    /// The one template of a shape, built the first time a test asks for it.
    fn template(shape: Shape) -> &'static Path {
        static TASK: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        static PAIR_ON: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        static PAIR_FLAT: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        static TRIO_FLAT: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        static CHAIN2: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        static CHAIN3: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        static CHILD: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        let slot = match shape {
            Shape::Task => &TASK,
            Shape::PairOn => &PAIR_ON,
            Shape::PairFlat => &PAIR_FLAT,
            Shape::TrioFlat => &TRIO_FLAT,
            Shape::Chain2 => &CHAIN2,
            Shape::Chain3 => &CHAIN3,
            Shape::ChildWeb => &CHILD,
        };
        slot.get_or_init(|| build_template(shape)).path()
    }

    /// Build a template repo in one shell: a bare origin and a clone with the
    /// shape's branches, every branch published to origin. A child repo's
    /// origin and work are named after it, so a copy keeps its relative remote.
    fn build_template(shape: Shape) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (name, integration) = match shape {
            Shape::ChildWeb => ("web", "web-dev"),
            _ => ("", "dev"),
        };
        let origin = if name.is_empty() {
            "origin.git".to_string()
        } else {
            format!("{name}-origin.git")
        };
        let work = if name.is_empty() {
            "work".to_string()
        } else {
            name.to_string()
        };

        // Three processes for a whole repo, whatever the shape (T-127): the
        // bare origin, one `fast-import` that writes every commit and branch
        // from the shape's data, and one shell that clones the checkout from
        // it. The checkout keeps the relative remote (`../<origin>.git`) the
        // fixtures and the child repo's copy rely on.
        sh(root, &format!("git init -q --bare '{origin}'"));
        fast_import(root, &origin, &shape_stream(shape, integration));
        sh(
            root,
            &format!(
                "set -e\n\
                 git init -q -b {integration} '{work}'\n\
                 git -C '{work}' remote add origin ../{origin}\n\
                 git -C '{work}' fetch -q --update-head-ok origin \
                   '+refs/heads/*:refs/heads/*' '+refs/heads/*:refs/remotes/origin/*'\n\
                 git -C '{work}' reset -q --hard {integration}\n"
            ),
        );
        dir
    }

    /// The fast-import stream of a shape: `dev`, then every task branch cut
    /// from its base, then the review branch cut from it. A branch on the
    /// README appends (the conflict fixture needs two branches that touch the
    /// same line); every other branch writes its own file.
    fn shape_stream(shape: Shape, integration: &str) -> String {
        let mut stream = String::new();
        commit(
            &mut stream,
            1,
            integration,
            None,
            "dev",
            &[("README.md", "dev\n".to_string())],
        );
        let mut marks = std::collections::BTreeMap::new();
        marks.insert(integration.to_string(), 1u32);
        let mut mark = 2u32;
        for branch in shape.branches() {
            let Branch {
                task,
                base,
                file,
                subject,
                review,
            } = branch;
            let base_mark = *marks.get(base).expect("a branch's base comes before it");
            let content = if file == "README.md" {
                format!("dev\n{subject}\n")
            } else {
                format!("{subject}\n")
            };
            let heat = format!("heat/{task}");
            commit(
                &mut stream,
                mark,
                &heat,
                Some(base_mark),
                subject,
                &[(file, content)],
            );
            marks.insert(heat, mark);
            mark += 1;
            let review_file = format!("{review}.txt");
            commit(
                &mut stream,
                mark,
                &format!("review/{task}"),
                Some(mark - 1),
                review,
                &[(&review_file, format!("{review}\n"))],
            );
            mark += 1;
        }
        stream
    }

    /// One commit of a fast-import stream: `subject` as its message, each
    /// `(path, content)` written into it.
    fn commit(
        stream: &mut String,
        mark: u32,
        branch: &str,
        base: Option<u32>,
        subject: &str,
        files: &[(&str, String)],
    ) {
        use std::fmt::Write;
        let _ = write!(
            stream,
            "commit refs/heads/{branch}\nmark :{mark}\n\
             committer GEAR <gear@test> 0 +0000\ndata {}\n{subject}\n",
            subject.len()
        );
        if let Some(base) = base {
            let _ = writeln!(stream, "from :{base}");
        }
        for (path, content) in files {
            let _ = write!(
                stream,
                "M 100644 inline {path}\ndata {}\n{content}\n",
                content.len()
            );
        }
        stream.push('\n');
    }

    /// Write `stream` into the bare repo `origin` (a path below `root`) with
    /// one `fast-import` process.
    fn fast_import(root: &Path, origin: &str, stream: &str) {
        use std::io::Write;
        let mut child = Command::new("git")
            .arg("-C")
            .arg(origin)
            .args(["fast-import", "--quiet"])
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(stream.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "fast-import into {origin}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The scratch directory of one test: its own copy of the shape's
    /// template, on a path no other test uses.
    fn scratch_root(name: &str, shape: Shape) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "gear-merge-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        copy_tree(template(shape), &root);
        root
    }

    /// Copy the template into `to`, tree and all, in this process (T-274): a
    /// copied fixture costs a copy, not a shell and a `cp` on the test's
    /// budget. File modes come along, which is what a git repo needs.
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    /// A scratch repo with a bare origin: `dev` plus one commit each on
    /// `heat/t5` and `review/t5`.
    struct Scratch {
        root: PathBuf,
        work: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let root = scratch_root(name, Shape::Task);
            let work = root.join("work");
            Scratch { root, work }
        }

        /// A worktree of this repo for the merge to run in (on `p/merge/between-voyages`).
        fn merge_worktree(&self) -> gear_herdr::Worktree {
            let path = self.root.join("merge");
            sh(
                &self.root,
                &format!(
                    "git -C {} worktree add -q -b p/merge/between-voyages {} origin/dev",
                    self.work.display(),
                    path.display()
                ),
            );
            gear_herdr::Worktree {
                workspace_id: "wt-merge".into(),
                pane_id: "wt-merge:p1".into(),
                path: path.display().to_string(),
                branch: "p/merge/between-voyages".into(),
            }
        }

        /// The merge worktree and the head of the template's task branch in
        /// one shell: the two the tests ask for together, for one shell
        /// instead of two (T-274).
        fn prepared(&self) -> (gear_herdr::Worktree, String) {
            let path = self.root.join("merge");
            let heads = sh(
                &self.root,
                &format!(
                    "git -C {} worktree add -q -b p/merge/between-voyages {} origin/dev && \
                     git -C origin.git rev-parse heat/t5",
                    self.work.display(),
                    path.display()
                ),
            );
            let head = heads.lines().last().unwrap_or_default().trim().to_string();
            let worktree = gear_herdr::Worktree {
                workspace_id: "wt-merge".into(),
                pane_id: "wt-merge:p1".into(),
                path: path.display().to_string(),
                branch: "p/merge/between-voyages".into(),
            };
            (worktree, head)
        }

        /// The head commit of a branch on origin.
        fn head_of(&self, branch: &str) -> String {
            sh(&self.root, &format!("git -C origin.git rev-parse {branch}"))
        }

        /// What origin holds after a merge: its integration branch and its
        /// branches, from one shell (T-274: the assertions of a test asked
        /// for both, and paid a process for each).
        fn state(&self) -> State {
            let out = sh(
                &self.root,
                "git -C origin.git log --format=%s dev; echo ---; \
                 git -C origin.git for-each-ref --format='%(refname:short)' refs/heads",
            );
            let (dev, branches) = out.split_once("---").unwrap_or((&out, ""));
            State {
                dev: dev.trim().to_string(),
                branches: branches.trim().to_string(),
            }
        }

        /// What origin's integration branch holds.
        fn dev_log(&self) -> String {
            sh(&self.root, "git -C origin.git log --format=%s dev")
        }

        /// The branches origin still has.
        fn origin_branches(&self) -> String {
            sh(
                &self.root,
                "git -C origin.git for-each-ref --format='%(refname:short)' refs/heads",
            )
        }

        /// `true` when the task branch's commit is on origin's dev.
        fn dev_has_task_commit(&self) -> bool {
            sh_ok(
                &self.root,
                "git -C origin.git log --format=%s dev | grep -qx task",
            )
        }

        /// The head of a branch in the working clone (a branch that was never
        /// pushed has no head on origin).
        fn local_head_of(&self, branch: &str) -> String {
            sh(&self.work, &format!("git rev-parse {branch}"))
        }
    }

    /// What origin of a scratch repo holds: see [`Scratch::state`].
    struct State {
        /// The subjects on the integration branch, one per line.
        dev: String,
        /// The branch names origin still has, one per line.
        branches: String,
    }

    impl State {
        /// `true` when the task branch's commit is on the integration branch.
        fn has_task_commit(&self) -> bool {
            self.dev.lines().any(|line| line == "task")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// Two tasks in one scratch repo (T-62): `heat/t1` (the work of T-1) and
    /// `heat/t2`, cut from `heat/t1` when `built_on` is true — so T-2's
    /// candidate carries T-1's work — or from `dev` when it is false.
    struct Pair {
        root: PathBuf,
        work: PathBuf,
        a: String,
        b: String,
    }

    impl Pair {
        fn new(name: &str, built_on: bool) -> Pair {
            let shape = if built_on {
                Shape::PairOn
            } else {
                Shape::PairFlat
            };
            let root = scratch_root(name, shape);
            let work = root.join("work");
            // One listing for both heads, not one git process each (T-127).
            let heads = branch_heads(&root);
            let a = heads["heat/t1"].clone();
            let b = heads["heat/t2"].clone();
            Pair { root, work, a, b }
        }

        /// The merge worktree of this repo.
        fn merge_worktree(&self) -> gear_herdr::Worktree {
            let path = self.root.join("merge");
            sh(
                &self.root,
                &format!(
                    "git -C {} worktree add -q -b p/merge/between-voyages {} origin/dev",
                    self.work.display(),
                    path.display()
                ),
            );
            gear_herdr::Worktree {
                workspace_id: "wt-merge".into(),
                pane_id: "wt-merge:p1".into(),
                path: path.display().to_string(),
                branch: "p/merge/between-voyages".into(),
            }
        }

        /// What origin's dev holds.
        fn dev_log(&self) -> String {
            sh(&self.root, "git -C origin.git log --format=%s dev")
        }
    }

    impl Drop for Pair {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// A project with two tasks, T-1 and T-2, both handed in: their
    /// candidates are the two commits of the scratch repo.
    fn pair_world(candidate_a: &str, candidate_b: &str) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                // T-72: the fixture's own origin, as `init` would freeze it.
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
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "heat",
                    gear_model::SeatRole::Lane,
                    "w1:p1",
                    "opencode",
                    0,
                )?;
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    gear_model::SeatRole::Captain,
                    "w9:p1",
                    "claude",
                    0,
                )?;
                for (id, rank, candidate) in [("T-1", 1, candidate_a), ("T-2", 2, candidate_b)] {
                    tx.insert_task(&gear_store::TaskRow {
                        repo: None,
                        base: None,
                        gate: None,
                        rework_estimate_min: None,
                        project: "p".into(),
                        id: id.into(),
                        rank,
                        state: TaskState::Ready,
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
                        issue: None,
                        updated: 0,
                    })?;
                    gear_app::tasks::start_attempt(tx, "p", id, "p:heat", 1, 0)?;
                    gear_app::tasks::on_delivery_confirmed(tx, "p", id, 1, 0)?;
                    gear_app::tasks::submit_result(tx, "p", id, 1, 1, candidate, "report", 0)?;
                }
                Ok(())
            })
            .unwrap();
        store
    }

    /// A passing verdict for a task's latest attempt.
    fn pass(store: &mut Store, task: &str, candidate: &str, at: Ts) {
        pass_n(store, task, 1, candidate, at);
    }

    /// A passing verdict for a given attempt.
    fn pass_n(store: &mut Store, task: &str, attempt: i64, candidate: &str, at: Ts) {
        store
            .write(|tx| {
                tx.insert_verdict(&gear_store::VerdictRow {
                    project: "p".into(),
                    task: task.into(),
                    attempt,
                    contract_version: 1,
                    candidate: candidate.into(),
                    passed: true,
                    evidence: Some(format!("ops/reviews/{task}.md")),
                    by_seat: "p:captain".into(),
                    created: at,
                })?;
                Ok(())
            })
            .unwrap();
    }

    /// A pass that names the candidate by its full sha while the attempt
    /// stored a short one is the same commit: its merge is queued.
    #[test]
    fn a_pass_naming_the_full_sha_of_a_short_candidate_is_merged() {
        let mut store = pair_world("ea91cbf12", "486aeaa77");
        pass(
            &mut store,
            "T-1",
            "ea91cbf12d23ef97e04c06cf9f60bdedcf759b2f",
            1,
        );
        let passed = passed_tasks(&mut store, "p").unwrap();
        assert_eq!(passed.len(), 1);
        assert_eq!(passed[0].0, "T-1");
    }

    /// A reviewed task whose own merge failed is not waited for: a candidate
    /// that carries it with the fix is how it lands (Lawrence T-17 carried
    /// T-2, and each waited for the other).
    #[test]
    fn a_reviewed_task_whose_merge_failed_is_not_waited_for() {
        let mut store = pair_world("aaa1111", "bbb2222");
        pass(&mut store, "T-1", "aaa1111", 1);
        let carried = |store: &mut Store| {
            other_candidates(store, "p", "T-2")
                .unwrap()
                .into_iter()
                .any(|(task, _, _)| task == "T-1")
        };
        assert!(
            carried(&mut store),
            "a passed task not yet merged is waited for"
        );
        store
            .write(|tx| {
                let op = tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    kind: OpKind::Merge,
                    task: Some("T-1".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({}),
                    created: 2,
                })?;
                tx.set_op_state("p", &op.id, OpState::Executing, None, 2)?;
                tx.set_op_state("p", &op.id, OpState::Failed, Some("gate failed"), 3)
            })
            .unwrap();
        assert!(
            !carried(&mut store),
            "its merge failed: the carrier lands it"
        );
    }

    /// A queued merge naming the full sha of the attempt's short candidate
    /// still merges what was passed.
    #[test]
    fn a_merge_naming_the_full_sha_of_a_short_candidate_is_still_current() {
        let mut store = pair_world("ea91cbf12", "486aeaa77");
        let mut merge = op("ea91cbf12d23ef97e04c06cf9f60bdedcf759b2f");
        merge.task = Some("T-1".into());
        merge.attempt = Some(1);
        assert!(still_current(&mut store, "p", &merge).unwrap().is_ok());
    }

    /// T-62: a merge that waits for another task goes last in the queue: the
    /// task it waits for must be able to land first.
    #[test]
    fn a_waiting_merge_goes_last_in_the_queue() {
        let mut waiting = op("abc1234");
        waiting.id = "O-1".into();
        waiting.state = OpState::Unknown;
        waiting.result = Some("waits for T-1".into());
        let mut running = op("abc1234");
        running.id = "O-2".into();
        running.state = OpState::Unknown;
        running.result = Some("merge will retry from origin".into());

        let order = merge_order(vec![waiting.clone(), running.clone()]);

        assert_eq!(order[0].id, running.id, "the merge that can run goes first");
        assert_eq!(order[1].id, waiting.id, "the waiting merge goes last");
    }

    /// A merge waits while a dep of its task has not landed, and runs once it
    /// has (Prometheus 2026-11-05: T-748 held until T-762 lands).
    #[test]
    fn a_merge_waits_for_its_deps_to_land() {
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.id = "T-6".into();
                task.rank = 6;
                task.deps = vec!["T-5".into()];
                tx.insert_task(&task)
            })
            .unwrap();
        let mut held = op("abc1234");
        held.task = Some("T-6".into());

        let ready = deps_landed(&mut store, "p", vec![held.clone()]).unwrap();
        assert!(ready.is_empty(), "T-5 has not landed: T-6's merge waits");

        lost_task(&mut store, "abc1234", OpState::Confirmed, 5);
        store
            .write(|tx| tx.set_attempt_time("p", "T-5", 1, AttemptStep::Merged, 30))
            .unwrap();
        let ready = deps_landed(&mut store, "p", vec![held]).unwrap();
        assert_eq!(ready.len(), 1, "T-5 landed: T-6 merges");
    }

    /// A merge of a task the plan marks `now` runs before older ordinary ones
    /// (Prometheus T-579, 2026-11-03).
    #[test]
    fn a_now_task_merges_before_older_ordinary_ones() {
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.priority = "must".into();
                tx.update_task(&task)?;
                task.id = "T-6".into();
                task.rank = 6;
                task.priority = "now".into();
                tx.insert_task(&task)
            })
            .unwrap();
        let mut older = op("abc1234");
        older.id = "O-1".into();
        older.task = Some("T-5".into());
        let mut urgent = op("abc1234");
        urgent.id = "O-2".into();
        urgent.task = Some("T-6".into());
        urgent.created = older.created + 10;

        let order = merge_order(now_first(&mut store, "p", vec![older, urgent]).unwrap());

        assert_eq!(order[0].id, "O-2", "the now task goes first");
        assert_eq!(order[1].id, "O-1");
    }

    /// A merge that waited for a task which has merged since is back in the
    /// ordinary queue, ahead of newer merges (Prometheus T-482, 2026-11-03).
    #[test]
    fn a_merge_whose_awaited_task_landed_waits_no_longer() {
        let mut store = world(TaskState::Ready);
        lost_task(&mut store, "abc1234", OpState::Confirmed, 5);
        let mut waiting = op("abc1234");
        waiting.id = "O-1".into();
        waiting.state = OpState::Unknown;
        waiting.result = Some("waits for T-5".into());
        let mut newer = op("abc1234");
        newer.id = "O-2".into();
        newer.created = waiting.created + 10;

        let still = merge_order(
            waits_cleared(&mut store, "p", vec![waiting.clone(), newer.clone()]).unwrap(),
        );
        assert_eq!(
            still[0].id, "O-2",
            "T-5 has not merged yet: O-1 still waits"
        );

        store
            .write(|tx| tx.set_attempt_time("p", "T-5", 1, AttemptStep::Merged, 30))
            .unwrap();
        let order = merge_order(waits_cleared(&mut store, "p", vec![waiting, newer]).unwrap());
        assert_eq!(order[0].id, "O-1", "T-5 landed: O-1 runs first again");
    }

    /// T-62: a task built on another task's branch does not merge before that
    /// task passes. The merge waits (it is not a failure), the captain is
    /// told which task it waits for, and both land in order once the other
    /// task passes.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_merge_waits_for_the_task_its_commits_carry() {
        let pair = Pair::new("carried", true);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        // T-2 passed; T-1 did not (yet). The engine queues T-2's merge itself.
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        // The engine runs T-2's merge: it waits for T-1.
        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Unknown)
        });

        let reason = op_result(&mut store, "O-1");
        assert!(reason.contains("waits for T-1"), "{reason}");
        assert_eq!(
            op_state(&mut store, "O-1"),
            Some(OpState::Unknown),
            "a wait is not a failure"
        );
        // T-2 is not merged: its commits are not on the integration branch.
        let log = pair.dev_log();
        assert!(!log.contains("b"), "T-2 is not merged: {log}");
        assert_eq!(log, "dev", "the integration branch is untouched: {log}");
        // The captain is told once which task it waits for.
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", 30))
            .unwrap();
        assert_eq!(inbox.len(), 1, "{inbox:?}");
        assert!(inbox[0].body.contains("T-1"), "{}", inbox[0].body);
        assert!(inbox[0].body.contains("T-2"), "{}", inbox[0].body);
        assert!(
            inbox[0].body.contains("has not passed"),
            "the line says what the captain has to do: {}",
            inbox[0].body
        );
        // Another tick does not add a second item.
        ticks(&mut store, &mut herdr, &cfg, 3);
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", 30))
            .unwrap();
        assert_eq!(inbox.len(), 1, "{inbox:?}");

        // T-1 passes now: its merge lands first, then T-2's.
        pass(&mut store, "T-1", &pair.a, 40);
        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Confirmed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });

        let log = pair.dev_log();
        assert!(log_has(&log, "a"), "T-1 landed: {log}");
        assert!(log_has(&log, "b"), "T-2 landed: {log}");
        assert!(log_has(&log, "r1"), "T-1's review landed: {log}");
        assert!(log_has(&log, "r2"), "T-2's review landed: {log}");
        // In order: T-1's merge is below T-2's (the log is newest first).
        let first = log.find("merge(T-1)").expect("T-1's merge commit");
        let second = log.find("merge(T-2)").expect("T-2's merge commit");
        assert!(second < first, "T-1 lands before T-2: {log}");
        // The captain's item about the wait is closed.
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", 60))
            .unwrap();
        assert!(inbox.is_empty(), "the wait is over: {inbox:?}");
    }

    /// T-220: the "the merge waits" notice is told once per attempt. A
    /// resolved item does not bring the same news back on the next tick, and
    /// the next attempt of the same task is told once again.
    ///
    /// The tick calls `tell_captain_waiting` once per waiting merge; this
    /// test calls it as the tick does.
    #[test]
    fn the_merge_waits_notice_is_told_once_per_attempt() {
        let mut store = world(TaskState::Verifying);
        let waiting = op("abc1234"); // T-5, attempt 1
                                     // T-1 has not passed: the merge of T-5 waits for it.
        let blocked = vec![("T-1".to_string(), false)];

        // The first tick: the captain is told once.
        tell_captain_waiting(&mut store, "p", &waiting, &blocked, 10).unwrap();
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", 11))
            .unwrap();
        assert_eq!(inbox.len(), 1, "the captain is told: {inbox:?}");
        assert!(inbox[0].body.contains("T-1"), "{}", inbox[0].body);
        assert!(inbox[0].body.contains("T-5"), "{}", inbox[0].body);

        // The captain resolves the item.
        let id = inbox[0].id.clone();
        store
            .write(|tx| {
                tx.update_message_state("p", &id, MsgState::Resolved, Some("ok"), None, None, 12)
            })
            .unwrap();

        // The later ticks add no item: the notice was already told for this
        // attempt.
        tell_captain_waiting(&mut store, "p", &waiting, &blocked, 20).unwrap();
        tell_captain_waiting(&mut store, "p", &waiting, &blocked, 30).unwrap();
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", 31))
            .unwrap();
        assert!(inbox.is_empty(), "the notice comes back: {inbox:?}");
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-wait:T-5"))
            .unwrap();
        assert_eq!(items.len(), 1, "a second item was added: {items:?}");

        // The next attempt of the same task is told once again.
        let mut next = waiting.clone();
        next.attempt = Some(2);
        tell_captain_waiting(&mut store, "p", &next, &blocked, 40).unwrap();
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", 41))
            .unwrap();
        assert_eq!(inbox.len(), 1, "the next attempt is told again: {inbox:?}");
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-wait:T-5"))
            .unwrap();
        assert_eq!(items.len(), 2, "the next attempt has no item: {items:?}");
    }

    /// T-62: a task that is not built on another task's branch is not held.
    /// The task as a settled pass leaves it: attempt 1 accepted, the task done.
    fn accept(store: &mut Store, task: &str, now: Ts) {
        store
            .write(|tx| {
                let mut attempt = tx.get_attempt("p", task, 1)?.unwrap();
                attempt.state = gear_model::AttemptState::Accepted;
                tx.update_attempt(&attempt)?;
                let mut row = tx.get_task("p", task)?.unwrap();
                row.state = TaskState::Done;
                row.updated = now;
                tx.update_task(&row)
            })
            .unwrap();
    }

    /// `gear plan apply` with a single `- remerge T-<n>` staff line.
    fn plan_remerge(store: &mut Store, task: &str, now: Ts) -> GearResult<()> {
        let text = format!("<!-- gear-plan project=p base=0 -->\n# Staff\n- remerge {task}\n");
        let draft = gear_app::draft::parse(&text)?;
        let env = gear_app::Env {
            ctx: Some(gear_model::GearCtx {
                project: "p".into(),
                seat: "captain".into(),
                generation: 1,
            }),
            pane_id: Some("w9:p1".into()),
            admin: false,
        };
        let fp = gear_app::identity::Fingerprint {
            pane_id: "w9:p1".into(),
            kind: Some("claude".into()),
            session_ref: None,
        };
        gear_app::plan::apply(
            store,
            &env,
            Some(&fp),
            None,
            &draft,
            &text,
            &format!("remerge-{task}-{now}"),
            now,
        )
        .map(|_| ())
    }

    /// T-92: a merge that failed can be retried from the plan, and the failed
    /// item closes when it lands. The gate fails once (a flaky test), as it
    /// did on T-89.
    #[test]
    fn a_failed_merge_is_retried_from_the_plan_and_its_item_closes() {
        let scratch = Scratch::new("remerge");
        let marker = scratch.root.join("gate-ran");
        let gate = format!(
            "if [ ! -e '{marker}' ]; then touch '{marker}'; echo the flaky test >&2; exit 1; fi",
            marker = marker.display()
        );
        let (mut store, mut herdr, cfg) = queued_merge(&scratch, &gate);
        accept(&mut store, "T-5", 0);

        // The first merge fails on the gate; nothing is pushed.
        let merged = wait_merge(&mut store, &mut herdr, &cfg);
        assert!(merged.is_empty(), "{merged:?}");
        let op = merge_op(&mut store);
        assert_eq!(op.state, OpState::Failed, "{op:?}");
        assert_eq!(scratch.dev_log(), "dev", "nothing was pushed");
        // The item names the plan line that retries it.
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", 30))
            .unwrap();
        assert_eq!(inbox.len(), 1, "{inbox:?}");
        assert!(
            inbox[0].body.contains("merge of T-5 failed"),
            "{}",
            inbox[0].body
        );
        assert!(inbox[0].body.contains("- remerge T-5"), "{}", inbox[0].body);

        // The captain retries it from the plan: a new op, which lands.
        plan_remerge(&mut store, "T-5", 40).unwrap();
        assert_eq!(
            op_state(&mut store, "O-2"),
            Some(OpState::Intended),
            "a new merge op"
        );
        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-2") == Some(OpState::Confirmed)
        });
        assert!(scratch.dev_has_task_commit(), "the retry landed");
        // The failed item closes with the merge.
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", 60))
            .unwrap();
        assert!(inbox.is_empty(), "the item is settled: {inbox:?}");
    }

    /// T-92: a merge that is already in flight or merged has nothing to retry,
    /// and the plan line says why.
    #[test]
    fn a_remerge_is_refused_without_a_failed_merge() {
        let scratch = Scratch::new("remerge-refuse");
        let (mut store, _herdr, _cfg) = queued_merge(&scratch, "true");
        accept(&mut store, "T-5", 0);
        // The merge op is Intended: nothing failed.
        let error = plan_remerge(&mut store, "T-5", 40).unwrap_err();
        assert!(error.detail.contains("already intended"), "{error:?}");
        // Merged: the last op is Confirmed.
        store
            .write(|tx| {
                tx.set_op_state("p", "O-1", OpState::Executing, None, 41)?;
                tx.set_op_state("p", "O-1", OpState::Submitted, None, 41)?;
                tx.set_op_state("p", "O-1", OpState::Confirmed, Some("merged"), 41)
            })
            .unwrap();
        let error = plan_remerge(&mut store, "T-5", 42).unwrap_err();
        assert!(error.detail.contains("already merged"), "{error:?}");
    }

    /// T-92: a task the engine never queued a merge for is refused, and so is
    /// a task that is not there.
    #[test]
    fn a_remerge_needs_a_task_with_a_merged_attempt() {
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-5", 1, 1, "abc1234", "report", 0)
            })
            .unwrap();
        accept(&mut store, "T-5", 0);
        // An accepted task with no merge of its own (it was carried, Lawrence
        // T-34) gets its first one from a remerge.
        let error = plan_remerge(&mut store, "T-99", 40).unwrap_err();
        assert!(error.detail.contains("no task T-99"), "{error:?}");
        plan_remerge(&mut store, "T-5", 41).unwrap();
        assert_eq!(merge_op(&mut store).state, OpState::Intended);
    }

    /// A done task whose attempt was accepted with `candidate`, whose merge op
    /// ended in `state` (T-397): the state a merge leaves when it failed, was
    /// voided, or never ran.
    fn lost_task(store: &mut Store, candidate: &str, state: OpState, now: Ts) {
        store
            .write(|tx| {
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-5", 1, 1, candidate, "report", 0)?;
                let mut attempt = tx.get_attempt("p", "T-5", 1)?.unwrap();
                attempt.state = gear_model::AttemptState::Accepted;
                tx.update_attempt(&attempt)?;
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.state = TaskState::Done;
                tx.update_task(&task)?;
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
                if state != OpState::Intended {
                    tx.set_op_state("p", "O-1", state, Some("gate said no"), now)?;
                    // T-516: a settled merge op writes the settle time, and the
                    // lost scan is due in the tick after one. The fixture seeds
                    // the op directly, so it seeds that word too.
                    if matches!(state, OpState::Confirmed | OpState::Failed) {
                        tx.set_meta(&settled_key("p"), &now.to_string())?;
                    }
                }
                Ok(())
            })
            .unwrap();
    }

    /// The failed-merge item of T-5, resolved: the captain settled it without a
    /// remerge (T-397).
    fn failed_item_resolved(store: &mut Store, now: Ts) {
        store
            .write(|tx| {
                tx.insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: failed_thread("T-5"),
                    task: Some("T-5".into()),
                    attempt: Some(1),
                    sender: "p:captain".into(),
                    sender_gen: 1,
                    recipient: "p:captain".into(),
                    kind: MsgKind::Question,
                    urgent: false,
                    body: "merge of T-5 failed: the gate said no".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Resolved,
                    created: now,
                })?;
                Ok(())
            })
            .unwrap();
    }

    /// A lost item closes once a merge of the task is queued again: the merge
    /// will land it (Prometheus T-721/T-723, 2026-11-05: a red batch's members
    /// were named lost, then queued again, and the items stayed open).
    #[test]
    fn a_lost_item_closes_once_a_merge_of_it_is_queued_again() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();
        lost_task(&mut store, "abc1234", OpState::Failed, 10);
        failed_item_resolved(&mut store, 11);
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let open = |store: &mut Store| {
            store
                .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
                .unwrap()
                .into_iter()
                .filter(|item| item.state == MsgState::Open)
                .count()
        };
        assert_eq!(open(&mut store), 1, "the lost work is named");

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
                    payload: serde_json::json!({ "branch": "heat/t5", "candidate": "abc1234" }),
                    created: 25,
                })?;
                tx.set_meta(&settled_key("p"), "25")
            })
            .unwrap();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 30).unwrap();

        assert_eq!(open(&mut store), 0, "a queued merge answers the item");
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert_eq!(items[0].result.as_deref(), Some(LOST_QUEUED));
    }

    /// T-397: a merge failed, its item was resolved without a remerge, and the
    /// accepted work sat unmerged with no word about it. The engine names it
    /// once, with the plan line that retries it, and the next tick is quiet.
    #[test]
    fn an_accepted_attempt_that_nothing_merges_is_named_once() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();
        lost_task(&mut store, "abc1234", OpState::Failed, 10);
        failed_item_resolved(&mut store, 11);

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert_eq!(items.len(), 1, "the lost work was not named: {items:?}");
        assert_eq!(
            items[0].body,
            "T-5 attempt 1 was accepted but is not merged and nothing will merge it: \
             `- remerge T-5` under `# Staff` retries it"
        );
        assert_eq!(items[0].state, MsgState::Open);

        // Once per attempt: a later tick adds no second item.
        merge_passed(&mut store, &mut herdr, &cfg, "p", 21).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert_eq!(items.len(), 1, "a second tick added an item: {items:?}");
    }

    /// `carried-by: T-m` (Lawrence T-31, 2026-11-03): a passed task whose work
    /// lands with another task's merge gets no merge of its own, and it is not
    /// named lost while its carrier has not merged.
    #[test]
    fn a_carried_task_queues_no_merge_and_is_not_named_lost() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();
        store
            .write(|tx| {
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-5", 1, 1, "abc1234", "report", 0)?;
                let mut attempt = tx.get_attempt("p", "T-5", 1)?.unwrap();
                attempt.state = gear_model::AttemptState::Accepted;
                tx.update_attempt(&attempt)?;
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.state = TaskState::Done;
                tx.update_task(&task)?;
                tx.set_task_carried_by("p", "T-5", "T-9")
            })
            .unwrap();
        // The carrier T-9 has no merge in flight: the carried work lands
        // nothing, so it is named lost (user, 2026-11-04).
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20 + LOST_SCAN_EVERY_S).unwrap();
        let merges = store
            .read(|tx| tx.ops_of_kind_in_states("p", OpKind::Merge, &[]))
            .unwrap();
        assert!(merges.is_empty(), "a carried task got a merge: {merges:?}");
        let lost = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert_eq!(
            lost.len(),
            1,
            "a carried task whose carrier merges nothing: {lost:?}"
        );
        assert!(lost[0].urgent, "lost accepted work is urgent");

        // Without the mark, the same task is queued as usual.
        store
            .write(|tx| tx.set_task_carried_by("p", "T-5", ""))
            .unwrap();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 30 + LOST_SCAN_EVERY_S).unwrap();
        let merges = store
            .read(|tx| tx.ops_of_kind_in_states("p", OpKind::Merge, &[]))
            .unwrap();
        assert_eq!(merges.len(), 1, "{merges:?}");
    }

    /// A merge queued before the task was marked `carried-by` is closed, not
    /// run: the work lands with its carrier (Prometheus T-574/T-577).
    #[test]
    fn a_queued_merge_of_a_task_that_became_carried_is_voided() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();
        lost_task(&mut store, "abc1234", OpState::Intended, 10);
        store
            .write(|tx| tx.set_task_carried_by("p", "T-5", "T-9"))
            .unwrap();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Void, "{op:?}");
        assert!(
            op.result
                .as_deref()
                .is_some_and(|r| r.contains("carried-by")),
            "{op:?}"
        );
    }

    /// T-397: a merge that is still queued will merge the attempt itself: it is
    /// not lost.
    #[test]
    fn an_attempt_with_a_queued_merge_is_not_named_lost() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();
        lost_task(&mut store, "abc1234", OpState::Intended, 10);
        // T-516 (F4 of the review of attempt 1): the scan is due, so this tick
        // really runs it — the empty inbox means the queued-merge filter held,
        // not that nothing was read.
        store
            .write(|tx| tx.set_meta(&lost_scan_key("p"), &(20 - LOST_SCAN_EVERY_S).to_string()))
            .unwrap();

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert!(
            items.is_empty(),
            "a queued merge is not a lost one: {items:?}"
        );
        let scanned = store.read(|tx| tx.get_meta(&lost_scan_key("p"))).unwrap();
        assert_eq!(scanned.as_deref(), Some("20"), "the scan ran on this tick");
    }

    /// T-397: an attempt that carries its merge time is merged: no item.
    #[test]
    fn a_merged_attempt_is_not_named_lost() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();
        lost_task(&mut store, "abc1234", OpState::Failed, 10);
        failed_item_resolved(&mut store, 11);
        store
            .write(|tx| tx.set_attempt_time("p", "T-5", 1, gear_store::AttemptStep::Merged, 12))
            .unwrap();

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert!(items.is_empty(), "a merged attempt is not lost: {items:?}");
    }

    /// F1 of the T-397 review: the ancestor check asks the repo and the base
    /// the *task* was created against, not its lane's current ones. A
    /// completed task keeps its snapshot, so a candidate that is already on
    /// the task's saved base — which differs from the project's current
    /// integration branch — is not lost.
    #[test]
    fn a_candidate_on_the_tasks_saved_base_is_not_named_lost() {
        let scratch = Scratch::new("merge-lost-saved-base");
        // The accepted work is already on the base the task was cut from, and
        // the project integrates on `dev`, which does not hold it.
        let candidate = scratch.head_of("heat/t5");
        sh(
            &scratch.work,
            &format!("git update-ref refs/remotes/origin/old-dev {candidate}"),
        );
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text_in("true", &scratch.work);
                tx.update_project(&project)?;
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.repo = Some(scratch.work.display().to_string());
                task.base = Some("old-dev".into());
                tx.update_task(&task)
            })
            .unwrap();
        lost_task(&mut store, &candidate, OpState::Failed, 10);
        failed_item_resolved(&mut store, 11);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert!(
            items.is_empty(),
            "the work is on the task's own base, not the lane's: {items:?}"
        );
    }

    /// T-397: a candidate that is already an ancestor of the integration branch
    /// is left alone — and the answer is remembered, so git is asked once.
    #[test]
    fn a_candidate_on_the_integration_branch_is_not_named_lost() {
        let scratch = Scratch::new("merge-lost-landed");
        let landed = scratch.head_of("dev");
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text_in("true", &scratch.work);
                tx.update_project(&project)
            })
            .unwrap();
        lost_task(&mut store, &landed, OpState::Failed, 10);
        failed_item_resolved(&mut store, 11);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert!(items.is_empty(), "the work is on the branch: {items:?}");
        assert!(
            store
                .read(|tx| tx.get_meta(&merge_lost_landed_key("p", "T-5", 1)))
                .unwrap()
                .is_some(),
            "the ancestor answer is kept"
        );
    }

    /// T-408: a task of a voyage whose tasks are all terminal is not checked —
    /// a finished voyage settled its lost merges with it (M1's T-13 was one of
    /// the 34 items T-397 opened the day it went live).
    #[test]
    fn a_task_of_a_finished_voyage_is_not_named_lost() {
        let mut store = world(TaskState::Ready);
        // T-5 is the only task of M1, and it is Done: the voyage is finished.
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.milestone = Some("M1".into());
                tx.update_task(&task)
            })
            .unwrap();
        lost_task(&mut store, "abc1234", OpState::Failed, 10);
        failed_item_resolved(&mut store, 11);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();

        // The last task settled more than LAND_WITHIN_S ago: the voyage is over.
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20 + LAND_WITHIN_S).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert!(
            items.is_empty(),
            "a finished voyage is not checked: {items:?}"
        );
    }

    /// A voyage whose last task has just settled is still open: that task holds
    /// the final candidate, and its unmerged work is named (Prometheus T-779
    /// sat accepted and unmerged for 20 minutes, unnamed, because every task of
    /// M4 was done).
    #[test]
    fn the_last_task_of_a_voyage_is_named_lost() {
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.milestone = Some("M1".into());
                tx.update_task(&task)
            })
            .unwrap();
        lost_task(&mut store, "abc1234", OpState::Failed, 10);
        failed_item_resolved(&mut store, 11);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert_eq!(items.len(), 1, "the voyage's last task: {items:?}");
    }

    /// T-408: a candidate the task's repository no longer holds is skipped —
    /// the item would name work that nobody can remerge (M1's T-13 named
    /// 6e555b1, which is no longer an object in the repository).
    #[test]
    fn a_candidate_the_repository_no_longer_holds_is_not_named_lost() {
        let scratch = Scratch::new("merge-lost-gone");
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text_in("true", &scratch.work);
                tx.update_project(&project)
            })
            .unwrap();
        // A commit the clone does not have: `git cat-file -e` does not find it.
        lost_task(&mut store, "6e555b1", OpState::Failed, 10);
        failed_item_resolved(&mut store, 11);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert!(
            items.is_empty(),
            "a candidate the repo does not hold is not named: {items:?}"
        );
        assert!(
            store
                .read(|tx| tx.get_meta(&merge_lost_landed_key("p", "T-5", 1)))
                .unwrap()
                .is_none(),
            "a candidate that is gone did not land"
        );
    }

    /// T-408: an item that is still open about a task the scan now skips closes
    /// with the word that says why.
    #[test]
    fn an_open_lost_item_of_a_skipped_task_is_resolved() {
        let scratch = Scratch::new("merge-lost-skipped");
        let candidate = scratch.local_head_of("heat/t5");
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text_in("true", &scratch.work);
                tx.update_project(&project)
            })
            .unwrap();
        lost_task(&mut store, &candidate, OpState::Failed, 10);
        failed_item_resolved(&mut store, 11);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();

        // The voyage is open and the repo holds the candidate: the work is named.
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert_eq!(items.len(), 1, "the lost work was not named: {items:?}");
        assert_eq!(items[0].state, MsgState::Open);

        // The voyage finishes: T-5 is its only task and it is done.
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.milestone = Some("M1".into());
                tx.update_task(&task)
            })
            .unwrap();

        // The voyage counts as finished once its last task settled
        // LAND_WITHIN_S ago, and the scan is due again by then.
        merge_passed(&mut store, &mut herdr, &cfg, "p", 21 + LAND_WITHIN_S).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert_eq!(items.len(), 1, "a second item was opened: {items:?}");
        assert_eq!(items[0].state, MsgState::Resolved, "{items:?}");
        assert_eq!(items[0].result.as_deref(), Some(LOST_SKIPPED));
    }

    /// T-458: the skip rule runs **before** the old filters (T-408 review F1).
    /// An open merge-lost item used to stay open when the task had a merge in
    /// flight, because the scan dropped the task before the skip check.
    #[test]
    fn an_open_lost_item_closes_while_a_merge_is_in_flight() {
        let scratch = Scratch::new("merge-lost-in-flight");
        let candidate = scratch.local_head_of("heat/t5");
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text_in("true", &scratch.work);
                tx.update_project(&project)
            })
            .unwrap();
        lost_task(&mut store, &candidate, OpState::Failed, 10);
        failed_item_resolved(&mut store, 11);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();

        // The voyage is open and the repo holds the candidate: the work is named.
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert_eq!(items.len(), 1, "the lost work was not named: {items:?}");
        assert_eq!(items[0].state, MsgState::Open);

        // The voyage finishes (T-5 is its only task and it is done) while a
        // new merge for it is queued: the old filter would drop the task.
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.milestone = Some("M1".into());
                tx.update_task(&task)?;
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
                    created: 21,
                })
            })
            .unwrap();

        // A tick where the voyage counts as finished and the scan is due.
        merge_passed(&mut store, &mut herdr, &cfg, "p", 22 + LAND_WITHIN_S).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert_eq!(items.len(), 1, "a second item was opened: {items:?}");
        assert_eq!(items[0].state, MsgState::Resolved, "{items:?}");
        assert_eq!(items[0].result.as_deref(), Some(LOST_SKIPPED));
    }

    /// T-569: a captain item on a task thread closes once the task is done,
    /// and stays open while it runs (M-10318: a note on T-715 rang for hours
    /// after the task was done).
    #[test]
    fn an_item_on_a_task_thread_closes_when_the_task_is_done() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();
        lost_task(&mut store, "abc1234", OpState::Failed, 5);
        // A note of a lane to the captain, on the task's own thread, and the
        // captain's decision item on `T-5:cap`.
        store
            .write(|tx| {
                for thread in ["T-5:1", "T-5:cap"] {
                    tx.insert_message(&gear_store::NewMessage {
                        meta: None,
                        project: "p".into(),
                        thread: thread.into(),
                        task: Some("T-5".into()),
                        attempt: Some(1),
                        sender: "p:heat".into(),
                        sender_gen: 1,
                        recipient: "p:captain".into(),
                        kind: gear_model::MsgKind::Instruction,
                        urgent: true,
                        body: "a note about T-5".into(),
                        reply_to: None,
                        needs_ack: false,
                        state: MsgState::Open,
                        created: 5,
                    })?;
                }
                Ok(())
            })
            .unwrap();
        let states = |store: &mut Store| {
            store
                .read(|tx| {
                    Ok((
                        tx.thread_messages("p", "T-5:1")?,
                        tx.thread_messages("p", "T-5:cap")?,
                    ))
                })
                .unwrap()
        };

        // The task still runs: its items stay open.
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.state = TaskState::Running;
                tx.update_task(&task)
            })
            .unwrap();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 10 + LOST_SCAN_EVERY_S).unwrap();
        let (item, decision) = states(&mut store);
        assert_eq!(item[0].state, MsgState::Open, "{item:?}");
        assert_eq!(decision[0].state, MsgState::Open, "{decision:?}");

        // The task is done: both close, and say why.
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.state = TaskState::Done;
                tx.update_task(&task)
            })
            .unwrap();
        merge_passed(
            &mut store,
            &mut herdr,
            &cfg,
            "p",
            11 + 2 * LOST_SCAN_EVERY_S,
        )
        .unwrap();
        let (item, decision) = states(&mut store);
        assert_eq!(item[0].state, MsgState::Resolved, "{item:?}");
        assert_eq!(item[0].result.as_deref(), Some("the task is done"));
        assert_eq!(decision[0].state, MsgState::Resolved, "{decision:?}");
        assert_eq!(decision[0].result.as_deref(), Some("the task is done"));
    }

    /// T-569: an open merge-failed item of a task whose attempt has a merge
    /// time closes, even when the task is not done.
    #[test]
    fn a_failed_item_closes_once_its_attempt_has_a_merge_time() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();
        lost_task(&mut store, "abc1234", OpState::Failed, 5);
        store
            .write(|tx| {
                tx.insert_message(&gear_store::NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: failed_thread("T-5"),
                    task: Some("T-5".into()),
                    attempt: Some(1),
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: "p:captain".into(),
                    kind: gear_model::MsgKind::System,
                    urgent: false,
                    body: "the merge of T-5 failed".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: 5,
                })
            })
            .unwrap();
        let state = |store: &mut Store| {
            store
                .read(|tx| tx.thread_messages("p", &failed_thread("T-5")))
                .unwrap()[0]
                .state
        };
        merge_passed(&mut store, &mut herdr, &cfg, "p", 10 + LOST_SCAN_EVERY_S).unwrap();
        assert_eq!(state(&mut store), MsgState::Open, "nothing landed yet");

        store
            .write(|tx| tx.set_attempt_time("p", "T-5", 1, AttemptStep::Merged, 30))
            .unwrap();
        merge_passed(
            &mut store,
            &mut herdr,
            &cfg,
            "p",
            31 + 2 * LOST_SCAN_EVERY_S,
        )
        .unwrap();
        assert_eq!(state(&mut store), MsgState::Resolved, "its work landed");
    }

    /// A merge-lost item closes once the work landed with another task's
    /// merge (`integrated-by`): the item asked for a remerge of work that is
    /// already on the integration branch (Lawrence M-81).
    #[test]
    fn a_lost_item_closes_once_the_work_landed_with_another_task() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();
        lost_task(&mut store, "abc1234", OpState::Failed, 10);
        failed_item_resolved(&mut store, 11);
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert_eq!(items[0].state, MsgState::Open, "{items:?}");

        // The work lands with another task's merge.
        store
            .write(|tx| tx.set_attempt_time("p", "T-5", 1, AttemptStep::Merged, 30))
            .unwrap();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 31 + LOST_SCAN_EVERY_S).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-lost:T-5"))
            .unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].state, MsgState::Resolved, "{items:?}");
        assert_eq!(items[0].result.as_deref(), Some(LOST_LANDED));
    }

    /// Prometheus M-8629 / M-9797 (2026-11-05): a task carried by another
    /// (`carried-by`) has no merge time of its own; once its carrier merged,
    /// its merge-failed item closes, and a reply may close it too.
    #[test]
    fn a_failed_merge_item_of_a_carried_task_closes_once_its_carrier_merged() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();
        lost_task(&mut store, "abc1234", OpState::Failed, 5);
        store
            .write(|tx| {
                let mut carrier = tx.get_task("p", "T-5")?.unwrap();
                carrier.id = "T-6".into();
                carrier.rank = 2;
                carrier.state = TaskState::Ready;
                tx.insert_task(&carrier)?;
                gear_app::tasks::start_attempt(tx, "p", "T-6", "p:heat", 1, 0)?;
                tx.set_task_carried_by("p", "T-5", "T-6")?;
                tx.insert_message(&gear_store::NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: failed_thread("T-5"),
                    task: Some("T-5".into()),
                    attempt: Some(1),
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: "p:captain".into(),
                    kind: gear_model::MsgKind::System,
                    urgent: false,
                    body: "the merge of T-5 failed".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: 5,
                })?;
                Ok(())
            })
            .unwrap();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 10).unwrap();
        let state = |store: &mut Store| {
            store
                .read(|tx| tx.thread_messages("p", &failed_thread("T-5")))
                .unwrap()[0]
                .state
        };
        assert_eq!(
            state(&mut store),
            MsgState::Open,
            "the carrier has not merged"
        );
        store
            .write(|tx| tx.set_attempt_time("p", "T-6", 1, AttemptStep::Merged, 30))
            .unwrap();
        assert!(store
            .read(|tx| gear_app::tasks::work_landed(tx, "p", "T-5"))
            .unwrap());
        merge_passed(&mut store, &mut herdr, &cfg, "p", 31 + LOST_SCAN_EVERY_S).unwrap();
        assert_eq!(state(&mut store), MsgState::Resolved, "its carrier merged");
    }

    /// A merge-failed item closes once the attempt it names has landed after
    /// all (Lawrence M-182: T-14 landed with T-26, `integrated-by`).
    #[test]
    fn a_failed_merge_item_closes_once_its_work_landed() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let cfg = test_cfg();
        lost_task(&mut store, "abc1234", OpState::Failed, 5);
        store
            .write(|tx| {
                tx.insert_message(&gear_store::NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: failed_thread("T-5"),
                    task: Some("T-5".into()),
                    attempt: Some(1),
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: "p:captain".into(),
                    kind: gear_model::MsgKind::System,
                    urgent: false,
                    body: "the merge of T-5 failed".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: 5,
                })?;
                Ok(())
            })
            .unwrap();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 10).unwrap();
        let open = |store: &mut Store| {
            store
                .read(|tx| tx.thread_messages("p", &failed_thread("T-5")))
                .unwrap()[0]
                .state
        };
        assert_eq!(open(&mut store), MsgState::Open, "nothing landed yet");

        store
            .write(|tx| tx.set_attempt_time("p", "T-5", 1, AttemptStep::Merged, 30))
            .unwrap();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 31 + LOST_SCAN_EVERY_S).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", &failed_thread("T-5")))
            .unwrap();
        assert_eq!(items[0].state, MsgState::Resolved, "{items:?}");
        assert_eq!(items[0].result.as_deref(), Some(LOST_LANDED));
    }

    /// A new failure of a merge whose failed item is still open rings again
    /// (Prometheus T-597: the third failure only rewrote an item the captain
    /// had seen, so no doorbell named it). The same failure again stays quiet.
    #[test]
    fn a_new_failure_of_the_same_merge_opens_a_fresh_item() {
        let mut store = world(TaskState::Ready);
        lost_task(&mut store, "abc1234", OpState::Failed, 5);
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        tell_captain_failed(&mut store, "p", &op, "the gate said no", 10).unwrap();
        tell_captain_failed(&mut store, "p", &op, "the gate said no", 11).unwrap();
        assert_eq!(failed_items(&mut store, "T-5").len(), 1, "the same failure");

        tell_captain_failed(&mut store, "p", &op, "a test failed", 20).unwrap();
        let items = failed_items(&mut store, "T-5");
        let open: Vec<_> = items.iter().filter(|m| m.state == MsgState::Open).collect();
        assert_eq!(open.len(), 1, "{items:?}");
        assert!(open[0].body.contains("a test failed"), "{}", open[0].body);
        assert_eq!(items.len(), 2, "the old item closed, a fresh one opened");
    }

    /// T-566: a merge worktree that cannot be used rings once, not once per
    /// retry, and the next merge that starts closes the item.
    #[test]
    fn a_repeating_worktree_failure_keeps_one_item_and_a_merge_that_starts_closes_it() {
        let scratch = Scratch::new("merge-wait-item");
        let foreign_a = Scratch::new("merge-wait-foreign-a");
        let foreign_b = Scratch::new("merge-wait-foreign-b");
        let mut store = world(TaskState::Ready);
        let candidate = scratch.head_of("heat/t5");
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text_in("true", &scratch.work);
                tx.update_project(&project)
            })
            .unwrap();
        lost_task(&mut store, &candidate, OpState::Intended, 5);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        // A checkout of another repository stands where the merge worktree goes.
        herdr.add_worktree(
            "p/merge/between-voyages",
            &foreign_a.work.display().to_string(),
            "wt-merge:p1",
        );
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        // Five retries of the same blocked merge.
        for tick in 0..3 {
            merge_passed(&mut store, &mut herdr, &cfg, "p", 20 + tick * 30).unwrap();
        }
        // The cause changes (another repository's checkout): the same item
        // carries the latest reason.
        herdr.add_worktree(
            "p/merge/between-voyages",
            &foreign_b.work.display().to_string(),
            "wt-merge:p1",
        );
        for tick in 3..5 {
            merge_passed(&mut store, &mut herdr, &cfg, "p", 20 + tick * 30).unwrap();
        }

        let items = store
            .read(|tx| tx.thread_messages("p", "merge-wait:p:worktree"))
            .unwrap();
        assert_eq!(items.len(), 1, "one item, not one per retry: {items:?}");
        assert_eq!(items[0].state, MsgState::Open, "still waiting");
        assert!(
            items[0].body.starts_with("merge waits: "),
            "{}",
            items[0].body
        );
        assert!(
            items[0]
                .body
                .contains(&foreign_b.work.display().to_string()),
            "the latest reason: {}",
            items[0].body
        );
        assert!(
            !items[0]
                .body
                .contains(&foreign_a.work.display().to_string()),
            "the earlier reason is gone: {}",
            items[0].body
        );

        // The merge worktree is right now: the merge starts and the item closes.
        let path = scratch.root.join("merge");
        sh(
            &scratch.root,
            &format!(
                "git -C {} worktree add -q -b p/merge/between-voyages {} origin/dev",
                scratch.work.display(),
                path.display()
            ),
        );
        herdr.add_worktree(
            "p/merge/between-voyages",
            &path.display().to_string(),
            "wt-merge:p1",
        );
        merge_passed(&mut store, &mut herdr, &cfg, "p", 400).unwrap();
        let items = store
            .read(|tx| tx.thread_messages("p", "merge-wait:p:worktree"))
            .unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].state, MsgState::Resolved, "{items:?}");
        assert_eq!(items[0].result.as_deref(), Some(MERGE_WAIT_OVER));
    }

    /// T-565, T-584: the merge seat is not a `SeatRow`, so no apply and no lane
    /// pass moves its branch. A repo still on the previous default
    /// (`p/merge/home`) gets its merge worktree on the voyage's name anyway
    /// (`p/merge/between-voyages`: no voyage has departed): the old ref is
    /// settled before git is asked for the branch, and no D/F refusal is
    /// reported.
    #[test]
    fn a_merge_seat_on_the_previous_default_branch_is_moved_before_its_worktree() {
        let scratch = Scratch::new("merge-seat-old-default");
        // The ref the previous default gave the merge seat.
        sh(
            &scratch.root,
            &format!("git -C {} branch p/merge/home dev", scratch.work.display()),
        );
        let mut store = world(TaskState::Ready);
        let candidate = scratch.head_of("heat/t5");
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text_in("true", &scratch.work);
                tx.update_project(&project)
            })
            .unwrap();
        lost_task(&mut store, &candidate, OpState::Intended, 5);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();

        // The ref moved to the current default, and the worktree was asked for
        // on the new name.
        assert!(
            git_ref_exists(&scratch.work, "refs/heads/p/merge/between-voyages"),
            "the new name is a branch"
        );
        assert!(
            !git_ref_exists(&scratch.work, "refs/heads/p/merge/home"),
            "the old name is gone"
        );
        assert!(
            herdr
                .worktree_branches()
                .contains(&"p/merge/between-voyages"),
            "the merge worktree is on the new branch: {:?}",
            herdr.worktree_branches()
        );
        let waits = store
            .read(|tx| tx.thread_messages("p", "merge-wait:p:worktree"))
            .unwrap();
        assert!(waits.is_empty(), "no D/F refusal is reported: {waits:?}");
    }

    /// T-565: a ref of another name that is a folder of the merge seat's branch
    /// is named and left alone — the merge waits, and nothing is deleted.
    #[test]
    fn a_foreign_ref_in_the_way_of_the_merge_branch_is_named_and_left_alone() {
        let scratch = Scratch::new("merge-seat-foreign");
        sh(
            &scratch.root,
            &format!("git -C {} branch p dev", scratch.work.display()),
        );
        let mut store = world(TaskState::Ready);
        let candidate = scratch.head_of("heat/t5");
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text_in("true", &scratch.work);
                tx.update_project(&project)
            })
            .unwrap();
        lost_task(&mut store, &candidate, OpState::Intended, 5);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();

        // The wait is one item on the merge-wait thread (T-566) or, before
        // that lands, on the thread the merge tells the captain on this tick.
        let said = |store: &mut Store, thread: &str| {
            store
                .read(|tx| tx.thread_messages("p", thread))
                .unwrap_or_default()
        };
        let mut waits = said(&mut store, "merge-wait:p:worktree");
        if waits.is_empty() {
            waits = said(&mut store, "merge:20");
        }
        assert!(
            waits
                .iter()
                .any(|item| item.body.contains("the ref p stands in its way")),
            "the ref is named: {waits:?}"
        );
        // The foreign ref is untouched and nothing was created over it.
        assert!(
            git_ref_exists(&scratch.work, "refs/heads/p"),
            "the foreign ref is untouched"
        );
        assert!(
            !git_ref_exists(&scratch.work, "refs/heads/p/merge/between-voyages"),
            "nothing was created over it"
        );
    }

    /// Whether the repo at `dir` has the ref `ref` (T-565).
    fn git_ref_exists(dir: &Path, name: &str) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "--verify", "--quiet", name])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// The worktree of a merge train's second stage, beside the pair's own.
    fn second_stage(pair: &Pair, herdr: &mut gear_herdr::fake::FakeHerdr) -> PathBuf {
        let path = pair.root.join("merge-2");
        sh(
            &pair.root,
            &format!(
                "git -C {} worktree add -q -b p/merge-2/between-voyages {} origin/dev",
                pair.work.display(),
                path.display()
            ),
        );
        herdr.add_worktree(
            "p/merge-2/between-voyages",
            &path.display().to_string(),
            "wt-merge-2:p1",
        );
        path
    }

    /// The project's merges gate two at a time, with `gate` as its gate.
    fn train_of_two(store: &mut Store, gate: &str) {
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text(gate)
                    .replace("[conventions]\n", "[conventions]\nmerge_parallel = 2\n");
                tx.update_project(&project)
            })
            .unwrap();
    }

    /// Three tasks of one project, each cut from `dev` (T-574's merge order).
    struct Trio {
        root: PathBuf,
        work: PathBuf,
    }

    impl Trio {
        fn new(name: &str) -> Trio {
            let root = scratch_root(name, Shape::TrioFlat);
            let work = root.join("work");
            Trio { root, work }
        }

        fn merge_worktree(&self) -> gear_herdr::Worktree {
            let path = self.root.join("merge");
            sh(
                &self.root,
                &format!(
                    "git -C {} worktree add -q -b p/merge/between-voyages {} origin/dev",
                    self.work.display(),
                    path.display()
                ),
            );
            gear_herdr::Worktree {
                workspace_id: "wt-merge".into(),
                pane_id: "wt-merge:p1".into(),
                path: path.display().to_string(),
                branch: "p/merge/between-voyages".into(),
            }
        }

        fn heads(&self) -> std::collections::BTreeMap<String, String> {
            branch_heads(&self.root)
        }
    }

    impl Drop for Trio {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// A project with the three tasks of a [`Trio`], all handed in.
    fn trio_world(candidate_a: &str, candidate_b: &str, candidate_c: &str) -> Store {
        let mut store = pair_world(candidate_a, candidate_b);
        store
            .write(|tx| {
                tx.insert_task(&gear_store::TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-3".into(),
                    rank: 3,
                    state: TaskState::Ready,
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
                    issue: None,
                    updated: 0,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-3", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-3", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-3", 1, 1, candidate_c, "report", 0)?;
                Ok(())
            })
            .unwrap();
        store
    }

    /// A third handed-in task, T-3, on its own branch of the scratch repo: its
    /// candidate is returned.
    fn add_third_task(store: &mut Store, pair: &Pair) -> String {
        let candidate = sh(
            &pair.work,
            "git switch -qc t3 dev && echo c > c.txt && git add c.txt \
             && git -c user.email=a@b -c user.name=a commit -qm c \
             && git -C ../origin.git fetch -q \"$(pwd)\" +HEAD:refs/heads/heat/t3 \
             && git rev-parse HEAD",
        )
        .trim()
        .to_string();
        store
            .write(|tx| {
                tx.insert_task(&gear_store::TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-3".into(),
                    rank: 3,
                    state: TaskState::Ready,
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
                    issue: None,
                    updated: 0,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-3", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-3", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-3", 1, 1, &candidate, "report", 0)?;
                Ok(())
            })
            .unwrap();
        candidate
    }

    /// T-576: a red batch of three sends its members back **queued**, not
    /// unknown (Prometheus 2026-11-05: O-7384..O-7390 sat `unknown` with "its
    /// batch failed the gate: it is gated again in a smaller batch", no later
    /// batch took them, the lost scan called them lost and `- remerge`
    /// refused them).
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_red_batch_leaves_its_members_queued_and_the_next_batch_takes_them() {
        let pair = Pair::new("batch-queued", false);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        let third = add_third_task(&mut store, &pair);
        // The gate says no to the whole batch.
        batch_of(&mut store, "false", 3);
        pass(&mut store, "T-1", &pair.a, 10);
        pass(&mut store, "T-2", &pair.b, 10);
        pass(&mut store, "T-3", &third, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        // One red batch, then collected: every member is queued again.
        let seen = cfg.merges.progress();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 30).unwrap();
        assert!(cfg.merges.running("p"), "the batch runs");
        assert!(cfg.merges.wait_progress(seen, WAIT), "the batch reports");
        merge_passed(&mut store, &mut herdr, &cfg, "p", 31).unwrap();

        let ids: Vec<String> = ["T-1", "T-2", "T-3"]
            .iter()
            .map(|task| {
                store
                    .read(|tx| Ok(tx.ops_in_state("p", OpState::Intended)?))
                    .unwrap()
                    .into_iter()
                    .find(|op| op.kind == OpKind::Merge && op.task.as_deref() == Some(task))
                    .unwrap_or_else(|| panic!("{task} is queued again"))
                    .id
            })
            .collect();
        for id in &ids {
            let op = store.read(|tx| tx.get_op("p", id)).unwrap().unwrap();
            assert_eq!(op.state, OpState::Intended, "{id} is queued, not unknown");
            assert!(
                op.result
                    .as_deref()
                    .is_some_and(|reason| reason.contains("gated again in a smaller batch")),
                "{id}: {:?}",
                op.result
            );
        }

        // The next round takes them: a passing gate lands all three.
        batch_of(&mut store, "true", 3);
        drive(&mut store, &mut herdr, &cfg, |store| {
            ["T-1", "T-2", "T-3"].iter().all(|task| {
                store
                    .read(|tx| {
                        Ok(tx
                            .ops_in_state("p", OpState::Confirmed)?
                            .into_iter()
                            .any(|op| op.kind == OpKind::Merge && op.task.as_deref() == Some(task)))
                    })
                    .unwrap()
            })
        });
        let log = pair.dev_log();
        for subject in ["a", "c"] {
            assert!(log_has(&log, subject), "{subject} landed: {log}");
        }
    }

    /// The project merges in batches of `size`, with `gate` as its gate
    /// (T-547).
    fn batch_of(store: &mut Store, gate: &str, size: usize) {
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text(gate).replace(
                    "[conventions]\n",
                    &format!("[conventions]\nmerge_batch = {size}\n"),
                );
                tx.update_project(&project)
            })
            .unwrap();
    }

    /// The project merges in batches of two, with `gate` as its gate (T-547).
    fn batch_of_two(store: &mut Store, gate: &str) {
        batch_of(store, gate, 2);
    }

    /// A batch merge (T-547): the two passed merges are merged into one
    /// worktree and gated once; both land together.
    #[test]
    fn two_merges_are_merged_together_and_gated_once() {
        let pair = Pair::new("batch", false);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        // The gate's own tally, in a file neither the worktree nor the remote
        // holds.
        let ran = pair.root.join("gate-runs");
        batch_of_two(&mut store, &format!("echo x >> {}; true", ran.display()));
        pass(&mut store, "T-1", &pair.a, 10);
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Confirmed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });

        let log = pair.dev_log();
        for subject in ["a", "b", "r1", "r2"] {
            assert!(log_has(&log, subject), "{subject} landed: {log}");
        }
        let runs = fs::read_to_string(&ran).unwrap_or_default();
        assert_eq!(runs.lines().count(), 1, "one gate for both: {runs}");
        // Each merge's line names its own commit, not the last one's (F2 of the
        // review): both are commits of dev and they differ.
        let line_of = |store: &mut Store, id: &str| {
            op_result(store, id)
                .lines()
                .next()
                .unwrap_or_default()
                .to_string()
        };
        let first = line_of(&mut store, "O-1");
        let second = line_of(&mut store, "O-2");
        assert_ne!(first, second, "each merge names its own commit");
        let heads = sh(&pair.root, "git -C origin.git log --format=%h dev");
        for line in [&first, &second] {
            // `merged <task> into <integration>: <sha> <subject>`
            let commit = line.split(": ").nth(1).unwrap_or_default();
            let sha = commit.split_whitespace().next().unwrap_or_default();
            assert!(
                heads.lines().any(|head| head == sha),
                "{line:?} names a commit of dev: {heads}"
            );
        }
    }

    /// T-555: a gate run is told which project it judges, which run it is (the
    /// id of its first merge op, `o1`) and which task it judges.
    #[test]
    fn a_single_merges_gate_names_its_project_round_and_task() {
        let pair = Pair::new("gate-vars", false);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        // The gate's own record, in a file outside the worktree.
        let got = pair.root.join("gate-run");
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text(&format!(
                    "echo $GEAR_GATE_PROJECT $GEAR_GATE_ROUND $GEAR_GATE_TASKS > {}; true",
                    got.display()
                ));
                tx.update_project(&project)
            })
            .unwrap();
        pass(&mut store, "T-1", &pair.a, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Confirmed)
        });

        let line = fs::read_to_string(&got).unwrap_or_default();
        assert_eq!(line.trim(), "p o1 T-1");
    }

    /// T-555: a batch's one gate names its first merge as the round and every
    /// merge it judges, in merge order.
    #[test]
    fn a_batchs_gate_names_the_round_and_every_task_it_judges() {
        let pair = Pair::new("gate-vars-batch", false);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        let got = pair.root.join("gate-run");
        batch_of_two(
            &mut store,
            &format!(
                "echo $GEAR_GATE_PROJECT $GEAR_GATE_ROUND $GEAR_GATE_TASKS > {}; true",
                got.display()
            ),
        );
        pass(&mut store, "T-1", &pair.a, 10);
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Confirmed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });

        let line = fs::read_to_string(&got).unwrap_or_default();
        assert_eq!(line.trim(), "p o1 T-1 T-2");
    }

    /// T-574: the captain's `- merge-first T-<n>` runs before every other ready
    /// merge — the batch they would form — and alone (Prometheus, 2026-11-05:
    /// T-738 held the fix every red round waited for, and its merge was last
    /// of fifteen).
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_merge_first_task_merges_before_the_batch() {
        let trio = Trio::new("merge-first");
        let worktree = trio.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let heads = trio.heads();
        let mut store = trio_world(&heads["heat/t1"], &heads["heat/t2"], &heads["heat/t3"]);
        // The gate writes whom it judged: one task when the merge runs alone.
        let ran = trio.root.join("gate-runs");
        batch_of(
            &mut store,
            &format!("printenv GEAR_GATE_TASKS >> {}; true", ran.display()),
            3,
        );
        for (task, branch) in [("T-1", "heat/t1"), ("T-2", "heat/t2"), ("T-3", "heat/t3")] {
            pass(&mut store, task, &heads[branch], 10);
        }
        // The captain puts the last of the three first.
        store
            .write(|tx| tx.set_meta(&gear_app::plan::merge_first_key("p"), "T-3"))
            .unwrap();
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        // The merge runs alone, then its report is collected on the next step.
        tick_step(&mut store, &mut herdr, &cfg, 30);
        tick_step(&mut store, &mut herdr, &cfg, 31);

        assert_eq!(op_state(&mut store, "O-3"), Some(OpState::Confirmed));
        assert_ne!(op_state(&mut store, "O-1"), Some(OpState::Confirmed));
        assert_ne!(op_state(&mut store, "O-2"), Some(OpState::Confirmed));
        let runs = fs::read_to_string(&ran).unwrap_or_default();
        assert_eq!(runs.lines().next(), Some("T-3"), "{runs}");
        // The line is one-shot: the mark is used.
        assert_eq!(
            store
                .read(|tx| tx.get_meta(&gear_app::plan::merge_first_key("p")))
                .unwrap()
                .unwrap_or_default(),
            "",
            "the mark is cleared when its merge starts"
        );

        // The other two land together afterwards.
        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Confirmed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });
        let runs = fs::read_to_string(&ran).unwrap_or_default();
        assert!(
            runs.lines()
                .any(|line| line.contains("T-1") && line.contains("T-2")),
            "the rest batch together: {runs}"
        );
    }

    /// T-547: a red batch is halved — its first merge runs alone and fails as
    /// it would, the other lands alone. Two gates after the red one.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_red_batch_is_halved_and_its_merges_run_alone() {
        let pair = Pair::new("batch-red", false);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        // The gate says no to T-1's work (a.txt) and yes to T-2's alone.
        batch_of_two(&mut store, "test ! -f a.txt");
        pass(&mut store, "T-1", &pair.a, 10);
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Failed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });

        let log = pair.dev_log();
        assert!(!log_has(&log, "a"), "T-1 did not land: {log}");
        assert!(log_has(&log, "b"), "T-2 landed: {log}");
        assert!(
            op_result(&mut store, "O-1").contains("gate failed"),
            "{}: {}",
            "the gate's own reason is kept",
            op_result(&mut store, "O-1")
        );
        // The red batch halved the cap to 1, where the batch path never runs;
        // T-2 landing alone lets it grow back, so the next merges batch again
        // (Prometheus, 2026-11-04: every merge ran alone after a red streak).
        let caps = cfg.merges.batch_cap.lock().unwrap().clone();
        assert!(
            !caps.is_empty() && caps.values().all(|cap| *cap == 2),
            "a merge that landed alone doubles the cap back to merge_batch: {caps:?}"
        );
    }

    /// F1 of the review of T-547: a batch whose push is rejected because the
    /// integration branch moved during its gate is a race, not a failure: both
    /// merges go back to the queue with the race reason and the next merge of
    /// this branch still batches (no `single`).
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn dev_moving_during_a_batch_gate_is_a_race() {
        let pair = Pair::new("batch-race", false);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        // The gate moves origin/dev under the batch: its push cannot be a
        // fast-forward.
        let racer = format!(
            "cd {} && git fetch -q origin && git switch -q dev && git reset -q --hard origin/dev \
             && echo x >> other.txt && git add other.txt \
             && git -c user.email=a@b -c user.name=a commit -qm racer \
             && git -C ../origin.git fetch -q $(pwd) +HEAD:refs/heads/dev",
            pair.work.display()
        );
        batch_of_two(&mut store, &racer);
        pass(&mut store, "T-1", &pair.a, 10);
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        // One batch: it is launched, it runs, and its push is rejected.
        let seen = cfg.merges.progress();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 30).unwrap();
        assert!(cfg.merges.running("p"), "the batch runs");
        assert!(cfg.merges.wait_progress(seen, WAIT), "the batch reports");
        // The next step collects it.
        merge_passed(&mut store, &mut herdr, &cfg, "p", 31).unwrap();

        for id in ["O-1", "O-2"] {
            // T-576: sent back is queued again (`intended`), not unknown: the
            // push's outcome is known (nothing landed).
            assert_eq!(
                op_state(&mut store, id),
                Some(OpState::Intended),
                "{id} is queued again"
            );
            let reason = op_result(&mut store, id);
            assert!(reason.starts_with(RACE), "{id}: {reason}");
        }
        assert!(
            cfg.merges.single.lock().unwrap().is_empty(),
            "a race does not leave the batch path"
        );
        let log = pair.dev_log();
        assert!(
            !log_has(&log, "a") && !log_has(&log, "b"),
            "nothing landed: {log}"
        );
    }

    /// T-548: the gate names a.txt in `$GEAR_GATE_FAILED`, and only T-1's
    /// changes hold it: T-1 fails with that line, T-2 merges again and lands
    /// alone. The gate runs twice in all.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_red_batch_names_the_culprit_the_gate_points_at() {
        let pair = Pair::new("batch-culprit", false);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        let ran = pair.root.join("gate-runs");
        batch_of_two(
            &mut store,
            &format!(
                "echo x >> {}; echo a.txt > $GEAR_GATE_FAILED; test ! -f a.txt",
                ran.display()
            ),
        );
        pass(&mut store, "T-1", &pair.a, 10);
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Failed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });

        let result = op_result(&mut store, "O-1");
        assert!(
            result.contains("match only T-1"),
            "the culprit is named: {result}"
        );
        // T-2 was requeued from a batch whose culprit is known: it is gated
        // again alone, not in a smaller batch of its own.
        let log = pair.dev_log();
        assert!(!log_has(&log, "a"), "T-1 did not land: {log}");
        assert!(log_has(&log, "b"), "T-2 landed: {log}");
        let runs = fs::read_to_string(&ran).unwrap_or_default();
        assert_eq!(
            runs.lines().count(),
            2,
            "the red batch, then T-2 alone: {runs}"
        );
    }

    /// T-548: the gate names a path no merge touched: nothing is a culprit, so
    /// the batch is halved as before and its merges are gated one by one.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_red_batch_naming_no_change_is_halved() {
        let pair = Pair::new("batch-nobody", false);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        let ran = pair.root.join("gate-runs");
        batch_of_two(
            &mut store,
            &format!(
                "echo x >> {}; echo nobody.txt > $GEAR_GATE_FAILED; test ! -f a.txt",
                ran.display()
            ),
        );
        pass(&mut store, "T-1", &pair.a, 10);
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Failed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });

        assert!(
            !op_result(&mut store, "O-1").contains("match only"),
            "no culprit is named: {}",
            op_result(&mut store, "O-1")
        );
        let log = pair.dev_log();
        assert!(!log_has(&log, "a"), "T-1 did not land: {log}");
        assert!(log_has(&log, "b"), "T-2 landed: {log}");
        // The red batch of two, then T-1 and T-2 one by one: the cap halved.
        let runs = fs::read_to_string(&ran).unwrap_or_default();
        assert_eq!(runs.lines().count(), 3, "the halving: {runs}");
    }

    /// T-548: the failed-paths file belongs to the run that wrote it. A file an
    /// earlier gate left in the worktree is removed before the gate runs, so
    /// its lines never name a culprit here.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_failed_paths_file_left_from_an_earlier_gate_is_not_read() {
        let pair = Pair::new("batch-leftover", false);
        let worktree = pair.merge_worktree();
        // An earlier run's leftovers, naming T-1's own file.
        fs::write(
            Path::new(&worktree.path).join(".git-gear-gate-failed"),
            "a.txt\n",
        )
        .unwrap();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        // The gate says no without naming any path.
        batch_of_two(&mut store, "test ! -f a.txt");
        pass(&mut store, "T-1", &pair.a, 10);
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Failed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });

        assert!(
            !op_result(&mut store, "O-1").contains("match only"),
            "the leftover is not read: {}",
            op_result(&mut store, "O-1")
        );
        let log = pair.dev_log();
        assert!(!log_has(&log, "a"), "T-1 did not land: {log}");
        assert!(log_has(&log, "b"), "T-2 landed: {log}");
    }
    /// T-547: a batch member whose merge conflicts with the integration branch
    /// steps aside on the base the merges before it left; the other lands.
    #[test]
    fn a_conflicting_batch_member_steps_aside_and_the_other_lands() {
        let pair = Pair::new("batch-conflict", false);
        // dev moves on the file T-1 adds: merging T-1 into it conflicts.
        sh(
            &pair.root,
            &format!(
                "set -e\n\\
                 git -C {work} checkout -q dev\n\\
                 printf 'dev\\n' > {work}/a.txt\n\\
                 git -C {work} add a.txt\n\\
                 git -C {work} commit -q -m dev-a\n\\
                 git -C {work} push -q origin dev\n",
                work = pair.work.display()
            ),
        );
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        batch_of_two(&mut store, "true");
        pass(&mut store, "T-1", &pair.a, 10);
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Failed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });

        let log = pair.dev_log();
        assert!(log_has(&log, "b"), "T-2 landed: {log}");
        assert!(!log_has(&log, "a"), "T-1 kept out by its conflict: {log}");
        assert!(
            op_result(&mut store, "O-1").contains("conflict"),
            "the conflict is named: {}",
            op_result(&mut store, "O-1")
        );
    }

    /// A merge train: two passed tasks gate side by side, the second on the
    /// commit the first lands, and both land in order.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn two_merges_gate_side_by_side_and_land_in_order() {
        let pair = Pair::new("train", false);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let second = second_stage(&pair, &mut herdr);
        let mut store = pair_world(&pair.a, &pair.b);
        train_of_two(&mut store, "true");
        pass(&mut store, "T-1", &pair.a, 10);
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Confirmed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });

        let log = pair.dev_log();
        for subject in ["a", "b", "r1", "r2"] {
            assert!(log_has(&log, subject), "{subject} landed: {log}");
        }
        // The second stage was gated on the first: what it gated is the tip.
        let tip = sh(&pair.root, "git -C origin.git rev-parse dev");
        let stage = sh(&second, "git rev-parse HEAD");
        assert_eq!(stage.trim(), tip.trim(), "the second stage landed as gated");
    }

    /// A merge whose gate says no fails as it would alone; the merge behind it
    /// was gated on top of it, so it goes back to the queue and lands alone.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_failed_stage_sends_the_merges_behind_it_back_to_the_queue() {
        let pair = Pair::new("train-fail", false);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        second_stage(&pair, &mut herdr);
        let mut store = pair_world(&pair.a, &pair.b);
        // The gate says no to T-1's work (a.txt) and yes to T-2's alone.
        train_of_two(&mut store, "test ! -f a.txt");
        pass(&mut store, "T-1", &pair.a, 10);
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Failed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });

        let log = pair.dev_log();
        assert!(!log_has(&log, "a"), "T-1 did not land: {log}");
        assert!(log_has(&log, "b"), "T-2 landed: {log}");
    }

    #[test]
    fn a_merge_that_carries_no_other_task_is_not_held() {
        let pair = Pair::new("independent", false);
        let worktree = pair.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pair_world(&pair.a, &pair.b);
        // T-2 passed; T-1 did not. T-2's commits carry none of T-1's work.
        pass(&mut store, "T-2", &pair.b, 10);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Confirmed)
        });

        let log = pair.dev_log();
        assert!(log_has(&log, "b"), "T-2 landed: {log}");
        assert!(!log_has(&log, "a"), "T-1 did not land: {log}");
        // The captain is not told about a wait that did not happen.
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", 30))
            .unwrap();
        assert!(inbox.is_empty(), "{inbox:?}");
    }

    /// A scratch repo with a chain of tasks (T-62, F2 of the review): each
    /// task's branch is cut from the one before, so T-2 carries T-1's work,
    /// T-3 carries T-2's, and so on.
    struct Chain {
        root: PathBuf,
        work: PathBuf,
        /// The candidates in chain order: index 0 is the base.
        candidates: Vec<String>,
    }

    impl Chain {
        fn new(name: &str, tasks: usize) -> Chain {
            let shape = match tasks {
                2 => Shape::Chain2,
                3 => Shape::Chain3,
                other => panic!("the chain fixture has no shape of {other} tasks"),
            };
            let root = scratch_root(name, shape);
            let work = root.join("work");
            // One listing for every head, not one git process each (T-127).
            let heads = branch_heads(&root);
            let candidates = (1..=tasks)
                .map(|n| heads[&format!("heat/t{n}")].clone())
                .collect();
            Chain {
                root,
                work,
                candidates,
            }
        }

        fn merge_worktree(&self) -> gear_herdr::Worktree {
            let path = self.root.join("merge");
            sh(
                &self.root,
                &format!(
                    "git -C {} worktree add -q -b p/merge/between-voyages {} origin/dev",
                    self.work.display(),
                    path.display()
                ),
            );
            gear_herdr::Worktree {
                workspace_id: "wt-merge".into(),
                pane_id: "wt-merge:p1".into(),
                path: path.display().to_string(),
                branch: "p/merge/between-voyages".into(),
            }
        }

        fn dev_log(&self) -> String {
            sh(&self.root, "git -C origin.git log --format=%s dev")
        }
    }

    impl Drop for Chain {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// A project with `candidates.len()` tasks, T-1..T-n, each handed in on
    /// its first attempt.
    fn chain_world(candidates: &[String]) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                // T-72: the fixture's own origin, as `init` would freeze it.
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
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "heat",
                    gear_model::SeatRole::Lane,
                    "w1:p1",
                    "opencode",
                    0,
                )?;
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    gear_model::SeatRole::Captain,
                    "w9:p1",
                    "claude",
                    0,
                )?;
                for (index, candidate) in candidates.iter().enumerate() {
                    let id = format!("T-{}", index + 1);
                    tx.insert_task(&gear_store::TaskRow {
                        repo: None,
                        base: None,
                        gate: None,
                        rework_estimate_min: None,
                        project: "p".into(),
                        id: id.clone(),
                        rank: index as i64 + 1,
                        state: TaskState::Ready,
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
                        issue: None,
                        updated: 0,
                    })?;
                    gear_app::tasks::start_attempt(tx, "p", &id, "p:heat", 1, 0)?;
                    gear_app::tasks::on_delivery_confirmed(tx, "p", &id, 1, 0)?;
                    gear_app::tasks::submit_result(tx, "p", &id, 1, 1, candidate, "report", 0)?;
                }
                Ok(())
            })
            .unwrap();
        store
    }

    /// T-62, F1 of the review: a task that reworked hands in a new candidate,
    /// and the task built on the older branch still carries the older,
    /// rejected work. The wait must look at every attempt, not only the last.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_merge_waits_for_an_earlier_attempt_of_the_task_it_carries() {
        let chain = Chain::new("rework", 2);
        // A's rework: one more commit on the same branch, outside B's history.
        sh(
            &chain.work,
            "git switch -q heat/t1 && echo rework > rework.txt && git add -A \
             && git -c user.email=a@b -c user.name=a commit -qm a2",
        );
        publish(&chain.work, "heat/t1");
        publish(&chain.work, "review/t1");
        let a1 = chain.candidates[0].clone();
        let b = chain.candidates[1].clone();
        let a2 = sh(&chain.root, "git -C origin.git rev-parse heat/t1");
        assert_ne!(a1, a2, "the rework is a new commit");
        // B holds A's first attempt, not its rework.
        assert!(
            sh_ok(
                &chain.work,
                &format!("git merge-base --is-ancestor {a1} {b}")
            ),
            "B was built on A's first attempt"
        );
        assert!(
            !sh_ok(
                &chain.work,
                &format!("git merge-base --is-ancestor {a2} {b}")
            ),
            "the rework is outside B's history"
        );

        let worktree = chain.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        // T-1: attempt 1 (a1) was rejected, attempt 2 (a2) is in review.
        let mut store = chain_world(&chain.candidates);
        store
            .write(|tx| {
                let mut first = tx.get_attempt("p", "T-1", 1)?.unwrap();
                first.state = gear_model::AttemptState::Rejected;
                tx.update_attempt(&first)?;
                let mut task = tx.get_task("p", "T-1")?.unwrap();
                task.state = TaskState::Ready;
                tx.update_task(&task)?;
                tx.insert_verdict(&gear_store::VerdictRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: a1.clone(),
                    passed: false,
                    evidence: Some("ops/reviews/T-1.md".into()),
                    by_seat: "p:captain".into(),
                    created: 5,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, 10)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 2, 10)?;
                gear_app::tasks::submit_result(tx, "p", "T-1", 2, 1, &a2, "report", 10)?;
                Ok(())
            })
            .unwrap();
        // T-2 passed; T-1 has not.
        pass(&mut store, "T-2", &b, 20);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        // T-2's merge waits: it carries T-1's first, rejected attempt.
        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Unknown)
        });
        let reason = op_result(&mut store, "O-1");
        assert!(reason.contains("waits for T-1"), "{reason}");
        assert_eq!(chain.dev_log(), "dev", "nothing landed yet");

        // T-1's rework passes: T-1 lands, then T-2.
        pass_n(&mut store, "T-1", 2, &a2, 40);
        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Confirmed)
                && op_state(store, "O-2") == Some(OpState::Confirmed)
        });

        let log = chain.dev_log();
        let b_merge = log.find("merge(T-2)").expect("T-2's merge");
        let a_merge = log.find("merge(T-1)").expect("T-1's merge");
        assert!(
            b_merge < a_merge,
            "T-1 lands before T-2 (newest first): {log}"
        );
        // The same log the order was read from: the rework's commit and T-2's
        // work are both on the integration branch (one git process, not two).
        assert!(log_has(&log, "a2"), "T-1's rework landed: {log}");
        assert!(log_has(&log, "t2"), "T-2's work landed: {log}");
    }

    /// T-62, F2 of the review: waiting merges take turns, so a chain lands in
    /// its own order even when the verdicts arrive last-task first, and even
    /// after a restart (every waiting op is Unknown then).
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn waiting_merges_take_turns_and_a_chain_lands_in_order() {
        // T-1 (the base) is carried by T-2, which is carried by T-3.
        let chain = Chain::new("chain", 3);
        let worktree = chain.merge_worktree();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = chain_world(&chain.candidates);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        // The verdicts arrive deepest first: T-3 (the task on top) passes, its
        // merge waits; then T-2, whose merge waits too; then T-1, the base, so
        // its merge can land. Every waiting op is Unknown by then.
        pass(&mut store, "T-3", &chain.candidates[2], 10);
        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Unknown)
        });
        pass(&mut store, "T-2", &chain.candidates[1], 20);
        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-2") == Some(OpState::Unknown)
        });
        pass(&mut store, "T-1", &chain.candidates[0], 30);
        // A restart finds the waiting ops Unknown, as a real one would.
        let recovered = crate::ops::recover_on_start(&mut store, 40).unwrap();
        assert!(
            recovered.is_empty(),
            "nothing was left executing: {recovered:?}"
        );

        // Only the fair order can finish the chain from here: T-1, then T-2,
        // then T-3.
        drive(&mut store, &mut herdr, &cfg, |store| {
            ["O-1", "O-2", "O-3"]
                .iter()
                .all(|id| op_state(store, id) == Some(OpState::Confirmed))
        });

        let log = chain.dev_log();
        assert!(
            log.contains("t1") && log.contains("t2") && log.contains("t3"),
            "{log}"
        );
        // Newest first: T-3's merge is on top, T-1's at the bottom.
        let t3 = log.find("merge(T-3)").expect("T-3's merge");
        let t2 = log.find("merge(T-2)").expect("T-2's merge");
        let t1 = log.find("merge(T-1)").expect("T-1's merge");
        assert!(t3 < t2 && t2 < t1, "the chain lands in order: {log}");
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
            // The fixture has one shape (T-53's child repo `web` on `web-dev`);
            // its copy carries the same relative remote as the template.
            assert_eq!(
                (name, integration),
                ("web", "web-dev"),
                "the child fixture's one shape"
            );
            copy_tree(template(Shape::ChildWeb), root);
            Child {
                root: root.to_path_buf(),
                work: root.join("web"),
                origin: root.join("web-origin.git"),
            }
        }

        /// The merge worktree of this repo: `p/merge/between-voyages`, cut from its
        /// integration branch.
        fn merge_worktree(&self, integration: &str) -> gear_herdr::Worktree {
            let path = self.root.join("web-merge");
            sh(
                &self.root,
                &format!(
                    "git -C {} worktree add -q -b p/merge/between-voyages {} origin/{integration}",
                    self.work.display(),
                    path.display()
                ),
            );
            gear_herdr::Worktree {
                workspace_id: "wt-merge-web".into(),
                pane_id: "wt-merge-web:p1".into(),
                path: path.display().to_string(),
                branch: "p/merge/between-voyages".into(),
            }
        }

        /// The head of a branch on this repo's origin.
        fn head_of(&self, branch: &str) -> String {
            sh(
                &self.root,
                &format!("git -C {} rev-parse {branch}", self.origin.display()),
            )
        }

        /// The commits on a branch of this repo's origin.
        fn log_of(&self, branch: &str) -> String {
            sh(
                &self.root,
                &format!("git -C {} log --format=%s {branch}", self.origin.display()),
            )
        }
    }

    fn sh(dir: &Path, command: &str) -> String {
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir)
            .output()
            .unwrap();
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

    /// The test project's config with `repo` pointing at a checkout that is
    /// really there (T-397): the ancestor check asks git.
    fn config_text_in(gate: &str, repo: &Path) -> String {
        config_text(gate).replace(
            "repo = \"/repo\"",
            &format!("repo = \"{}\"", repo.display()),
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
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "heat",
                    gear_model::SeatRole::Lane,
                    "w1:p1",
                    "opencode",
                    0,
                )?;
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    gear_model::SeatRole::Captain,
                    "w9:p1",
                    "claude",
                    0,
                )?;
                tx.insert_task(&gear_store::TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
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
                    issue: None,
                    updated: 0,
                })?;
                // T-72: the project's frozen destinations name the fixture's
                // origin, so the merge's own push passes the allow-list the way
                // a real project's does.
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

    /// The branch sweep: a cancelled task's branches are deleted on the
    /// remote, a branch older than `branch_max_age_min` is named to the
    /// captain once per limit, and a young branch is left alone.
    #[test]
    fn the_branch_sweep_deletes_cancelled_branches_and_names_old_ones() {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let work = root.path().join("work");
        sh(
            root.path(),
            &format!("git init -q --bare {}", origin.display()),
        );
        sh(
            root.path(),
            &format!("git init -q -b dev {}", work.display()),
        );
        sh(
            &work,
            "git -c user.email=t@t -c user.name=t commit -q --allow-empty -m base",
        );
        sh(
            &work,
            &format!("git remote add origin {}", origin.display()),
        );
        for branch in ["dev", "heat/t5", "review/t5", "heat/t6", "heat/t7"] {
            sh(
                &work,
                &format!("git push -q origin HEAD:refs/heads/{branch}"),
            );
        }
        // T-5 is cancelled; T-6 started three hours ago; T-7 ten minutes ago.
        let mut store = world(TaskState::Cancelled);
        store
            .write(|tx| {
                for (id, started) in [("T-6", 0), ("T-7", 3 * 3600 - 600)] {
                    let mut task = tx.get_task("p", "T-5")?.unwrap();
                    task.id = id.into();
                    task.state = TaskState::Ready;
                    tx.insert_task(&task)?;
                    gear_app::tasks::start_attempt(tx, "p", id, "p:heat", 1, started)?;
                    // Sent back: the seat is free for the next task.
                    let mut attempt = tx.get_attempt("p", id, 1)?.unwrap();
                    attempt.state = gear_model::AttemptState::Rejected;
                    tx.update_attempt(&attempt)?;
                }
                Ok(())
            })
            .unwrap();
        let config = gear_app::config::parse(&config_text_in("true", &work)).unwrap();
        let tasks = store.read(|tx| tx.tasks_of_project("p")).unwrap();
        let repo = config.repo_of_task(&tasks[0]);
        let now = 3 * 3600;
        let heads = git(&work, &["ls-remote", "--heads", "origin"]).unwrap();
        let found = task_branches(&mut store, "p", &config, &tasks, &repo, &heads).unwrap();
        let remote = Target {
            name: "origin",
            runner: &GitRemote,
        };
        let dev = git(&work, &["rev-parse", "dev"]).unwrap();
        sweep_in(
            &mut store,
            "p",
            &config,
            &work,
            remote,
            found,
            now,
            "dev",
            Some(dev.trim()),
        )
        .unwrap();

        let left = git(&work, &["ls-remote", "--heads", "origin"]).unwrap();
        assert!(
            !left.contains("heat/t5") && !left.contains("review/t5"),
            "cancelled branches deleted: {left}"
        );
        assert!(
            left.contains("heat/t6") && left.contains("heat/t7") && left.contains("refs/heads/dev"),
            "{left}"
        );
        let items = store
            .read(|tx| tx.actionable_for("p", "p:captain", now))
            .unwrap();
        let body: Vec<&str> = items.iter().map(|m| m.body.as_str()).collect();
        assert_eq!(items.len(), 1, "{body:?}");
        assert!(items[0].urgent, "an old branch is urgent");
        assert!(
            body[0].contains("heat/t6 (T-6") && body[0].contains("open 180 min"),
            "{}",
            body[0]
        );
        assert!(
            !body[0].contains("heat/t7"),
            "a young branch is not named: {}",
            body[0]
        );
        assert!(
            body[0].contains("heat/t5 at") && body[0].contains("review/t5 at"),
            "{}",
            body[0]
        );

        // Ten minutes later the same old branch is not named again.
        let heads = git(&work, &["ls-remote", "--heads", "origin"]).unwrap();
        let found = task_branches(&mut store, "p", &config, &tasks, &repo, &heads).unwrap();
        let dev = git(&work, &["rev-parse", "dev"]).unwrap();
        sweep_in(
            &mut store,
            "p",
            &config,
            &work,
            remote,
            found,
            now + 600,
            "dev",
            Some(dev.trim()),
        )
        .unwrap();
        let items = store
            .read(|tx| tx.actionable_for("p", "p:captain", now + 600))
            .unwrap();
        assert_eq!(items.len(), 1, "told once per limit");
    }

    /// T-474: a task moved to another lane leaves its old lane's task branch
    /// behind (another project's T-199: `api/t199`, fully an ancestor of dev).
    /// When the task is done, every branch its template gives for any lane that
    /// held it is deleted when it is on the integration branch; one that still
    /// holds commits the integration branch does not have is kept and named to
    /// the captain with them.
    #[test]
    fn the_branch_sweep_cleans_a_done_task_moved_between_lanes() {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let work = root.path().join("work");
        sh(
            root.path(),
            &format!("git init -q --bare {}", origin.display()),
        );
        sh(
            root.path(),
            &format!("git init -q -b dev {}", work.display()),
        );
        sh(
            &work,
            "git -c user.email=t@t -c user.name=t commit -q --allow-empty -m base",
        );
        sh(
            &work,
            &format!("git remote add origin {}", origin.display()),
        );
        for branch in ["dev", "api/t8"] {
            sh(
                &work,
                &format!("git push -q origin HEAD:refs/heads/{branch}"),
            );
        }
        // The lane it moved to left work the integration branch does not have.
        sh(
            &work,
            "git checkout -q -b heat/t8 && git -c user.email=t@t -c user.name=t \
             commit -q --allow-empty -m extra && git push -q origin HEAD:refs/heads/heat/t8",
        );
        sh(&work, "git checkout -q dev");
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.id = "T-8".into();
                task.rank = 8;
                task.state = TaskState::Done;
                task.lane = Some("heat".into());
                task.milestone = Some("M5".into());
                tx.insert_task(&task)?;
                // First held by `api`, then moved to `heat`.
                for (n, seat) in [(1, "p:api"), (2, "p:heat")] {
                    tx.insert_attempt(&gear_store::AttemptRow {
                        project: "p".into(),
                        task: "T-8".into(),
                        n,
                        seat: seat.into(),
                        generation: 1,
                        contract_version: 1,
                        adopted_version: 1,
                        pending_update: None,
                        state: gear_model::AttemptState::Accepted,
                        candidate: None,
                        report: None,
                        safe_point: None,
                        started: 0,
                        updated: 0,
                        session_ref: None,
                        session_cli: None,
                        session_action: None,
                        sent_at: None,
                    })?;
                }
                Ok(())
            })
            .unwrap();
        let config = gear_app::config::parse(&config_text_in("true", &work)).unwrap();
        let tasks = store.read(|tx| tx.tasks_of_project("p")).unwrap();
        let repo = config.repo_of_task(&tasks[1]);
        let now = 3 * 3600;
        let heads = git(&work, &["ls-remote", "--heads", "origin"]).unwrap();
        let found = task_branches(&mut store, "p", &config, &tasks, &repo, &heads).unwrap();
        let dev = git(&work, &["rev-parse", "dev"]).unwrap();
        let remote = Target {
            name: "origin",
            runner: &GitRemote,
        };
        sweep_in(
            &mut store,
            "p",
            &config,
            &work,
            remote,
            found,
            now,
            "dev",
            Some(dev.trim()),
        )
        .unwrap();

        let branches = git(&work, &["ls-remote", "--heads", "origin"]).unwrap();
        assert!(
            !branches.contains("api/t8"),
            "the old lane's merged branch is deleted: {branches}"
        );
        assert!(
            branches.contains("heat/t8"),
            "the branch holding work is kept: {branches}"
        );
        let items = store
            .read(|tx| tx.actionable_for("p", "p:captain", now))
            .unwrap();
        let body: Vec<&str> = items.iter().map(|m| m.body.as_str()).collect();
        assert_eq!(items.len(), 1, "{body:?}");
        assert!(
            body[0].contains("heat/t8") && body[0].contains("extra") && body[0].contains("T-8"),
            "{}",
            body[0]
        );
    }

    /// A done task carried by another task that has not merged yet (Lawrence
    /// T-31 / M-603, 2026-11-03): its branch holds the work in flight, so it is
    /// kept and not named to the captain.
    #[test]
    fn the_branch_sweep_leaves_a_carried_task_waiting_for_its_carrier() {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let work = root.path().join("work");
        sh(
            root.path(),
            &format!("git init -q --bare {}", origin.display()),
        );
        sh(
            root.path(),
            &format!("git init -q -b dev {}", work.display()),
        );
        sh(
            &work,
            "git -c user.email=t@t -c user.name=t commit -q --allow-empty -m base",
        );
        sh(
            &work,
            &format!("git remote add origin {}", origin.display()),
        );
        for branch in ["dev", "api/t8"] {
            sh(
                &work,
                &format!("git push -q origin HEAD:refs/heads/{branch}"),
            );
        }
        // The lane it moved to left work the integration branch does not have.
        sh(
            &work,
            "git checkout -q -b heat/t8 && git -c user.email=t@t -c user.name=t \
             commit -q --allow-empty -m extra && git push -q origin HEAD:refs/heads/heat/t8",
        );
        sh(&work, "git checkout -q dev");
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.id = "T-8".into();
                task.rank = 8;
                task.state = TaskState::Done;
                task.lane = Some("heat".into());
                task.milestone = Some("M5".into());
                tx.insert_task(&task)?;
                // First held by `api`, then moved to `heat`.
                for (n, seat) in [(1, "p:api"), (2, "p:heat")] {
                    tx.insert_attempt(&gear_store::AttemptRow {
                        project: "p".into(),
                        task: "T-8".into(),
                        n,
                        seat: seat.into(),
                        generation: 1,
                        contract_version: 1,
                        adopted_version: 1,
                        pending_update: None,
                        state: gear_model::AttemptState::Accepted,
                        candidate: None,
                        report: None,
                        safe_point: None,
                        started: 0,
                        updated: 0,
                        session_ref: None,
                        session_cli: None,
                        session_action: None,
                        sent_at: None,
                    })?;
                }
                tx.set_task_carried_by("p", "T-8", "other:T-9")
            })
            .unwrap();
        let config = gear_app::config::parse(&config_text_in("true", &work)).unwrap();
        let tasks = store.read(|tx| tx.tasks_of_project("p")).unwrap();
        let repo = config.repo_of_task(&tasks[1]);
        let now = 3 * 3600;
        let heads = git(&work, &["ls-remote", "--heads", "origin"]).unwrap();
        let found = task_branches(&mut store, "p", &config, &tasks, &repo, &heads).unwrap();
        let dev = git(&work, &["rev-parse", "dev"]).unwrap();
        let remote = Target {
            name: "origin",
            runner: &GitRemote,
        };
        sweep_in(
            &mut store,
            "p",
            &config,
            &work,
            remote,
            found,
            now,
            "dev",
            Some(dev.trim()),
        )
        .unwrap();

        let branches = git(&work, &["ls-remote", "--heads", "origin"]).unwrap();
        assert!(
            !branches.contains("api/t8"),
            "the old lane's merged branch is deleted: {branches}"
        );
        assert!(
            branches.contains("heat/t8"),
            "the branch holding work is kept: {branches}"
        );
        let items = store
            .read(|tx| tx.actionable_for("p", "p:captain", now))
            .unwrap();
        let branches = git(&work, &["ls-remote", "--heads", "origin"]).unwrap();
        assert!(
            branches.contains("heat/t8"),
            "the carried work is kept: {branches}"
        );
        assert!(
            !items.iter().any(|m| m.body.contains("heat/t8")),
            "{items:?}"
        );
    }

    /// The branch sweep counts the open voyage's task branches: more than
    /// lanes × `branch_max_per_lane` (1 × 3 here) is named to the captain,
    /// review branches and other voyages' branches are not counted.
    #[test]
    fn the_branch_sweep_names_too_many_open_branches() {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let work = root.path().join("work");
        sh(
            root.path(),
            &format!("git init -q --bare {}", origin.display()),
        );
        sh(
            root.path(),
            &format!("git init -q -b dev {}", work.display()),
        );
        sh(
            &work,
            "git -c user.email=t@t -c user.name=t commit -q --allow-empty -m base",
        );
        sh(
            &work,
            &format!("git remote add origin {}", origin.display()),
        );
        let now = 3600;
        let mut store = world(TaskState::Done);
        store
            .write(|tx| {
                for (id, milestone) in [("T-6", "M1"), ("T-7", "M1"), ("T-8", "M1"), ("T-9", "M0")]
                {
                    let mut task = tx.get_task("p", "T-5")?.unwrap();
                    task.id = id.into();
                    task.state = TaskState::Ready;
                    task.milestone = Some(milestone.into());
                    tx.insert_task(&task)?;
                    gear_app::tasks::start_attempt(tx, "p", id, "p:heat", 1, now - 60)?;
                    let mut attempt = tx.get_attempt("p", id, 1)?.unwrap();
                    attempt.state = gear_model::AttemptState::Rejected;
                    tx.update_attempt(&attempt)?;
                }
                Ok(())
            })
            .unwrap();
        for branch in [
            "heat/t6",
            "heat/t7",
            "heat/t8",
            "review/t6",
            "review/t7",
            "heat/t9",
        ] {
            sh(
                &work,
                &format!("git push -q origin HEAD:refs/heads/{branch}"),
            );
        }
        let config = gear_app::config::parse(&config_text_in("true", &work)).unwrap();
        let tasks = store.read(|tx| tx.tasks_of_project("p")).unwrap();
        let repo = config.repo_of_task(&tasks[0]);
        let remote = Target {
            name: "origin",
            runner: &GitRemote,
        };
        let sweep = |store: &mut Store, at: Ts| {
            let heads = git(&work, &["ls-remote", "--heads", "origin"]).unwrap();
            let found = task_branches(store, "p", &config, &tasks, &repo, &heads).unwrap();
            let dev = git(&work, &["rev-parse", "dev"]).unwrap();
            sweep_in(
                store,
                "p",
                &config,
                &work,
                remote,
                found,
                at,
                "dev",
                Some(dev.trim()),
            )
            .unwrap();
        };
        // Three task branches of M1 (and M0's T-9, also open): four, over 3.
        sweep(&mut store, now);
        let items = store
            .read(|tx| tx.actionable_for("p", "p:captain", now))
            .unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        let body = &items[0].body;
        assert!(
            body.contains("4 task branches") && body.contains("limit of 3"),
            "{body}"
        );
        assert!(
            !body.contains("review/t6"),
            "review branches are not counted: {body}"
        );
        // The same count a minute later is not told again.
        sweep(&mut store, now + 60);
        assert_eq!(
            store
                .read(|tx| tx.actionable_for("p", "p:captain", now + 60))
                .unwrap()
                .len(),
            1
        );
    }

    /// T-568: a sweep's report replaces the one the last sweep wrote — its
    /// thread is named after the tick, so the captain ended up with 123 open
    /// `branches:` items and doorbells for the old ones.
    #[test]
    fn three_branch_sweeps_leave_one_open_item() {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let work = root.path().join("work");
        sh(
            root.path(),
            &format!("git init -q --bare {}", origin.display()),
        );
        sh(
            root.path(),
            &format!("git init -q -b dev {}", work.display()),
        );
        sh(
            &work,
            "git -c user.email=t@t -c user.name=t commit -q --allow-empty -m base",
        );
        sh(
            &work,
            &format!("git remote add origin {}", origin.display()),
        );
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config_text_in("true", &work);
                tx.update_project(&project)
            })
            .unwrap();
        let config = gear_app::config::parse(&config_text_in("true", &work)).unwrap();
        let remote = Target {
            name: "origin",
            runner: &GitRemote,
        };
        let dev = git(&work, &["rev-parse", "dev"]).unwrap();
        let sweep = |store: &mut Store, name: &str, at: Ts| {
            let found = vec![TaskBranch {
                name: name.into(),
                sha: dev.trim().into(),
                task: "T-5".into(),
                state: TaskState::Ready,
                since: Some(0),
                review: false,
                open_voyage: true,
            }];
            sweep_in(
                store,
                "p",
                &config,
                &work,
                remote,
                found,
                at,
                "dev",
                Some(dev.trim()),
            )
            .unwrap();
        };
        sweep(&mut store, "heat/t6", 1_000_000);
        sweep(&mut store, "heat/t7", 2_000_000);
        sweep(&mut store, "heat/t8", 3_000_000);

        let items = store
            .read(|tx| tx.messages_in_threads_starting("p", "branches:"))
            .unwrap();
        assert_eq!(items.len(), 3, "{items:?}");
        let open: Vec<&gear_store::MessageRow> = items
            .iter()
            .filter(|item| item.state == MsgState::Open)
            .collect();
        assert_eq!(open.len(), 1, "one open sweep item: {items:?}");
        assert_eq!(open[0].thread, "branches:3000000");
        // Each one says which replaced it; the last one is the open item.
        for pair in items.windows(2) {
            assert_eq!(pair[0].state, MsgState::Resolved, "{:?}", pair[0]);
            assert_eq!(
                pair[0].result.as_deref(),
                Some(format!("superseded by {}", pair[1].id).as_str()),
                "the item says which one replaced it"
            );
        }
        assert_eq!(items[items.len() - 1].id, open[0].id);
    }

    /// T-553: a done task whose merge waits for another task sits in
    /// `Unknown` (`Report::Waiting` leaves it there). That merge is landing,
    /// not lost: the sweep names no unlanded branch for it (2026-11-04: the
    /// captain was told "nothing will merge them" for T-548 while its merge
    /// queued behind T-547).
    #[test]
    fn a_done_task_whose_merge_waits_for_another_is_not_named_unlanded() {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let work = root.path().join("work");
        sh(
            root.path(),
            &format!("git init -q --bare {}", origin.display()),
        );
        sh(
            root.path(),
            &format!("git init -q -b dev {}", work.display()),
        );
        sh(
            &work,
            "git -c user.email=t@t -c user.name=t commit -q --allow-empty -m base",
        );
        sh(
            &work,
            &format!("git remote add origin {}", origin.display()),
        );
        // A commit of T-5 that dev does not have.
        sh(
            &work,
            "git switch -q -c heat/t5 && echo a > a.txt && git add -A \
             && git -c user.email=t@t -c user.name=t commit -qm a",
        );
        sh(&work, "git push -q origin HEAD:refs/heads/heat/t5");
        let now = 3600;
        let config = gear_app::config::parse(&config_text_in("true", &work)).unwrap();
        let sweep = |store: &mut Store| {
            let tasks = store.read(|tx| tx.tasks_of_project("p")).unwrap();
            let repo = config.repo_of_task(&tasks[0]);
            let remote = Target {
                name: "origin",
                runner: &GitRemote,
            };
            let heads = git(&work, &["ls-remote", "--heads", "origin"]).unwrap();
            let found = task_branches(store, "p", &config, &tasks, &repo, &heads).unwrap();
            let dev = git(&work, &["rev-parse", "dev"]).unwrap();
            sweep_in(
                store,
                "p",
                &config,
                &work,
                remote,
                found,
                now,
                "dev",
                Some(dev.trim()),
            )
            .unwrap();
            store
                .read(|tx| tx.actionable_for("p", "p:captain", now))
                .unwrap()
        };
        let named = |items: &[gear_store::MessageRow]| {
            items
                .iter()
                .any(|item| item.body.contains("nothing will merge them"))
        };

        // Without a merge op the branch is news: the captain is told.
        let mut store = world(TaskState::Done);
        let items = sweep(&mut store);
        assert!(named(&items), "{items:?}");

        // A merge of T-5 that waits for another task is landing.
        let mut store = world(TaskState::Done);
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
                    payload: serde_json::json!({}),
                    created: now,
                })?;
                tx.set_op_state("p", "O-1", OpState::Unknown, Some("waits for T-4"), now)
            })
            .unwrap();
        let items = sweep(&mut store);
        assert!(!named(&items), "{items:?}");
    }

    /// The project config of a repo in pull-request mode (design §11.1).
    fn pr_config_text(gate: &str) -> String {
        format!(
            r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[github]
repo = "owner/name"
merge = "pr"
merge_method = "squash"

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

    /// A project in pull-request mode with one passed task (T-5) whose
    /// candidate is `candidate` (design §11.2).
    fn pr_world(candidate: &str) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&gear_store::ProjectRow {
                    id: "p".into(),
                    herdr_session: "sess".into(),
                    config_toml: pr_config_text("true"),
                    config_version: 1,
                    quota_lanes: 1,
                    quota_temps: 0,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: 0,
                })?;
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "heat",
                    gear_model::SeatRole::Lane,
                    "w1:p1",
                    "opencode",
                    0,
                )?;
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    gear_model::SeatRole::Captain,
                    "w9:p1",
                    "claude",
                    0,
                )?;
                tx.insert_task(&gear_store::TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-5".into(),
                    rank: 1,
                    state: TaskState::Ready,
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
                    issue: Some(3),
                    updated: 0,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-5", 1, 1, candidate, "report", 0)?;
                tx.insert_verdict(&gear_store::VerdictRow {
                    project: "p".into(),
                    task: "T-5".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: candidate.into(),
                    passed: true,
                    evidence: Some("ops/reviews/T-5.md".into()),
                    by_seat: "p:captain".into(),
                    created: 10,
                })?;
                Ok(())
            })
            .unwrap();
        store
    }

    /// The claim op of a task: the forge op that opened its pull request.
    fn claim_op(store: &mut Store, task: &str, number: u64) {
        let op = store
            .write(|tx| {
                tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:forge".into(),
                    generation: 0,
                    kind: OpKind::Forge,
                    task: Some(task.into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({
                        "forge": "claim",
                        "key": format!("{task}:claim"),
                        "attempts": 1,
                    }),
                    created: 0,
                })
            })
            .unwrap();
        let result = serde_json::json!({
            "repo": "owner/name",
            "number": number,
            "url": format!("https://github.com/owner/name/pull/{number}"),
        })
        .to_string();
        store
            .write(|tx| tx.set_op_state("p", &op.id, OpState::Confirmed, Some(&result), 1))
            .unwrap();
    }

    /// A forge that merges the pull request (design §11.2 step 4), or refuses
    /// it with the message `gh` gave. It counts its merge calls: a second
    /// call is the retry a `- remerge` line asks for.
    struct FakeForge {
        refuse: Option<String>,
        commit: String,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeForge {
        fn merging() -> FakeForge {
            FakeForge {
                refuse: None,
                commit: "cafe1234".to_string(),
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn refusing(reason: &str) -> FakeForge {
            FakeForge {
                refuse: Some(reason.to_string()),
                commit: "cafe1234".to_string(),
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        /// How many times the forge was asked to merge.
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl gear_forge::Forge for FakeForge {
        fn issues(
            &self,
            _repo: &str,
            _filter: &gear_forge::Filter,
        ) -> gear_forge::ForgeResult<Vec<gear_forge::Issue>> {
            Ok(Vec::new())
        }

        fn open_claim(
            &self,
            repo: &str,
            _branch: &str,
            _base: &str,
            _title: &str,
            _body: &str,
        ) -> gear_forge::ForgeResult<gear_forge::PrRef> {
            Ok(gear_forge::PrRef {
                repo: repo.to_string(),
                number: 1,
                url: String::new(),
            })
        }

        fn comment(&self, _pr: &gear_forge::PrRef, _body: &str) -> gear_forge::ForgeResult<()> {
            Ok(())
        }

        fn merge(
            &self,
            _pr: &gear_forge::PrRef,
            _method: gear_forge::MergeMethod,
            _head: &str,
        ) -> gear_forge::ForgeResult<gear_forge::MergeResult> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match &self.refuse {
                Some(reason) => Err(gear_forge::Error::Failed(reason.clone())),
                None => Ok(gear_forge::MergeResult {
                    merged: true,
                    message: "merged".to_string(),
                }),
            }
        }

        fn pr_state(
            &self,
            _pr: &gear_forge::PrRef,
        ) -> gear_forge::ForgeResult<gear_forge::PrState> {
            Ok(gear_forge::PrState {
                state: "MERGED".to_string(),
                merge_commit: Some(self.commit.clone()),
                head_ref_oid: None,
            })
        }

        fn pr_by_head(
            &self,
            _repo: &str,
            _branch: &str,
        ) -> gear_forge::ForgeResult<Option<gear_forge::PrRef>> {
            Ok(None)
        }

        fn close_issue(
            &self,
            _repo: &str,
            _number: u64,
            _comment: &str,
        ) -> gear_forge::ForgeResult<()> {
            Ok(())
        }
    }

    /// One merge step and the wait for the worker it launched: the step that
    /// collects the worker is the next one (design §11.2).
    fn pr_step(
        store: &mut Store,
        herdr: &mut gear_herdr::fake::FakeHerdr,
        cfg: &EngineCfg,
        now: Ts,
    ) -> Vec<Merged> {
        let seen = cfg.merges.progress();
        let merged = merge_passed(store, herdr, cfg, "p", now).unwrap();
        if cfg.merges.running("p") {
            assert!(
                cfg.merges.wait_progress(seen, WAIT),
                "the merge worker did not report within {WAIT:?}"
            );
        }
        merged
    }

    /// The forge ops of a project, by their key.
    fn forge_ops(store: &mut Store) -> Vec<gear_store::OpRow> {
        store
            .read(|tx| tx.ops_of_seat_kind("p:forge", OpKind::Forge))
            .unwrap()
    }

    /// The merge op of a task (the ids are the store's, so the op is found by
    /// what it is).
    fn merge_op(store: &mut Store) -> gear_store::OpRow {
        let mut all = Vec::new();
        for state in [
            OpState::Intended,
            OpState::Executing,
            OpState::Submitted,
            OpState::Unknown,
            OpState::Confirmed,
            OpState::Failed,
            OpState::Void,
        ] {
            all.extend(
                store
                    .read(|tx| tx.ops_in_state("p", state))
                    .unwrap()
                    .into_iter()
                    .filter(|op| op.kind == OpKind::Merge),
            );
        }
        all.pop().expect("a merge op")
    }

    /// The land op of a task: the forge op that merges its pull request.
    fn land_op(store: &mut Store, task: &str) -> Option<gear_store::OpRow> {
        forge_ops(store)
            .into_iter()
            .find(|op| op.payload.get("forge").and_then(|forge| forge.as_str()) == Some("land"))
            .filter(|op| op.task.as_deref() == Some(task))
    }

    /// The captain's items about a task's failed merge.
    fn failed_items(store: &mut Store, task: &str) -> Vec<gear_store::MessageRow> {
        store
            .read(|tx| tx.thread_messages("p", &format!("merge-failed:{task}")))
            .unwrap()
    }

    /// A pull-request merge (design §11.2): the gate runs as today, the
    /// review's commits go onto the pull request's head, the merge op waits
    /// for the land op, and the land op's answer is the merge.
    #[test]
    fn a_pr_mode_merge_pushes_the_pull_request_head_and_waits_for_the_land_op() {
        let scratch = Scratch::new("pr");
        let (worktree, candidate) = scratch.prepared();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pr_world(&candidate);
        claim_op(&mut store, "T-5", 7);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        // The engine queues the merge op, runs it, and collects the result.
        pr_step(&mut store, &mut herdr, &cfg, 20);
        pr_step(&mut store, &mut herdr, &cfg, 21);

        // The merge op waits for the land op, with the pull request and the
        // head the gate checked (design §11.2 step 3).
        let op = merge_op(&mut store);
        assert_eq!(op.state, OpState::Submitted, "{op:?}");
        let result: serde_json::Value =
            serde_json::from_str(op.result.as_deref().unwrap_or("{}")).unwrap();
        assert_eq!(result["mode"], "pr", "{result}");
        assert_eq!(result["pr"], 7, "{result}");
        let head = result["head"].as_str().unwrap_or_default().to_string();
        assert_eq!(head.len(), 40, "the head is a commit: {result}");

        // The land op is the forge's call, with the head and the method.
        let land = land_op(&mut store, "T-5").expect("the land op is inserted");
        assert_eq!(land.state, OpState::Intended, "{land:?}");
        assert_eq!(land.payload["forge"], "land");
        assert_eq!(land.payload["pr"], 7, "{land:?}");
        assert_eq!(land.payload["head"], head.as_str(), "{land:?}");
        assert_eq!(land.payload["method"], "squash", "{land:?}");

        // Only the pull request's head moved: the integration branch is
        // untouched, and the head holds the candidate and the review.
        assert!(
            !scratch.dev_has_task_commit(),
            "the integration branch is not pushed"
        );
        assert_eq!(
            scratch.head_of("heat/t5"),
            head,
            "the head is on the task's branch"
        );
        let log = sh(&scratch.root, "git -C origin.git log --format=%s heat/t5");
        assert!(log.contains("task"), "{log}");
        assert!(
            log.contains("review"),
            "the review's commits are on the head: {log}"
        );
        // Nothing is deleted yet (design §11.2 step 3).
        let branches = scratch.origin_branches();
        assert!(branches.contains("heat/t5"), "{branches}");
        assert!(branches.contains("review/t5"), "{branches}");

        // The forge merges the pull request (design §11.2 step 4).
        gear_forge::sync_with(&mut store, "p", &FakeForge::merging(), 30).unwrap();
        let land = land_op(&mut store, "T-5").expect("the land op is there");
        assert_eq!(land.state, OpState::Confirmed, "{land:?}");

        // The next tick confirms the merge with the pull request's commit
        // (design §11.2 step 5).
        let merged = pr_step(&mut store, &mut herdr, &cfg, 40);
        assert_eq!(merged.len(), 1, "{merged:?}");
        let op = merge_op(&mut store);
        assert_eq!(op.state, OpState::Confirmed, "{op:?}");
        let line = op.result.clone().unwrap_or_default();
        assert!(line.contains("merged T-5 into dev by PR #7"), "{line}");
        assert!(line.contains("cafe1234"), "{line}");
        assert_eq!(
            store
                .read(|tx| tx.attempt_times("p", "T-5", 1))
                .unwrap()
                .merged,
            Some(op.updated)
        );
        // The branches the merge consumed are gone now.
        let branches = scratch.origin_branches();
        assert!(!branches.contains("heat/t5"), "{branches}");
        assert!(!branches.contains("review/t5"), "{branches}");
    }

    /// §11.3, row 1: a task whose claim pull request was never opened cannot
    /// land through a pull request: the merge fails and says so.
    #[test]
    fn no_claim_pull_request_fails_the_merge_with_the_text() {
        let scratch = Scratch::new("pr-nopr");
        let (worktree, candidate) = scratch.prepared();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pr_world(&candidate);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        pr_step(&mut store, &mut herdr, &cfg, 20);

        let op = merge_op(&mut store);
        assert_eq!(op.state, OpState::Failed, "{op:?}");
        let reason = op.result.clone().unwrap_or_default();
        assert!(
            reason.contains("no pull request for T-5: the claim PR was never opened"),
            "{reason}"
        );
        // The captain hears about it, with the plan line that retries it.
        let items = failed_items(&mut store, "T-5");
        assert_eq!(items.len(), 1, "{items:?}");
        assert!(items[0].body.contains("- remerge T-5"), "{}", items[0].body);
        // Nothing was pushed.
        assert!(!scratch.dev_has_task_commit());
    }

    /// §11.3, row 2: a forge that refuses the merge fails the land op and the
    /// merge op, with the forge's own message.
    #[test]
    fn a_refused_land_fails_the_merge_with_the_forges_message() {
        let scratch = Scratch::new("pr-refused");
        let (worktree, candidate) = scratch.prepared();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = pr_world(&candidate);
        claim_op(&mut store, "T-5", 7);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        pr_step(&mut store, &mut herdr, &cfg, 20);
        pr_step(&mut store, &mut herdr, &cfg, 21);
        assert_eq!(merge_op(&mut store).state, OpState::Submitted);

        // The forge refuses the merge.
        gear_forge::sync_with(
            &mut store,
            "p",
            &FakeForge::refusing("not mergeable: required checks are missing"),
            30,
        )
        .unwrap();
        let land = land_op(&mut store, "T-5").expect("the land op is there");
        assert_eq!(land.state, OpState::Failed, "{land:?}");
        assert!(
            land.result
                .as_deref()
                .unwrap_or_default()
                .contains("required checks"),
            "{land:?}"
        );

        // The merge op fails with the forge's message, and the captain hears
        // about it (design §11.3, row 2).
        pr_step(&mut store, &mut herdr, &cfg, 40);
        let op = merge_op(&mut store);
        assert_eq!(op.state, OpState::Failed, "{op:?}");
        let reason = op.result.clone().unwrap_or_default();
        assert!(reason.contains("required checks"), "{reason}");
        let items = failed_items(&mut store, "T-5");
        assert_eq!(items.len(), 1, "{items:?}");
        assert!(items[0].body.contains("- remerge T-5"), "{}", items[0].body);
    }

    #[test]
    fn a_cancelled_task_voids_its_queued_merge() {
        // The task was cancelled after the merge was queued: the op is void and
        // nothing is merged (T-12 adversarial case).
        let mut store = world(TaskState::Verifying);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let mut cfg = test_cfg();
        cfg.profiles
            .insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
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

    /// A queued merge whose project config carries its own `merge_message`
    /// (T-296): everything else is [`queued_merge`].
    fn queued_merge_templated(
        scratch: &Scratch,
        gate: &str,
        template: &str,
    ) -> (Store, gear_herdr::fake::FakeHerdr, EngineCfg) {
        let (mut store, herdr, cfg) = queued_merge(scratch, gate);
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                // `[conventions]` is the config text's last section, so the
                // line lands in it.
                project.config_toml =
                    format!("{}\nmerge_message = \"{template}\"\n", config_text(gate));
                tx.update_project(&project)?;
                Ok(())
            })
            .unwrap();
        (store, herdr, cfg)
    }

    /// T-165 (review F2, design §10.7 row B03-01): a task whose work is
    /// already on the integration branch — it landed with another task's
    /// merge — is not merged again: no merge op is queued for it, and a merge
    /// op that was queued before is voided instead of run.
    #[test]
    fn a_task_already_merged_by_another_task_is_not_merged_again() {
        let mut store = world(TaskState::Ready);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        let mut cfg = test_cfg();
        cfg.profiles
            .insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
        store
            .write(|tx| {
                // T-5's attempt was accepted and carries the merge time of
                // the merge that landed it (another task's merge).
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                let mut attempt = tx.get_attempt("p", "T-5", 1)?.unwrap();
                attempt.state = gear_model::AttemptState::Accepted;
                attempt.candidate = Some("abc1234".into());
                tx.update_attempt(&attempt)?;
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.state = TaskState::Verifying;
                tx.update_task(&task)?;
                tx.set_attempt_time("p", "T-5", 1, gear_store::AttemptStep::Merged, 5)?;
                // A merge op was queued in an earlier tick, before the
                // attribution arrived.
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

        let merged = merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();

        assert!(merged.is_empty(), "the work was already merged: {merged:?}");
        // The queued op is void, not run: no worker was launched.
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Void, "{op:?}");
        assert!(!cfg.merges.running("p"), "no merge worker was launched");
        // And no second merge op was queued for the same attempt.
        let merges = store
            .read(|tx| tx.ops_in_state("p", OpState::Void))
            .unwrap()
            .iter()
            .filter(|op| op.kind == OpKind::Merge && op.task.as_deref() == Some("T-5"))
            .count();
        assert_eq!(merges, 1, "one merge op for the task, not two");
    }

    /// A project with a handed-in task and a queued merge op for it, whose
    /// candidate is the given commit (T-61: it may sit on any pushed ref).
    fn queued_merge_candidate(
        scratch: &Scratch,
        gate: &str,
        candidate: &str,
    ) -> (Store, gear_herdr::fake::FakeHerdr, EngineCfg) {
        queued_merge_payload(
            scratch,
            gate,
            candidate,
            serde_json::json!({ "branch": "heat/t5", "candidate": candidate }),
        )
    }

    /// A queued merge whose op carries the payload a test wants (T-532: the
    /// review branch and the `remerge` mark of the captain's retry).
    fn queued_merge_payload(
        scratch: &Scratch,
        gate: &str,
        candidate: &str,
        payload: serde_json::Value,
    ) -> (Store, gear_herdr::fake::FakeHerdr, EngineCfg) {
        let worktree = scratch.merge_worktree();
        let candidate = candidate.to_string();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
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
                    payload,
                    created: 10,
                })?;
                Ok(())
            })
            .unwrap();
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }
        (store, herdr, cfg)
    }

    /// How long a test waits for a merge worker before it calls the step hung.
    /// Nothing waits on a clock otherwise: the tick collects a finished
    /// worker, and a test waits on the worker's own report (T-127).
    const WAIT: std::time::Duration = std::time::Duration::from_secs(60);

    /// One merge step, then a wait for the worker it launched (or for the one
    /// it did not collect yet) to report: no test sleeps between attempts.
    fn tick_step(
        store: &mut Store,
        herdr: &mut gear_herdr::fake::FakeHerdr,
        cfg: &EngineCfg,
        now: Ts,
    ) -> Vec<Merged> {
        let seen = cfg.merges.progress();
        let merged = merge_passed(store, herdr, cfg, "p", now).unwrap();
        if cfg.merges.running("p") {
            assert!(
                cfg.merges.wait_progress(seen, WAIT),
                "the merge worker did not report within {WAIT:?}"
            );
        } else if cfg.merges.progress() == seen {
            panic!("the merge step had nothing to run");
        }
        merged
    }

    fn wait_merge(
        store: &mut Store,
        herdr: &mut gear_herdr::fake::FakeHerdr,
        cfg: &EngineCfg,
    ) -> Vec<Merged> {
        loop {
            let merged = tick_step(store, herdr, cfg, 30);
            let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
            if matches!(
                op.state,
                OpState::Confirmed | OpState::Failed | OpState::Void
            ) {
                return merged;
            }
        }
    }

    /// Drive the engine's merge step until the store is in the state the test
    /// wants.
    fn drive(
        store: &mut Store,
        herdr: &mut gear_herdr::fake::FakeHerdr,
        cfg: &EngineCfg,
        ready: impl Fn(&mut Store) -> bool,
    ) {
        while !ready(store) {
            tick_step(store, herdr, cfg, 30);
        }
    }

    /// Run the engine's merge step a few times, letting the worker run and be
    /// collected between the calls.
    fn ticks(
        store: &mut Store,
        herdr: &mut gear_herdr::fake::FakeHerdr,
        cfg: &EngineCfg,
        times: usize,
    ) {
        for _ in 0..times {
            tick_step(store, herdr, cfg, 30);
            while cfg.merges.running("p") {
                tick_step(store, herdr, cfg, 31);
            }
        }
    }

    /// The tip of every branch of the fixture repo's origin, by name: one git
    /// process for all of them (T-127).
    fn branch_heads(root: &Path) -> std::collections::BTreeMap<String, String> {
        let listing = sh(
            root,
            "git -C origin.git for-each-ref --format='%(refname:short) %(objectname)' refs/heads/",
        );
        listing
            .lines()
            .filter_map(|line| {
                let (name, sha) = line.split_once(' ')?;
                Some((name.trim().to_string(), sha.trim().to_string()))
            })
            .collect()
    }

    /// Whether `log` — what [`Pair::dev_log`] and [`Chain::dev_log`] read, one
    /// line per commit — holds `subject` exactly. A test reads the log once
    /// and its assertions read that string, never one git process per subject
    /// (T-127).
    fn log_has(log: &str, subject: &str) -> bool {
        log.lines().any(|line| line == subject)
    }

    /// The state of a merge op.
    fn op_state(store: &mut Store, id: &str) -> Option<OpState> {
        store
            .read(|tx| tx.get_op("p", id))
            .unwrap()
            .map(|op| op.state)
    }

    /// The result of a merge op.
    fn op_result(store: &mut Store, id: &str) -> String {
        store
            .read(|tx| tx.get_op("p", id))
            .unwrap()
            .and_then(|op| op.result)
            .unwrap_or_default()
    }

    /// A fifo a test and a gate it started hand a byte over (T-127): the test
    /// blocks on the gate's own write instead of polling the clock.
    fn fifo(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        let c_path = std::ffi::CString::new(path.display().to_string()).unwrap();
        // SAFETY: the path lives for the call; mkfifo only creates the node.
        assert_eq!(
            unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) },
            0,
            "mkfifo {}",
            path.display()
        );
        path
    }

    /// Block until the gate writes its byte into the fifo.
    fn await_fifo(path: &Path) {
        await_fifo_within(path, WAIT);
    }

    /// Block until the gate writes its byte into the fifo, but no longer than
    /// `bound`: a worker that died before its gate opened the other end would
    /// otherwise block the suite for ever (T-297).
    fn await_fifo_within(path: &Path, bound: std::time::Duration) {
        use std::io::Read;
        let (done, wait) = std::sync::mpsc::channel();
        let fifo = path.to_path_buf();
        std::thread::spawn(move || {
            let mut file = fs::File::open(&fifo).unwrap();
            let mut byte = [0u8; 1];
            file.read_exact(&mut byte).unwrap();
            let _ = done.send(());
        });
        wait.recv_timeout(bound).unwrap_or_else(|_| {
            panic!(
                "fifo handshake on {} did not happen within 60 s \
                 (the merge worker never reached its gate)",
                path.display()
            )
        });
    }

    /// Let a gate that reads the fifo go on.
    fn release_fifo(path: &Path) {
        release_fifo_within(path, WAIT);
    }

    /// Let a gate that reads the fifo go on, but no longer than `bound`: the
    /// write blocks until the gate opens its end, so a worker that died before
    /// that would otherwise block the suite for ever (T-297).
    fn release_fifo_within(path: &Path, bound: std::time::Duration) {
        let (done, wait) = std::sync::mpsc::channel();
        let fifo = path.to_path_buf();
        std::thread::spawn(move || {
            fs::write(&fifo, "go\n").unwrap();
            let _ = done.send(());
        });
        wait.recv_timeout(bound).unwrap_or_else(|_| {
            panic!(
                "fifo handshake on {} did not happen within 60 s \
                 (the merge worker never reached its gate)",
                path.display()
            )
        });
    }

    /// A gate that holds the first merge inside it: it says it started on
    /// `running`, waits for the test's byte on `release`, then runs `after`.
    /// Every later run of the gate is a no-op, so a second merge (a recovery
    /// after a kill, a retry) never waits on a fifo the test is not feeding
    /// (T-127).
    fn held_gate(root: &Path, tag: &str, after: &str) -> (PathBuf, PathBuf, String) {
        let running = fifo(root, &format!("{tag}-running"));
        let release = fifo(root, &format!("{tag}-release"));
        let marker = root.join(format!("{tag}-held"));
        let gate = format!(
            "if [ ! -e '{marker}' ]; then touch '{marker}'; echo x > '{running}'; \
             read go < '{release}'; {after}; fi",
            marker = marker.display(),
            running = running.display(),
            release = release.display(),
        );
        (running, release, gate)
    }

    /// The merge with its retries (T-61): a reason that can clear is retried a
    /// bounded number of times, with a growing wait between the attempts.
    fn merge_retried(
        worktree: &gear_herdr::Worktree,
        repo: &Repo,
        op: &OpRow,
    ) -> Result<String, Failed> {
        merge_retried_with(worktree, repo, op, &FetchRemote, &RetryWait::immediate())
    }

    fn merge_retried_with(
        worktree: &gear_herdr::Worktree,
        repo: &Repo,
        op: &OpRow,
        remote: &dyn Remote,
        wait: &RetryWait,
    ) -> Result<String, Failed> {
        merge_retried_on("origin", worktree, repo, op, remote, wait)
    }

    /// The merge on a fixture whose remote is named `remote_name` (T-262).
    fn merge_retried_on(
        remote_name: &str,
        worktree: &gear_herdr::Worktree,
        repo: &Repo,
        op: &OpRow,
        remote: &dyn Remote,
        wait: &RetryWait,
    ) -> Result<String, Failed> {
        let review = {
            let task = op.task.clone().unwrap_or_default();
            gear_model::branch::expand(gear_model::branch::REVIEW_BRANCH, "", "", &task)
        };
        // The tests' default message, as [`merge_one`] passes it (T-296).
        let commit = gear_model::commit::fill(
            gear_model::commit::MERGE_MESSAGE,
            &[("task", op.task.as_deref().unwrap_or_default())],
        );
        let prefix = test_review_prefix(op);
        step_result(merge_with_retries(
            worktree,
            repo,
            op,
            &lock_of(worktree)?,
            &[],
            Target {
                name: remote_name,
                runner: remote,
            },
            wait,
            None,
            &review,
            &commit,
            &prefix,
            "",
        ))
    }

    /// A runner that fails its first push and then behaves like [`FetchRemote`]
    /// (T-107): the merge's retry of a failure that can clear, without a hook.
    #[derive(Debug)]
    struct FailOnceRemote {
        failed: std::sync::atomic::AtomicBool,
        reason: String,
    }

    impl FailOnceRemote {
        fn new(reason: &str) -> FailOnceRemote {
            FailOnceRemote {
                failed: std::sync::atomic::AtomicBool::new(false),
                reason: reason.to_string(),
            }
        }
    }

    impl Remote for FailOnceRemote {
        fn push(&self, dir: &Path, remote: &str, dst: &str) -> Result<(), String> {
            if !self.failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return Err(self.reason.clone());
            }
            FetchRemote.push(dir, remote, dst)
        }

        fn delete(&self, dir: &Path, remote: &str, branch: &str) -> bool {
            FetchRemote.delete(dir, remote, branch)
        }
    }

    /// A runner that loses the push race `times` times (T-280): the remote
    /// rejects the push, then behaves like [`FetchRemote`].
    #[derive(Debug)]
    struct RaceRemote {
        left: std::sync::atomic::AtomicUsize,
        reason: String,
        /// Every push, refused or not: how many tries a test cost.
        pushes: std::sync::atomic::AtomicUsize,
    }

    impl RaceRemote {
        fn new(times: usize, reason: &str) -> RaceRemote {
            RaceRemote {
                left: std::sync::atomic::AtomicUsize::new(times),
                reason: reason.to_string(),
                pushes: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn pushes(&self) -> usize {
            self.pushes.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Remote for RaceRemote {
        fn push(&self, dir: &Path, remote: &str, dst: &str) -> Result<(), String> {
            self.pushes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let left = self.left.load(std::sync::atomic::Ordering::SeqCst);
            if left > 0 {
                self.left
                    .store(left - 1, std::sync::atomic::Ordering::SeqCst);
                return Err(self.reason.clone());
            }
            FetchRemote.push(dir, remote, dst)
        }

        fn delete(&self, dir: &Path, remote: &str, branch: &str) -> bool {
            FetchRemote.delete(dir, remote, branch)
        }
    }

    /// What a remote says when the integration branch moved under the push.
    const REJECTED: &str = "! [rejected] dev -> dev (non-fast-forward)";

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
             && git -C ../origin.git fetch -q $(pwd) +HEAD:refs/heads/dev; fi",
            raced = raced.display(),
            work = scratch.work.display(),
        );

        let line =
            merge_retried(&worktree, &repo(&racer), &op(&scratch.head_of("heat/t5"))).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        let log = scratch.dev_log();
        assert!(log.contains("racer"), "the race happened: {log}");
        assert!(log.contains("task"), "the task landed: {log}");
        assert!(scratch.dev_has_task_commit());
    }

    /// T-280: a merge that loses the push race on every try is not a failed
    /// merge. The step says it raced, so the op goes back to the queue
    /// instead of a captain item and a `- remerge` by hand.
    #[test]
    fn a_merge_that_loses_the_push_race_on_every_try_is_raced() {
        let scratch = Scratch::new("raced-step");
        let worktree = scratch.merge_worktree();
        let remote = RaceRemote::new(4, REJECTED);

        let failed = merge_retried_with(
            &worktree,
            &repo("true"),
            &op(&scratch.head_of("heat/t5")),
            &remote,
            &RetryWait::immediate(),
        )
        .unwrap_err();

        assert!(failed.reason.starts_with(RACE), "{}", failed.reason);
        assert_eq!(failed.blocker, Blocker::Clear, "a race can clear");
        assert_eq!(
            remote.pushes(),
            MERGE_RETRIES as usize + 1,
            "one push per try"
        );
        assert!(!scratch.dev_has_task_commit(), "nothing of T-5 was pushed");
    }

    /// T-280: several fleets merge on one integration branch. A merge that
    /// loses the race on every try of one tick goes back to the queue and
    /// lands from the new tip on the next tick: no Failed state on the way,
    /// no captain item.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_merge_that_races_every_try_goes_back_to_the_queue_and_lands() {
        let scratch = Scratch::new("raced-queue");
        let (mut store, mut herdr, mut cfg) = queued_merge(&scratch, "true");
        let remote = Arc::new(RaceRemote::new(MERGE_RETRIES as usize + 1, REJECTED));
        cfg.remote = remote.clone();

        drive(&mut store, &mut herdr, &cfg, |store| {
            let state = op_state(store, "O-1");
            assert_ne!(state, Some(OpState::Failed), "a race is not a failure");
            state == Some(OpState::Confirmed)
        });

        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Confirmed, "{op:?}");
        // The first launch raced every try; the second launch's push landed.
        assert_eq!(
            remote.pushes(),
            MERGE_RETRIES as usize + 2,
            "one push per try"
        );
        assert!(scratch.dev_has_task_commit(), "T-5 landed on dev");
        assert!(
            failed_items(&mut store, "T-5").is_empty(),
            "a race rings no captain"
        );
    }

    /// T-280: every other clear failure keeps today's three retries and its
    /// Failed end — a push a hook declines is not the remote moving.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_push_a_hook_declines_every_try_still_fails_and_rings_the_captain() {
        let scratch = Scratch::new("declined");
        let (mut store, mut herdr, mut cfg) = queued_merge(&scratch, "true");
        cfg.remote = Arc::new(RaceRemote::new(usize::MAX, "hook declined"));

        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-1") == Some(OpState::Failed)
        });

        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Failed, "{op:?}");
        let items = failed_items(&mut store, "T-5");
        assert_eq!(items.len(), 1, "the captain hears once: {items:?}");
        assert!(
            items[0].body.contains("push failed: hook declined"),
            "{}",
            items[0].body
        );
        assert!(!scratch.dev_has_task_commit(), "nothing of T-5 was pushed");
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

        let line =
            merge_retried(&worktree, &repo(&gate), &op(&scratch.head_of("heat/t5"))).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        assert!(scratch.dev_has_task_commit());
    }

    /// A gate that exits 75 (EX_TEMPFAIL) could not judge this time (a CI
    /// service refused a request): the merge is retried and lands, as for a
    /// missing toolchain (Prometheus, 2026-11-03: GitHub HTTP 403 failed three
    /// merges outright).
    #[test]
    fn a_gate_that_could_not_judge_is_retried_and_the_merge_lands() {
        let scratch = Scratch::new("gate-tempfail");
        let worktree = scratch.merge_worktree();
        let ran = scratch.root.join("gate-ran");
        let gate = format!(
            "if [ ! -e '{ran}' ]; then touch '{ran}'; exit 75; fi",
            ran = ran.display()
        );

        let line =
            merge_retried(&worktree, &repo(&gate), &op(&scratch.head_of("heat/t5"))).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        assert!(scratch.dev_has_task_commit());
    }

    /// A gate behind a wrapper that hides exit 75 (`make` turns it into its
    /// own exit 2) asks for a retry by creating the file `GEAR_GATE_TEMPFAIL`
    /// names; the merge is retried and lands (Prometheus, 2026-11-03).
    #[test]
    fn a_gate_that_marks_the_tempfail_file_is_retried_and_the_merge_lands() {
        let scratch = Scratch::new("gate-tempfail-file");
        let worktree = scratch.merge_worktree();
        let ran = scratch.root.join("gate-ran");
        let gate = format!(
            "if [ ! -e '{ran}' ]; then touch '{ran}'; touch $GEAR_GATE_TEMPFAIL; exit 2; fi",
            ran = ran.display()
        );

        let line =
            merge_retried(&worktree, &repo(&gate), &op(&scratch.head_of("heat/t5"))).unwrap();

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
        let op = merge_op(&mut store);
        assert_eq!(op.state, OpState::Failed, "{op:?}");
        let reason = op.result.clone().unwrap_or_default();
        assert!(reason.contains("no pushed ref"), "{reason}");
        assert!(
            reason.contains("retried"),
            "the retries are named: {reason}"
        );
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
                // T-72: the child repo's own origin, as `init` would freeze it.
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
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
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
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut store = child_world(&config_toml, &candidate);
        let mut cfg = test_cfg();
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
                gear_herdr::fake::Call::WorktreeOpen {
                    repo,
                    branch,
                    label,
                } => Some((repo.clone(), branch.clone(), label.clone())),
                _ => None,
            })
            .expect("the engine opened the merge worktree");
        assert_eq!(opened.0, format!("{}/web", scratch.root.display()));
        assert_eq!(opened.1, "p/merge/between-voyages");
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
        assert_eq!(
            scratch.dev_log(),
            "dev",
            "the root repo was not merged into"
        );
    }

    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_task_from_before_a_lane_rebind_merges_on_its_original_repo_and_gate() {
        let scratch = Scratch::new("rebound-child");
        let child = Child::new(&scratch.root, "web", "web-dev");
        let worktree = child.merge_worktree("web-dev");
        let candidate = child.head_of("heat/t5");
        let child_gate = scratch.root.join("old-gate");
        let new_gate = scratch.root.join("new-gate");
        let config_toml = child_config(
            &scratch.root,
            &format!("touch '{}'", child_gate.display()),
            &format!("touch '{}'", new_gate.display()),
        );
        let mut store = child_world(&config_toml, &candidate);
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-5")?.unwrap();
                task.repo = Some(scratch.root.join("web").to_string_lossy().into_owned());
                task.base = Some("web-dev".into());
                task.gate = Some(format!("touch '{}'", child_gate.display()));
                tx.update_task(&task)?;
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = project.config_toml.replace("repo = \"web\"", "");
                tx.update_project(&project)
            })
            .unwrap();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }
        assert_eq!(wait_merge(&mut store, &mut herdr, &cfg).len(), 1);
        assert!(herdr.calls.iter().any(|call| matches!(call, gear_herdr::fake::Call::WorktreeOpen {repo,branch,..} if repo == &scratch.root.join("web").to_string_lossy() && branch == "p/merge/between-voyages")));
        assert!(child_gate.exists());
        assert!(!new_gate.exists());
        assert!(child.log_of("web-dev").contains("task"));
        assert_eq!(scratch.dev_log(), "dev");
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
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }

        merge_passed(&mut store, &mut herdr, &cfg, "p", 30).unwrap();

        let created = herdr
            .calls
            .iter()
            .find_map(|call| match call {
                gear_herdr::fake::Call::WorktreeCreate {
                    repo,
                    branch,
                    base,
                    label,
                } => Some((repo.clone(), branch.clone(), base.clone(), label.clone())),
                _ => None,
            })
            .expect("the engine created the merge worktree");
        assert_eq!(created.0, format!("{}/web", scratch.root.display()));
        assert_eq!(created.1, "p/merge/between-voyages");
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
             && git add -A && git -c user.email=a@b -c user.name=a commit -qm rework",
        );
        publish(&scratch.work, "heat/t5-v2");
        let candidate = scratch.head_of("heat/t5-v2");
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
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
        let mut cfg = test_cfg();
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
    fn dispatch_and_doorbells_continue_while_the_gate_runs() {
        let scratch = Scratch::new("tick-gate");
        let done = scratch.root.join("gate-finished");
        let (running, release, gate) = held_gate(
            &scratch.root,
            "gate",
            &format!("touch '{}'", done.display()),
        );
        let (mut store, mut herdr, mut cfg) = queued_merge(&scratch, &gate);
        cfg.profiles
            .get_mut("opencode")
            .unwrap()
            .new_session
            .clear();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 10).unwrap();
        // The worker is in the gate, which stays there until the test says go.
        await_fifo(&running);
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
                    mechanical: None,
                })?;
                tx.insert_message(&NewMessage {
                    meta: None,
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
        release_fifo(&release);
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
        let running = fifo(&scratch.root, "gate-running");
        let release = fifo(&scratch.root, "gate-release");
        // The first merge's gate stays until the test lets it go; every run
        // adds one line to `started`.
        let gate = format!(
            "echo gate >> '{}'; if [ ! -e '{marker}' ]; then touch '{marker}'; \
             echo x > '{running}'; read go < '{release}'; fi",
            started.display(),
            marker = scratch.root.join("gate-held").display(),
            running = running.display(),
            release = release.display(),
        );
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
        await_fifo(&running);
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
        release_fifo(&release);
        assert_eq!(wait_merge(&mut store, &mut herdr, &cfg).len(), 1);
        // The second merge runs next, and reports its own result. Its branches
        // are T-5's, already on dev, so it lands without a gate of its own.
        drive(&mut store, &mut herdr, &cfg, |store| {
            op_state(store, "O-2") == Some(OpState::Confirmed)
        });
        assert_eq!(fs::read_to_string(&started).unwrap().lines().count(), 1);
        assert_eq!(
            store
                .read(|tx| tx.get_op("p", "O-2"))
                .unwrap()
                .unwrap()
                .state,
            OpState::Confirmed
        );
    }

    /// T-127: the merge lock is found without asking git (a process saved on
    /// every attempt), so the path it derives must be the one git names — in
    /// the fixture's own checkout and in the linked worktree the merge runs in.
    #[test]
    fn the_merge_lock_is_the_file_git_names() {
        let scratch = Scratch::new("lock-path");
        let worktree = scratch.merge_worktree();
        for dir in [scratch.work.as_path(), Path::new(&worktree.path)] {
            // git answers a plain checkout relatively and a linked worktree
            // absolutely; the merge joins it onto the worktree either way, and
            // that is the file this must name.
            let named = sh(dir, "git rev-parse --git-path gear-merge.lock");
            assert_eq!(
                super::merge_lock_path(dir),
                dir.join(named.trim()),
                "the lock path of {}",
                dir.display()
            );
        }
        // The worktree's `.git` is a file, not a directory: the path is
        // resolved through it, and the lock is still one file for both sides.
        assert!(scratch.root.join("merge/.git").is_file());
    }

    /// Two projects in one repo that land on the same integration branch
    /// merge one after the other: while another project's merge holds the
    /// branch, this project's merge stays queued, and the other project's
    /// result is never collected as this project's.
    #[test]
    fn another_projects_merge_into_the_same_branch_goes_first() {
        let scratch = Scratch::new("shared");
        let (mut store, mut herdr, cfg) = queued_merge(&scratch, "true");
        let mut other = op("abc1234");
        other.project = "q".into();
        other.seat = "q:heat".into();
        let finished = Arc::new(Mutex::new(Some(Finished {
            result: Report::Merged("q merged".into()),
            at: 20,
        })));
        cfg.merges.workers.lock().unwrap().insert(
            worker_key(&repo("true")),
            Worker {
                op: other,
                finished,
            },
        );
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        assert_eq!(op_state(&mut store, "O-1"), Some(OpState::Intended));
        assert!(cfg.merges.running("q"), "q's result stays for q's own tick");
        assert!(!cfg.merges.running("p"));
        assert_eq!(
            scratch.dev_log(),
            "dev",
            "no merge while q holds the branch"
        );
        cfg.merges.workers.lock().unwrap().clear();
        assert_eq!(wait_merge(&mut store, &mut herdr, &cfg).len(), 1);
    }

    /// Another integration branch of the same repo is another key: those
    /// merges do not wait for each other.
    #[test]
    fn the_merge_worker_key_is_the_repo_and_its_integration_branch() {
        let dev = repo("true");
        let mut staging = dev.clone();
        staging.integration = "staging".into();
        let mut child = dev.clone();
        child.name = "child".into();
        assert_ne!(worker_key(&dev), worker_key(&staging));
        assert_eq!(worker_key(&dev), worker_key(&child));
    }

    #[test]
    fn a_busy_worktree_waits_without_running_a_second_merge() {
        let scratch = Scratch::new("locked");
        let (mut store, mut herdr, cfg) = queued_merge(&scratch, "true");
        let lock = merge_lock(&scratch.root.join("merge")).unwrap().unwrap();
        // The worker finds the lock taken and reports Busy; the next step
        // puts the op back in the queue.
        tick_step(&mut store, &mut herdr, &cfg, 20);
        while op_state(&mut store, "O-1") != Some(OpState::Unknown) {
            tick_step(&mut store, &mut herdr, &cfg, 21);
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
            branch: "p/merge/between-voyages".into(),
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
        let done = scratch.root.join("gate-finished");
        // The held gate writes no fifo after the release: it must exit on its
        // own, because it is the process that holds the merge lock and the
        // recovery below cannot run until it does.
        let (running, release, gate) = held_gate(
            &scratch.root,
            "gate",
            &format!("touch '{}'", done.display()),
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
        await_fifo(&running);
        assert_eq!(
            scratch.dev_log(),
            "dev",
            "gate has not passed, so origin is untouched"
        );
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            merge_lock(&scratch.root.join("merge")).unwrap().is_none(),
            "the surviving gate must exclude recovery until it exits"
        );
        release_fifo(&release);
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
        // The orphaned gate has finished — recovery needed the lock it held —
        // and no surviving merge thread pushed its half-finished tree.
        assert!(done.exists(), "the orphaned gate is still running");
    }

    #[test]
    fn a_gate_that_is_still_running_does_not_block_the_merge_step() {
        let scratch = Scratch::new("nonblocking");
        let done = scratch.root.join("gate-finished");
        let (running, release, gate) = held_gate(
            &scratch.root,
            "gate",
            &format!("touch '{}'", done.display()),
        );
        let (mut store, mut herdr, cfg) = queued_merge(&scratch, &gate);
        // A whole second before the completion: the launching tick's time and
        // the gate's end are then told apart by the strict `>` below, with no
        // sleep to make the difference (T-127, review F3).
        let launched_at = wall_time() - 1;
        let result = merge_passed(&mut store, &mut herdr, &cfg, "p", launched_at).unwrap();
        assert!(!done.exists(), "merge_passed waited for the running gate");
        assert!(result.is_empty(), "the merge is still running");
        // The worker waits inside the gate, which the test holds open.
        await_fifo(&running);
        assert!(
            store
                .read(|tx| tx.get_op("p", "O-1"))
                .unwrap()
                .unwrap()
                .state
                != OpState::Confirmed
        );
        let released_at = wall_time();
        release_fifo(&release);
        assert_eq!(wait_merge(&mut store, &mut herdr, &cfg).len(), 1);
        assert!(done.exists());
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.state, OpState::Confirmed);
        // The completion time is when the gate and the push ended, not when
        // the launching tick ran, and not before the gate was let go.
        assert!(
            op.updated > launched_at,
            "the completion time is not the launching tick's"
        );
        assert!(
            op.updated >= released_at,
            "the completion time follows the running gate"
        );
        assert!(op.updated <= wall_time());
        assert_eq!(
            store
                .read(|tx| tx.attempt_times("p", "T-5", 1))
                .unwrap()
                .merged,
            Some(op.updated)
        );
    }

    #[test]
    fn a_clean_merge_is_pushed_and_the_branches_are_gone() {
        let scratch = Scratch::new("clean");
        let worktree = scratch.merge_worktree();

        let line = merge_one(&worktree, &repo("true"), &op(&scratch.head_of("heat/t5"))).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        // dev has both commits … (one shell for the log and the branches)
        let state = scratch.state();
        let log = &state.dev;
        assert!(log.contains("task"), "{log}");
        assert!(log.contains("review"), "{log}");
        assert_eq!(
            log.lines()
                .filter(|line| line.starts_with("merge(T-5)"))
                .count(),
            2,
            "{log}"
        );
        assert!(state.has_task_commit());
        // … and the two branches are closed on origin.
        let branches = &state.branches;
        assert!(!branches.contains("heat/t5"), "{branches}");
        assert!(!branches.contains("review/t5"), "{branches}");
    }

    /// A merge whose task and review branches are already on the integration
    /// branch (they landed with another merge) is merged, not gated: the gate
    /// would refuse a candidate that carries nothing new (Prometheus T-745).
    #[test]
    fn a_merge_whose_branches_are_already_on_dev_is_merged_without_a_gate() {
        let scratch = Scratch::new("already-on-dev");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        sh(
            &scratch.work,
            "git fetch -q origin && git switch -qC landed origin/dev \
             && git -c user.email=a@b -c user.name=a merge -q --no-ff origin/review/t5 -m landed \
             && git push -q origin HEAD:dev",
        );

        let line = merge_one(&worktree, &repo("false"), &op(&candidate)).unwrap();

        assert!(line.contains("already merged"), "{line}");
    }

    /// T-573: only a file under the voyage folder counts, with `{project}`
    /// filled and `{milestone}` any one folder.
    #[test]
    fn only_a_file_under_the_voyage_folder_is_a_report() {
        let template = "gear/{project}/voyages/{milestone}";
        assert!(is_under_voyage(
            "gear/p/voyages/M1/reviews/T-5.md",
            template,
            "p"
        ));
        assert!(is_under_voyage("gear/p/voyages/M1/plan.md", template, "p"));
        // Any milestone, one folder: not another project's, not the folder
        // itself, not a file that only looks like one.
        assert!(!is_under_voyage(
            "gear/other/voyages/M1/reviews/T-5.md",
            template,
            "p"
        ));
        assert!(!is_under_voyage("gear/p/voyages/M1", template, "p"));
        assert!(!is_under_voyage("gear/p/voyages", template, "p"));
        assert!(!is_under_voyage("src/lib.rs", template, "p"));
        // The default template names no project, and matches the same way.
        let default = gear_model::layout::VOYAGE_DIR;
        assert!(is_under_voyage(
            "gear/voyages/M3/reports/T-7.md",
            default,
            "p"
        ));
        assert!(!is_under_voyage("gear/voyages/M3", default, "p"));
        // A merge that changes nothing is no report-only merge.
        assert!(!reports_only(&[], template, "p"));
        assert!(reports_only(
            &[
                "gear/p/voyages/M1/reports/T-5.md".to_string(),
                "gear/p/voyages/M1/reviews/T-5.md".to_string(),
            ],
            template,
            "p"
        ));
        assert!(!reports_only(
            &[
                "gear/p/voyages/M1/reports/T-5.md".to_string(),
                "src/lib.rs".to_string(),
            ],
            template,
            "p"
        ));
    }

    /// The config of a test project whose voyage folder names the project
    /// (T-573): `gear/<project>/voyages/<milestone>`.
    fn repo_with_voyage_dir(gate: &str) -> Repo {
        gear_app::config::parse(&format!(
            "{}\nvoyage_dir = \"gear/{{project}}/voyages/{{milestone}}\"\n",
            config_text(gate)
        ))
        .unwrap()
        .repo_of_lane("heat")
    }

    /// A scratch repo carrying one report-only task: `heat/t6` holds a commit
    /// that adds only `gear/p/voyages/M1/reviews/T-6.md`, and the review commit
    /// on top of it adds only a record.
    fn reports_only_task(scratch: &Scratch) -> String {
        sh(
            &scratch.work,
            "git switch -qc report/t6 dev && mkdir -p gear/p/voyages/M1/reviews \
             && echo record > gear/p/voyages/M1/reviews/T-6.md && git add -A \
             && git -c user.email=a@b -c user.name=a commit -qm report \
             && git -C ../origin.git fetch -q \"$(pwd)\" +HEAD:refs/heads/heat/t6 \
             && git fetch -q origin",
        );
        let candidate = sh(&scratch.work, "git rev-parse HEAD");
        sh(
            &scratch.work,
            "echo reviewed >> gear/p/voyages/M1/reviews/T-6.md && git add -A \
             && git -c user.email=a@b -c user.name=a commit -qm 'review(T-6): the record is right' \
             && git -C ../origin.git fetch -q \"$(pwd)\" +HEAD:refs/heads/heat/t6 \
             && git fetch -q origin",
        );
        candidate.trim().to_string()
    }

    /// The merge op of `reports_only_task`.
    fn reports_only_op(candidate: &str) -> OpRow {
        let mut report = op(candidate);
        report.task = Some("T-6".into());
        report.payload = serde_json::json!({
            "branch": "heat/t6",
            "candidate": candidate,
            "review": "review/t6",
        });
        report
    }

    /// T-573: a merge that carries only reports skips the gate (the user,
    /// 2026-11-05: a merge holding one review record still paid a four-to-five
    /// minute CI round). The gate here always fails, so the merge landing
    /// proves it did not run.
    #[test]
    fn a_merge_that_carries_only_reports_skips_the_gate() {
        let scratch = Scratch::new("reports-only");
        let worktree = scratch.merge_worktree();
        let candidate = reports_only_task(&scratch);

        let line = merge_one(
            &worktree,
            &repo_with_voyage_dir("false"),
            &reports_only_op(&candidate),
        )
        .expect("a report-only merge runs no gate");

        assert!(line.contains("merged T-6 into dev"), "{line}");
        assert!(line.contains("no gate: reports only"), "{line}");
        // The conflict check and the push ran as usual: dev holds the record.
        let dev = scratch.dev_log();
        assert!(dev.contains("report"), "{dev}");
        assert!(dev.contains("review(T-6)"), "{dev}");
        assert!(!line.contains("gate failed"), "{line}");
    }

    /// The same merge with one ordinary file in it: the gate runs and its
    /// failure ends the merge.
    #[test]
    fn a_merge_that_touches_another_file_still_gates() {
        let scratch = Scratch::new("reports-plus");
        let worktree = scratch.merge_worktree();
        let candidate = reports_only_task(&scratch);
        // One file outside the voyage folder, on the same candidate.
        sh(
            &scratch.work,
            "git switch -q report/t6 && echo note > notes.txt && git add -A \
             && git -c user.email=a@b -c user.name=a commit -qm notes \
             && git -C ../origin.git fetch -q \"$(pwd)\" +HEAD:refs/heads/heat/t6 \
             && git fetch -q origin",
        );

        let failed = merge_one(
            &worktree,
            &repo_with_voyage_dir("false"),
            &reports_only_op(&candidate),
        )
        .expect_err("a merge with another file runs the gate, and it fails");
        assert!(!failed.reason.contains("no gate"), "{}", failed.reason);
        assert!(!scratch.dev_log().contains("report"), "nothing landed");
    }

    /// T-483: a confirmed merge closes the review branch it merged, whatever
    /// its tip holds. A review branch left by an earlier attempt of the task
    /// does not contain the candidate, and it stayed on the remote before
    /// this (T-457 attempt 2: a merge left `review/t457` behind). The earlier
    /// attempt's review stays in that attempt's record.
    #[test]
    fn a_confirmed_merge_deletes_the_review_branch_of_an_earlier_attempt() {
        let scratch = Scratch::new("stale-review");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        // The review branch still points at an earlier attempt's review: a
        // commit cut from `dev`, not from this attempt's candidate.
        sh(
            &scratch.work,
            "git switch -qc earlier dev && echo earlier > earlier.txt && git add -A \
             && git -c user.email=a@b -c user.name=a commit -qm earlier-review \
             && git -C ../origin.git fetch -q \"$(pwd)\" +HEAD:refs/heads/review/t5 \
             && git fetch -q origin",
        );
        assert!(!sh_ok(
            &scratch.root,
            &format!("git -C origin.git merge-base --is-ancestor {candidate} review/t5")
        ));

        let line = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        assert!(
            line.contains("closed on origin: ") && line.contains("review/t5"),
            "{line}"
        );
        // Neither the lane branch nor the review branch is on the remote.
        let branches = scratch.origin_branches();
        assert!(!branches.contains("heat/t5"), "{branches}");
        assert!(!branches.contains("review/t5"), "{branches}");
    }

    /// T-407: a merge writes one line per step — fetch, merge, gate, push,
    /// close — each with its own time and the attempt's time so far, so a
    /// slow step is found from the engine log.
    #[test]
    fn a_merge_logs_its_five_steps_in_the_engine_log() {
        let scratch = Scratch::new("merge-probe");
        let worktree = scratch.merge_worktree();
        // A task of its own, so no other test's merge lines mix in.
        let mut op = op(&scratch.head_of("heat/t5"));
        op.task = Some("T-9071".into());

        let line = merge_one(&worktree, &repo("true"), &op).unwrap();

        assert!(line.contains("merged T-9071 into dev"), "{line}");
        let log: Vec<String> = crate::ops::probe_log()
            .into_iter()
            .filter(|line| line.starts_with("probe merge T-9071 "))
            .collect();
        let steps: Vec<&str> = log
            .iter()
            .map(|line| line.split_whitespace().nth(3).unwrap())
            .collect();
        assert_eq!(
            steps,
            ["fetch", "merge", "gate", "push", "close", "done"],
            "{log:?}"
        );
        // Every line carries the step's time and the whole attempt's so far.
        for line in &log {
            let fields: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 6, "{line}");
            assert!(fields[4].parse::<u64>().is_ok(), "the step's ms: {line}");
            assert!(
                fields[5].starts_with("total=") && fields[5][6..].parse::<u64>().is_ok(),
                "{line}"
            );
        }
    }

    /// T-407: a step the engine restart interrupts keeps its durable
    /// beginning, so the attempt that runs after the restart measures it from
    /// the time it really started and not from its own first key.
    #[test]
    fn a_step_interrupted_by_a_restart_is_measured_from_its_durable_beginning() {
        let scratch = Scratch::new("merge-probe-restart");
        let worktree = scratch.merge_worktree();
        let mut op = op(&scratch.head_of("heat/t5"));
        // A task of its own, so the other probe test's lines never mix in.
        op.task = Some("T-9072".into());
        // The engine died in the fetch a minute ago: the op's meta says so.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("gear.db").to_string_lossy().into_owned();
        let mut store = gear_store::Store::open(std::path::Path::new(&db)).unwrap();
        let began = now_ms() - 60_000;
        store
            .write(|tx| {
                tx.set_meta(
                    &probe_key("O-1"),
                    &serde_json::json!([["fetch", began, null]]).to_string(),
                )
            })
            .unwrap();
        drop(store);

        let line = merge_one_with_db(&worktree, &repo("true"), &op, &db).unwrap();

        assert!(line.contains("merged T-9072 into dev"), "{line}");
        let fetch = crate::ops::probe_log()
            .into_iter()
            .find(|line| line.starts_with("probe merge T-9072 fetch "))
            .expect("the fetch is logged");
        let ms: i64 = fetch.split_whitespace().nth(4).unwrap().parse().unwrap();
        assert!(
            ms >= 60_000,
            "the fetch runs from its durable beginning: {fetch}"
        );
        // The recovered record keeps that beginning, and now the step's end.
        let mut store = gear_store::Store::open(std::path::Path::new(&db)).unwrap();
        let kept = read_steps(&mut store, &probe_key("O-1"));
        assert_eq!(kept[0].0, "fetch");
        assert_eq!(kept[0].1, began, "the beginning is the durable one");
        assert!(
            kept[0].2.is_some(),
            "and the step has its end now: {kept:?}"
        );
    }

    /// F1 of the T-407 review: a retry starts a new boundary for a step
    /// whose earlier boundary ended, so each attempt's step time is its own —
    /// not the time since the first attempt's fetch began.
    #[test]
    fn a_retried_step_logs_its_own_time() {
        // The durable record of an earlier attempt: fetch began at 1000 ms and
        // ended at 1200 ms (the reviewer's numbers). The retry reopens it.
        let mut probe = Probe {
            task: "T-9073".into(),
            started_ms: 500,
            kept: vec![("fetch".into(), 1000, Some(1200))],
            store: None,
        };
        probe.begin("fetch");
        let retry = probe
            .kept
            .iter()
            .find(|(step, begin, end)| step == "fetch" && end.is_none() && *begin > 1200)
            .unwrap_or_else(|| panic!("the retry has its own boundary: {:?}", probe.kept));
        assert!(
            retry.1 > 1200,
            "the retry starts now, not at the ended boundary"
        );
        probe.step("fetch");
        let line = crate::ops::probe_log()
            .into_iter()
            .find(|line| line.starts_with("probe merge T-9073 fetch "))
            .expect("the retry's fetch line");
        let ms: i64 = line.split_whitespace().nth(4).unwrap().parse().unwrap();
        // Its own time (a few milliseconds), not the 1000 ms between the first
        // attempt's beginning and now.
        assert!(
            (0..5_000).contains(&ms),
            "the retry's fetch reads its own time: {line}"
        );
        // The earlier attempt keeps its completed boundary.
        assert_eq!(
            probe
                .kept
                .iter()
                .filter(|(step, _, _)| step == "fetch")
                .count(),
            2
        );
    }

    /// T-296: with the default template the merge commits read exactly as they
    /// read before: the candidate by its sha, and the review branch, under
    /// `merge(T-5): `.
    #[test]
    fn the_default_merge_message_names_the_task_and_what_it_merged() {
        let scratch = Scratch::new("merge-message-default");
        let candidate = scratch.head_of("heat/t5");
        let (mut store, mut herdr, cfg) = queued_merge(&scratch, "true");

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        wait_merge(&mut store, &mut herdr, &cfg);

        let log = scratch.dev_log();
        assert!(log_has(&log, &format!("merge(T-5): {candidate}")), "{log}");
        assert!(log_has(&log, "merge(T-5): origin/review/t5"), "{log}");
    }

    /// T-296: a team with commit rules reads its own message. `{project}` and
    /// `{task}` are filled once per op, `{source}` on every merge of it.
    #[test]
    fn a_project_merge_message_replaces_the_default() {
        let scratch = Scratch::new("merge-message-template");
        let candidate = scratch.head_of("heat/t5");
        let (mut store, mut herdr, cfg) = queued_merge_templated(
            &scratch,
            "true",
            "chore(merge): {task} [{project}] {source}",
        );

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        wait_merge(&mut store, &mut herdr, &cfg);

        let log = scratch.dev_log();
        assert!(
            log_has(&log, &format!("chore(merge): T-5 [p] {candidate}")),
            "{log}"
        );
        assert!(
            log_has(&log, "chore(merge): T-5 [p] origin/review/t5"),
            "{log}"
        );
        assert!(
            !log.contains("merge(T-5)"),
            "the default is not used: {log}"
        );
    }

    /// A push rejected because dev moved during the gate (the captain pushed)
    /// is a race the merge retries, not a failed merge.
    #[test]
    fn dev_moving_during_the_gate_is_a_race() {
        let scratch = Scratch::new("race");
        let worktree = scratch.merge_worktree();
        let racer = format!(
            "cd {} && git fetch -q origin && git switch -q dev && git reset -q --hard origin/dev && echo x >> other.txt && git add other.txt && git -c user.email=a@b -c user.name=a commit -qm racer && git -C ../origin.git fetch -q $(pwd) +HEAD:refs/heads/dev",
            scratch.work.display()
        );

        let failed =
            merge_one(&worktree, &repo(&racer), &op(&scratch.head_of("heat/t5"))).unwrap_err();

        assert!(failed.reason.starts_with(RACE), "{}", failed.reason);
        assert_eq!(failed.blocker, Blocker::Clear, "a race can clear");
        assert!(!scratch.dev_has_task_commit(), "nothing of T-5 was pushed");
    }

    /// F1 of the T-61 review: a push that fails once for a reason that can
    /// clear is not a failed merge. The merge waits, tries again, and lands.
    /// (T-107: the failure comes from the runner — the engine no longer
    /// installs a pre-push hook to reject it.)
    #[test]
    fn a_push_that_fails_once_is_retried_and_lands() {
        let scratch = Scratch::new("hook");
        let worktree = scratch.merge_worktree();
        let remote = FailOnceRemote::new("the remote said no once");

        let line = merge_retried_with(
            &worktree,
            &repo("true"),
            &op(&scratch.head_of("heat/t5")),
            &remote,
            &RetryWait::immediate(),
        )
        .unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        assert!(
            remote.failed.load(std::sync::atomic::Ordering::SeqCst),
            "the runner refused once"
        );
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
        // The lane pushes while the merge waits: the first attempt does not
        // see the candidate, and the wait before the retry is where the push
        // lands — no test sleeps on the clock to get that order (T-127).
        let work = scratch.work.clone();
        let wait = RetryWait::instead(move |_| publish(&work, "heat/t5-late"));

        let line = merge_retried_with(
            &worktree,
            &repo("true"),
            &op(&candidate),
            &FetchRemote,
            &wait,
        )
        .unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        assert!(scratch.dev_has_task_commit());
    }

    /// L-67: a failed merge is not queued again tick after tick (it filed 120
    /// items for the captain); a new passing attempt gets its own merge.
    #[test]
    fn a_failed_merge_is_not_queued_again() {
        let mut store = gear_store::Store::open_in_memory().unwrap();
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

        assert!(
            merge_op_of(&mut store, "p", "T-5", 1).unwrap().is_some(),
            "the failed merge stands"
        );
        assert!(
            merge_op_of(&mut store, "p", "T-5", 2).unwrap().is_none(),
            "a new attempt merges anew"
        );
    }

    #[test]
    fn a_conflict_leaves_dev_untouched_and_fails() {
        let scratch = Scratch::new("conflict");
        // Make the task branch and dev disagree on the same line.
        // dev moves the same line the task branch appends to.
        sh(&scratch.work, "git switch -q dev && printf 'dev\nother\n' > README.md && git -c user.email=a@b -c user.name=a commit -qam other");
        publish(&scratch.work, "dev");
        let worktree = scratch.merge_worktree();
        let before = scratch.dev_log();

        let failed =
            merge_one(&worktree, &repo("true"), &op(&scratch.head_of("heat/t5"))).unwrap_err();

        assert!(
            failed.reason.contains("conflict merging"),
            "{}",
            failed.reason
        );
        assert_eq!(failed.blocker, Blocker::Stuck, "a conflict does not clear");
        let state = scratch.state();
        assert_eq!(state.dev, before, "dev is untouched");
        let branches = &state.branches;
        assert!(branches.contains("heat/t5"), "{branches}");
    }

    /// T-532: the repos a conflict needs (dev moves the line the candidate
    /// appends to), with the captain's fix on the review branch: it merged dev
    /// and resolved the conflict there, so the review branch contains the
    /// candidate.
    fn remerge_scratch(name: &str) -> Scratch {
        let scratch = Scratch::new(name);
        sh(&scratch.work, "git switch -q dev && printf 'dev\nother\n' > README.md && git -c user.email=a@b -c user.name=a commit -qam other");
        publish(&scratch.work, "dev");
        // The review branch merges dev, conflicts, and keeps the captain's
        // resolution.
        sh(
            &scratch.work,
            "git switch -q review/t5 && git merge -q --no-edit dev || true",
        );
        sh(
            &scratch.work,
            "printf 'dev\nother\ntask\n' > README.md && git add README.md \
             && git -c user.email=a@b -c user.name=a commit -qm resolution",
        );
        publish(&scratch.work, "review/t5");
        scratch
    }

    /// T-532 (decision 03:55): a remerge carries the captain's conflict fix on
    /// the review branch — the review branch contains the candidate, so the
    /// remerge merges only its commit: the candidate is inside it, and the
    /// candidate alone would conflict again.
    #[test]
    fn a_remerge_carries_the_captains_conflict_fix_on_the_review_branch() {
        let scratch = remerge_scratch("remerge-fix");
        let marker = scratch.root.join("gate-ran");
        let gate = format!("touch '{}'", marker.display());
        let candidate = scratch.head_of("heat/t5");
        let (mut store, mut herdr, cfg) = queued_merge_payload(
            &scratch,
            &gate,
            &candidate,
            serde_json::json!({
                "branch": "heat/t5",
                "candidate": candidate,
                "review": "review/t5",
                "remerge": true,
            }),
        );
        accept(&mut store, "T-5", 0);

        let merged = wait_merge(&mut store, &mut herdr, &cfg);
        assert_eq!(merged.len(), 1, "{merged:?}");
        assert_eq!(
            op_state(&mut store, "O-1"),
            Some(OpState::Confirmed),
            "the op is confirmed"
        );
        assert!(marker.exists(), "the gate ran");
        // dev holds the candidate and the captain's resolution.
        let log = scratch.dev_log();
        assert!(log.contains("task"), "the candidate landed: {log}");
        assert!(log.contains("resolution"), "the fix landed: {log}");
        assert!(
            log.contains("merge(T-5): origin/review/t5"),
            "the source names the review branch: {log}"
        );
        let readme = sh(&scratch.root, "git -C origin.git show dev:README.md");
        assert_eq!(readme, "dev\nother\ntask");
    }

    /// T-532: the same repos with a first merge op (no `remerge`) still fail:
    /// the candidate alone conflicts with dev.
    #[test]
    fn a_first_merge_of_the_same_repos_still_conflicts() {
        let scratch = remerge_scratch("remerge-first");
        let worktree = scratch.merge_worktree();
        let before = scratch.dev_log();
        let candidate = scratch.head_of("heat/t5");

        let failed = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap_err();

        assert!(
            failed
                .reason
                .contains(&format!("conflict merging {candidate}")),
            "{}",
            failed.reason
        );
        assert_eq!(failed.blocker, Blocker::Stuck);
        assert_eq!(scratch.dev_log(), before, "dev is untouched");
    }

    /// T-532: a remerge whose review branch does not contain the candidate
    /// merges the candidate, then the review branch, as today.
    #[test]
    fn a_remerge_whose_review_branch_lacks_the_candidate_merges_both_as_today() {
        let scratch = Scratch::new("remerge-lacks");
        // The review branch is cut from dev again and carries its own commit:
        // it does not contain the candidate.
        sh(
            &scratch.work,
            "git switch -q review/t5 && git reset -q --hard dev && echo reviewed > review.txt \
             && git add review.txt && git -c user.email=a@b -c user.name=a commit -qm reviewed",
        );
        publish(&scratch.work, "review/t5");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        let mut remerge = op(&candidate);
        remerge.payload["remerge"] = serde_json::json!(true);

        let line = merge_one(&worktree, &repo("true"), &remerge).unwrap();

        assert!(line.contains("merged T-5"), "{line}");
        let log = scratch.dev_log();
        assert!(log.contains("task"), "the candidate landed: {log}");
        assert!(log.contains("reviewed"), "the review landed: {log}");
        assert!(
            log.contains("merge(T-5): origin/review/t5"),
            "the review branch was merged: {log}"
        );
    }

    #[test]
    fn a_failing_gate_leaves_dev_untouched_and_fails() {
        let scratch = Scratch::new("gate");
        let worktree = scratch.merge_worktree();
        let before = scratch.dev_log();

        let failed = merge_one(
            &worktree,
            &repo("echo the gate said no >&2; exit 1"),
            &op(&scratch.head_of("heat/t5")),
        )
        .unwrap_err();

        assert!(failed.reason.contains("gate failed"), "{}", failed.reason);
        assert!(
            failed.reason.contains("the gate said no"),
            "{}",
            failed.reason
        );
        assert_eq!(
            failed.blocker,
            Blocker::Stuck,
            "the work is not good enough"
        );
        let state = scratch.state();
        assert_eq!(state.dev, before, "dev is untouched");
        assert!(!state.has_task_commit());
        let branches = &state.branches;
        assert!(branches.contains("heat/t5"), "{branches}");
        // The worktree is back at the integration branch (nothing half-merged
        // for the next run).
        let head = sh(Path::new(&worktree.path), "git log --format=%s -1");
        assert_eq!(head, "dev", "{head}");
    }

    /// T-219: a gate that failed names the failing tests. The tail of a long
    /// test run is slow-test timings, and the captain could not see what
    /// failed in it.
    #[test]
    fn a_failing_gate_names_the_failing_tests_before_the_tail() {
        let scratch = Scratch::new("gate-failures");
        // The FAIL line is far above 40 slow-test lines: the tail alone
        // would say nothing about what failed.
        let gate = "printf 'FAIL [t] the_broken_test\\n'; \
                   i=0; while [ $i -lt 40 ]; do printf 'test slow_%s ... ok (4.2s)\\n' $i; \
                   i=$((i+1)); done; \
                   printf 'error: the build failed\\n' >&2; exit 1";
        let (mut store, mut herdr, cfg) = queued_merge(&scratch, gate);
        accept(&mut store, "T-5", 0);

        let merged = wait_merge(&mut store, &mut herdr, &cfg);
        assert!(merged.is_empty(), "{merged:?}");
        let op = merge_op(&mut store);
        assert_eq!(op.state, OpState::Failed, "{op:?}");
        let reason = op.result.clone().unwrap_or_default();
        assert!(reason.contains("FAIL [t] the_broken_test"), "{reason}");
        // The tail is still there, after the failure lines.
        assert!(reason.contains("test slow_39 ... ok (4.2s)"), "{reason}");
        let failure = reason.find("FAIL [t] the_broken_test").unwrap();
        let tail_line = reason.find("test slow_39 ... ok (4.2s)").unwrap();
        assert!(
            failure < tail_line,
            "the failing line comes before the tail: {reason}"
        );

        // The captain's item carries the same reason.
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", 30))
            .unwrap();
        assert_eq!(inbox.len(), 1, "{inbox:?}");
        assert!(
            inbox[0].body.contains("FAIL [t] the_broken_test"),
            "{}",
            inbox[0].body
        );
    }

    /// T-563: a gate that pushed leaves the remote's chatter in its output;
    /// the refusal the captain reads comes first and no `remote:` line is kept
    /// (prometheus: the item led with push noise).
    #[test]
    fn a_failed_gate_keeps_its_refusal_and_drops_the_push_noise() {
        let text =
            "remote: \nremote: Create a pull request for 'p/heat/t1' on GitHub by visiting: \n\
                    remote:      https://github.com/o/r/pull/new/p/heat/t1\n\
                    To github.com:o/r.git\n\
                     * [new branch]      p/heat/t1 -> p/heat/t1\n\
                     - [deleted]         p/heat/t0\n\
                    make gate\n  FAIL [t] the_broken_test\n\
                    the judge refuses at judge: T-1 has no report\n";
        let kept = without_push_noise(text);
        assert!(!kept.contains("remote:"), "{kept}");
        assert!(!kept.contains("To github.com"), "{kept}");
        assert!(!kept.contains("[new branch]"), "{kept}");
        assert!(!kept.contains("[deleted]"), "{kept}");
        // The refusal is still what the captain reads first (T-219's lines).
        let failures = failure_lines(&kept, 20);
        assert_eq!(failures, vec!["  FAIL [t] the_broken_test"], "{failures:?}");
        assert!(tail(&kept, 12).ends_with("the judge refuses at judge: T-1 has no report"));
    }

    /// T-219: the failure lines are the ones that name a failure: a line that
    /// starts (after spaces) with FAIL, TIMEOUT or SIGSEGV, or one that holds
    /// `panicked at`, `error[` or `error:`.
    #[test]
    fn the_failure_lines_are_the_lines_that_name_a_failure() {
        let text = "running 3 tests\n  FAIL [t] one_test\n\
                   test ok ... ok (4.2s)\n\
                   TIMEOUT [t] other_test\n\
                   SIGSEGV [t] third_test\n\
                   thread panicked at src/lib.rs:12:5\n\
                   error[E0308]: mismatched types\n\
                   error: could not compile\n\
                   not a failure line\n";
        let failures = failure_lines(text, 20);
        assert_eq!(
            failures,
            vec![
                "  FAIL [t] one_test",
                "TIMEOUT [t] other_test",
                "SIGSEGV [t] third_test",
                "thread panicked at src/lib.rs:12:5",
                "error[E0308]: mismatched types",
                "error: could not compile",
            ]
        );
        // At most `max` of them come back.
        let many: String = (0..30).map(|n| format!("FAIL [t] test_{n}\n")).collect();
        assert_eq!(failure_lines(&many, 20).len(), 20);
    }

    /// T-41: the gate is the project's build and test command. It must not
    /// see the store of this machine: a test that inherited `GEAR_DB` would
    /// write to the production store (L-59).
    #[test]
    fn the_gate_runs_without_the_store_of_this_machine() {
        let command = gate_command(
            "cargo test",
            Path::new("/tmp"),
            "p",
            "p/dev",
            "o1",
            &["T-1".to_string()],
        );
        let integration = command
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new("GEAR_GATE_INTEGRATION"))
            .and_then(|(_, value)| value);
        assert_eq!(
            integration,
            Some(std::ffi::OsStr::new("p/dev")),
            "the gate is told its integration branch"
        );
        let removes = |name: &str| {
            command
                .get_envs()
                .filter(|(key, value)| *key == std::ffi::OsStr::new(name) && value.is_none())
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
        let (worktree, candidate) = scratch.prepared();
        // An unreviewed commit lands on the task branch.
        sh(
            &scratch.work,
            "git switch -q heat/t5 && echo sneaky > sneaky.txt && git add -A \
             && git -c user.email=a@b -c user.name=a commit -qm sneaky",
        );
        publish(&scratch.work, "heat/t5");

        let line = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        let log = scratch.dev_log();
        assert!(log.contains("task"), "{log}");
        assert!(log.contains("review"), "{log}");
        assert!(
            !log.contains("sneaky"),
            "the unreviewed commit is not on dev: {log}"
        );
        assert!(scratch.dev_has_task_commit());
        // T-474: the branch holds more than the candidate, so it is closed
        // anyway — a merged task leaves no branch behind — and the commit it
        // held is named to the captain.
        let branches = scratch.origin_branches();
        assert!(!branches.contains("heat/t5"), "{branches}");
        assert!(!branches.contains("review/t5"), "{branches}");
        assert!(
            line.contains(
                "closed on origin (their commits after the approved candidate were not merged):"
            ) && line.contains("sneaky"),
            "{line}"
        );
    }

    /// T-540 (M-7576): a task branch deleted on the remote between the merge's
    /// fetch and its close step is closed, not reported as left on the remote.
    #[test]
    fn a_branch_deleted_on_the_remote_before_the_close_is_closed() {
        let scratch = Scratch::new("close-gone");
        let candidate = scratch.head_of("heat/t5");
        // The gate runs between the fetch and the close: the branch is gone
        // from the remote meanwhile.
        let gate = "git --git-dir=../origin.git update-ref -d refs/heads/heat/t5";
        let (mut store, mut herdr, cfg) = queued_merge_candidate(&scratch, gate, &candidate);

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let merged = wait_merge(&mut store, &mut herdr, &cfg);
        assert_eq!(merged.len(), 1, "the candidate merged");
        assert!(scratch.dev_has_task_commit());
        let result = op_result(&mut store, "O-1");
        assert!(result.contains("closed on origin: "), "{result}");
        assert!(
            result.contains("heat/t5"),
            "the branch that was already gone is closed: {result}"
        );
        assert!(
            !result.contains("left on"),
            "no branch is reported as left on: {result}"
        );
        assert!(!scratch.origin_branches().contains("heat/t5"));
    }

    /// A runner whose delete of one branch fails while the branch stays
    /// (T-540): every other branch is deleted as [`FetchRemote`] does.
    #[derive(Debug)]
    struct KeepsBranchRemote(&'static str);

    impl Remote for KeepsBranchRemote {
        fn push(&self, dir: &Path, remote: &str, dst: &str) -> Result<(), String> {
            FetchRemote.push(dir, remote, dst)
        }

        fn delete(&self, dir: &Path, remote: &str, branch: &str) -> bool {
            if branch == self.0 {
                false
            } else {
                FetchRemote.delete(dir, remote, branch)
            }
        }
    }

    /// T-540: a branch whose delete fails while it still exists is still
    /// reported under "left on".
    #[test]
    fn a_branch_that_stays_after_a_failed_delete_is_left_on_the_remote() {
        let scratch = Scratch::new("close-kept");
        let candidate = scratch.head_of("heat/t5");
        let (mut store, mut herdr, mut cfg) = queued_merge_candidate(&scratch, "true", &candidate);
        cfg.remote = std::sync::Arc::new(KeepsBranchRemote("heat/t5"));

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let merged = wait_merge(&mut store, &mut herdr, &cfg);
        assert_eq!(merged.len(), 1, "the candidate merged");
        let result = op_result(&mut store, "O-1");
        assert!(result.contains("left on origin"), "{result}");
        assert!(
            result.contains("heat/t5"),
            "the branch that stays is named: {result}"
        );
        assert!(
            result.contains("closed on origin: review/t5"),
            "the review branch is closed as usual: {result}"
        );
        let branches = scratch.origin_branches();
        assert!(branches.contains("heat/t5"), "the branch stays: {branches}");
        assert!(!branches.contains("review/t5"), "{branches}");
    }

    /// T-402: lanes often push their report after the hand-in. A task branch
    /// whose only commits past the candidate write reports holds no work the
    /// review did not see: it is closed with the others, and the captain is
    /// not rung for it.
    #[test]
    fn a_branch_whose_only_commits_past_the_candidate_are_reports_is_closed() {
        let scratch = Scratch::new("report-extra");
        let candidate = scratch.head_of("heat/t5");
        let (mut store, mut herdr, cfg) = queued_merge_candidate(&scratch, "true", &candidate);
        // The lane pushes its report after the hand-in.
        sh(
            &scratch.work,
            "git switch -q heat/t5 && mkdir -p gear/voyages/M1/reports \
             && echo report > gear/voyages/M1/reports/T-5.md && git add -A \
             && git -c user.email=a@b -c user.name=a commit -qm report",
        );
        publish(&scratch.work, "heat/t5");

        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let merged = wait_merge(&mut store, &mut herdr, &cfg);
        assert_eq!(merged.len(), 1, "the candidate merged");
        // The branch is gone …
        let result = op_result(&mut store, "O-1");
        assert!(result.contains("closed on origin: "), "{result}");
        assert!(!result.contains("left on"), "{result}");
        let branches = scratch.origin_branches();
        assert!(!branches.contains("heat/t5"), "{branches}");
        assert!(!branches.contains("review/t5"), "{branches}");
        // … and the captain was not rung about it.
        let items = store
            .read(|tx| tx.messages_from_since("p", "p:captain", 0))
            .unwrap();
        assert!(
            items.is_empty(),
            "{:?}",
            items.iter().map(|item| &item.body).collect::<Vec<_>>()
        );
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
             && git add -A && git -c user.email=a@b -c user.name=a commit -qm rework",
        );
        publish(&scratch.work, "heat/t5-v2");
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
        assert_eq!(
            failed.blocker,
            Blocker::Clear,
            "a push that has not landed can clear"
        );
        assert_eq!(scratch.dev_log(), "dev", "dev is untouched");
    }

    #[test]
    fn a_settled_review_op_without_a_review_key_still_merges() {
        // F5 of the review: `tasks::settle_review` queues the op with `branch`
        // and `candidate` only. The missing review branch must be derived —
        // never `git merge --no-ff origin/`.
        let scratch = Scratch::new("settled");
        let (worktree, candidate) = scratch.prepared();
        let mut settled = op(&candidate);
        settled.payload =
            serde_json::json!({ "branch": "heat/t5", "candidate": candidate.clone() });

        let line = merge_one(&worktree, &repo("true"), &settled).unwrap();

        assert!(line.contains("merged T-5"), "{line}");
        let log = scratch.dev_log();
        assert!(log.contains("task"), "{log}");
        // The review branch exists in this fixture, so it was merged too.
        assert!(log.contains("review"), "{log}");
    }

    /// T-558: the review commit is pushed onto the task branch and no review
    /// branch exists on the remote: the merge takes the task branch, which
    /// holds the approved candidate and the review commit on top of it.
    #[test]
    fn a_review_on_the_task_branch_merges_candidate_and_review() {
        let scratch = Scratch::new("task-branch-review");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        // The reviewer's commit goes onto the task branch, and no review
        // branch is on the remote. Its subject is the project's review message
        // (T-564): anything else on the branch is a lane commit made after the
        // hand-in.
        sh(
            &scratch.work,
            "git switch -q heat/t5 && echo r > r5.txt && git add -A \
             && git -c user.name=a -c user.email=a@b commit -qm 'review(T-5): pass, r5'",
        );
        publish(&scratch.work, "heat/t5");
        sh(
            &scratch.root,
            "git -C origin.git update-ref -d refs/heads/review/t5",
        );

        let line = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap();

        assert!(line.contains("merged T-5"), "{line}");
        assert!(
            line.contains("origin/heat/t5"),
            "the task branch is the source: {line}"
        );
        let log = scratch.dev_log();
        assert!(log.contains("task"), "the candidate landed: {log}");
        assert!(log.contains("r5"), "the review commit landed: {log}");
        assert!(scratch.dev_has_task_commit());
        // The task branch is closed with the merge, as before.
        assert!(
            !scratch.origin_branches().contains("heat/t5"),
            "{:?}",
            scratch.origin_branches()
        );
    }

    /// T-564: a lane commit pushed onto the task branch after the hand-in is
    /// not merged (prometheus, 2026-11-04: hull's fix on T-562 would have
    /// landed unreviewed). The op fails `stuck` and names the commit.
    #[test]
    fn a_lane_commit_after_the_candidate_on_the_task_branch_is_refused() {
        let scratch = Scratch::new("task-branch-lane-commit");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        // The review, then the lane's own fix on top of it.
        sh(
            &scratch.work,
            "git switch -q heat/t5 && echo r > r5.txt && git add -A \
             && git -c user.name=a -c user.email=a@b commit -qm 'review(T-5): pass' \
             && echo w > w5.txt && git add -A \
             && git -c user.name=a -c user.email=a@b commit -qm 'fix the thing after hand-in'",
        );
        publish(&scratch.work, "heat/t5");
        sh(
            &scratch.root,
            "git -C origin.git update-ref -d refs/heads/review/t5",
        );
        let lane_commit = scratch.head_of("heat/t5");

        let failed = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap_err();

        assert_eq!(failed.blocker, Blocker::Stuck, "{}", failed.reason);
        assert!(
            failed.reason.contains(&lane_commit),
            "the lane commit is named: {}",
            failed.reason
        );
        assert!(
            failed.reason.contains("fix the thing after hand-in"),
            "{}",
            failed.reason
        );
        assert!(
            failed
                .reason
                .contains("a change after hand-in needs a new attempt"),
            "{}",
            failed.reason
        );
        // Nothing landed: dev is where it was.
        assert!(!scratch.dev_log().contains("fix the thing"));
    }

    /// T-564: the lane commit merged in under a merge commit is caught too: the
    /// range keeps every commit, and the merge commit's own subject is no pass.
    #[test]
    fn a_pulled_in_lane_commit_under_a_merge_commit_is_refused() {
        let scratch = Scratch::new("task-branch-merge-commit");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        sh(
            &scratch.work,
            "git switch -q heat/t5 && echo r > r5.txt && git add -A \
             && git -c user.name=a -c user.email=a@b commit -qm 'review(T-5): pass' \
             && git checkout -q -b lane-side \
             && echo w > w5.txt && git add -A \
             && git -c user.name=a -c user.email=a@b commit -qm 'lane fix' \
             && git switch -q heat/t5 \
             && git -c user.name=a -c user.email=a@b merge -q --no-ff lane-side -m 'review(T-5): pass'",
        );
        publish(&scratch.work, "heat/t5");
        sh(
            &scratch.root,
            "git -C origin.git update-ref -d refs/heads/review/t5",
        );

        let failed = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap_err();

        assert_eq!(failed.blocker, Blocker::Stuck, "{}", failed.reason);
        assert!(
            failed.reason.contains("lane fix"),
            "the pulled-in commit is named: {}",
            failed.reason
        );
    }

    /// T-564 review F1: the documented conflict fix is `merge dev into the
    /// task branch, resolve, push, remerge` (course.md). That merge brings the
    /// integration branch's own commits into the range, and they are reviewed
    /// work already on the branch, not lane commits: the merge goes through.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_task_branch_that_merged_the_integration_branch_still_merges() {
        let scratch = Scratch::new("task-branch-dev-merge");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        // The integration branch moves after the hand-in, and the task branch
        // takes it in: the captain's conflict fix, with nothing to resolve.
        sh(
            &scratch.work,
            "git switch -q dev && echo d > devd.txt && git add -A \
             && git -c user.name=a -c user.email=a@b commit -qm 'dev moved'",
        );
        publish(&scratch.work, "dev");
        sh(
            &scratch.work,
            "git switch -q heat/t5 && echo r > r5.txt && git add -A \
             && git -c user.name=a -c user.email=a@b commit -qm 'review(T-5): pass' \
             && git -c user.name=a -c user.email=a@b merge -q --no-edit dev",
        );
        publish(&scratch.work, "heat/t5");
        sh(
            &scratch.root,
            "git -C origin.git update-ref -d refs/heads/review/t5",
        );

        let line = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap();

        assert!(line.contains("merged T-5"), "{line}");
        let log = scratch.dev_log();
        assert!(log.contains("review(T-5)"), "the review landed: {log}");
        assert!(log.contains("dev moved"), "dev's commit is there: {log}");
    }

    /// T-558, the transition: a task reviewed before the change still has its
    /// review branch on the remote, and that one merges exactly as it did.
    #[test]
    fn an_old_review_branch_still_on_the_remote_merges_as_before() {
        let scratch = Scratch::new("old-review-branch");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        // The task branch has moved on since the hand-in; the review is on the
        // review branch, as reviews of before T-558 were.
        sh(
            &scratch.work,
            "git switch -q heat/t5 && echo late > late.txt && git add -A \
             && git -c user.name=a -c user.email=a@b commit -qm late",
        );
        publish(&scratch.work, "heat/t5");

        let line = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap();

        assert!(
            line.contains("origin/review/t5"),
            "the review branch is the source: {line}"
        );
        let log = scratch.dev_log();
        assert!(log.contains("task"), "the candidate landed: {log}");
        assert!(log.contains("review"), "the review landed: {log}");
        assert!(
            !log.contains("late"),
            "work pushed after the hand-in is not merged: {log}"
        );
    }

    #[test]
    fn a_missing_task_branch_does_not_publish_a_partial_merge() {
        // F6 of the review, for T-558: the review commit is pushed onto the
        // task branch, so the task branch is what a merge publishes. One that
        // is gone on origin (and no old review branch either) is not silently
        // skipped: the merge fails with the git diagnostic, dev stays where it
        // was, and nothing is pushed.
        let scratch = Scratch::new("noreview");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        // The task branch and the old review branch are gone; the approved
        // candidate is still on a pushed ref of its own.
        sh(
            &scratch.root,
            &format!(
                "git -C origin.git update-ref refs/heads/keep/t5 {candidate} \
                 && git -C origin.git update-ref -d refs/heads/review/t5 \
                 && git -C origin.git update-ref -d refs/heads/heat/t5"
            ),
        );
        let before = scratch.dev_log();

        let failed = merge_one(&worktree, &repo("true"), &op(&candidate)).unwrap_err();

        assert_eq!(failed.blocker, Blocker::Stuck, "it does not clear");
        assert!(
            failed.reason.contains("task branch") && failed.reason.contains("origin/heat/t5"),
            "the missing branch is named: {}",
            failed.reason
        );
        assert_eq!(scratch.dev_log(), before, "dev is untouched");
        assert!(!scratch.dev_has_task_commit(), "no partial merge on dev");
        // The worktree is back at the integration branch (nothing half-merged
        // for the next run).
        let head = sh(Path::new(&worktree.path), "git log --format=%s -1");
        assert_eq!(head, "dev", "{head}");
    }

    #[test]
    fn a_missing_task_branch_fails_the_op_and_tells_the_captain() {
        // The public path (T-558): the engine's merge step runs, the task
        // branch is gone on origin and there is no old review branch, so the
        // op fails with the git diagnostic, dev is untouched and the captain
        // gets one item — as the missing review branch did before.
        let scratch = Scratch::new("noreview-op");
        let (worktree, candidate) = scratch.prepared();
        // The task branch and the old review branch are gone before the merge
        // runs, and the candidate is on a ref of its own, so the op reaches the
        // merge's own check rather than the "never pushed" one.
        sh(
            &scratch.root,
            &format!(
                "git -C origin.git update-ref refs/heads/keep/t5 {candidate} \
                 && git -C origin.git update-ref -d refs/heads/review/t5 \
                 && git -C origin.git update-ref -d refs/heads/heat/t5"
            ),
        );
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
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

        let mut cfg = test_cfg();
        cfg.profiles
            .insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let merged = wait_merge(&mut store, &mut herdr, &cfg);

        assert!(merged.is_empty(), "{merged:?}");
        let op = merge_op(&mut store);
        assert_eq!(op.state, OpState::Failed, "{op:?}");
        let reason = op.result.clone().unwrap_or_default();
        assert!(reason.contains("task branch"), "{reason}");
        assert!(reason.contains("origin/heat/t5"), "{reason}");
        // dev is untouched: no partial merge.
        assert_eq!(scratch.dev_log(), "dev", "dev is untouched");
        assert!(!scratch.dev_has_task_commit());
        // One item in the captain's inbox.
        let inbox = store
            .read(|tx| tx.actionable_for("p", "p:captain", op.updated + 1))
            .unwrap();
        assert_eq!(inbox.len(), 1, "{inbox:?}");
        assert!(inbox[0].body.contains("task branch"), "{}", inbox[0].body);
    }

    #[test]
    fn a_mechanical_task_merges_without_a_review_branch() {
        // T-405: a mechanically settled task has no reviewer, so origin never
        // had a review branch for it. Its merge op carries `"mechanical":
        // true`, and the pinned candidate merges into dev alone.
        let scratch = Scratch::new("mechanical");
        sh(
            &scratch.root,
            "git -C origin.git update-ref -d refs/heads/review/t5",
        );
        let (worktree, candidate) = scratch.prepared();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
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
                // T-273: the engine settles a mechanical hand-in itself; the
                // note of its verdict says so.
                tx.insert_verdict(&gear_store::VerdictRow {
                    project: "p".into(),
                    task: "T-5".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: candidate.clone(),
                    passed: true,
                    evidence: Some("mechanical: the task is a rename".into()),
                    by_seat: "engine".into(),
                    created: 10,
                })?;
                Ok(())
            })
            .unwrap();
        accept(&mut store, "T-5", 10);

        let mut cfg = test_cfg();
        cfg.profiles
            .insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let merged = wait_merge(&mut store, &mut herdr, &cfg);

        assert_eq!(merged.len(), 1, "{merged:?}");
        let op = merge_op(&mut store);
        assert_eq!(op.state, OpState::Confirmed, "{op:?}");
        assert_eq!(
            op.payload["mechanical"],
            serde_json::json!(true),
            "the op of a mechanically settled attempt says so"
        );
        let log = scratch.dev_log();
        assert!(log.contains("task"), "the candidate landed: {log}");
        assert!(
            !log.contains("review"),
            "no review branch was merged: {log}"
        );
        assert!(scratch.dev_has_task_commit());
        let branches = scratch.origin_branches();
        assert!(
            !branches.contains("heat/t5"),
            "the task branch is closed: {branches}"
        );
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
            second
                .lines()
                .filter(|line| line.starts_with("merge(T-5)"))
                .count(),
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
        let (worktree, candidate) = scratch.prepared();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
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

        let mut cfg = test_cfg();
        cfg.profiles
            .insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
        let before = wall_time();
        merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();
        let merged = wait_merge(&mut store, &mut herdr, &cfg);

        assert_eq!(merged.len(), 1, "{merged:?}");
        let times = store.read(|tx| tx.attempt_times("p", "T-5", 1)).unwrap();
        let completed = store
            .read(|tx| tx.get_op("p", "O-1"))
            .unwrap()
            .unwrap()
            .updated;
        assert_eq!(times.merged, Some(completed), "the merge has its own time");
        assert!(completed >= before && completed <= wall_time());
        // The other steps keep theirs.
        assert_eq!(times.started, Some(0));
        assert_eq!(times.handed_in, Some(0));
    }
    #[test]
    fn passed_work_still_merges_while_the_captain_is_on_quota() {
        let scratch = Scratch::new("captain-quota");
        let (worktree, candidate) = scratch.prepared();
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);
        herdr.add_agent("w9:p1", "claude", gear_herdr::AgentStatus::Idle);
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen("w9:p1", "────────────────\n❯\n────────────────\nOpus 5.5 · medium /repo (dev) · ctx 63% · 5h 100%\n⏵⏵ bypass permissions on");
        let mut store = world(TaskState::Ready);
        store
            .write(|tx| {
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-5", 1, 1, &candidate, "report", 1)?;
                tx.insert_verdict(&gear_store::VerdictRow {
                    project: "p".into(),
                    task: "T-5".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate,
                    passed: true,
                    evidence: Some("review".into()),
                    by_seat: "p:captain".into(),
                    created: 10,
                })
            })
            .unwrap();
        let mut cfg = test_cfg();
        for kind in ["opencode", "claude"] {
            let mut profile = gear_herdr::builtin(kind).unwrap();
            profile.confirm_rounds = 1;
            cfg.profiles.insert(kind.into(), profile);
        }
        loop {
            // Wait on the merge worker's own report between two ticks.
            let seen = cfg.merges.progress();
            let report = crate::tick(&mut store, &mut herdr, &cfg, 20).unwrap();
            if !report.merged.is_empty() {
                break;
            }
            assert!(
                !cfg.merges.running("p") || cfg.merges.wait_progress(seen, WAIT),
                "the merge worker did not report"
            );
        }
        assert!(store.read(|tx| tx.captain_quota_paused("p")).unwrap());
        assert!(scratch.dev_has_task_commit());
        assert!(scratch.dev_log().contains("review"));
        assert!(
            herdr.prompts_to("w9:p1").is_empty(),
            "the unavailable captain is not rung"
        );
    }

    /// The three regressions of the review of attempt 1 (F1-F3): the landing must
    /// survive a restart, a refusal the captain retries, and a push whose answer
    /// was lost.
    use gear_herdr::fake::FakeHerdr;
    use gear_model::OpKind;

    /// A project in pull-request mode with one passed task (T-5) whose
    /// candidate is `candidate`, and the claim pull request #7.
    fn pr_setup(candidate: &str) -> (Store, FakeHerdr, EngineCfg) {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&gear_store::ProjectRow {
                    id: "p".into(),
                    herdr_session: "sess".into(),
                    config_toml: pr_config_text("true"),
                    config_version: 1,
                    quota_lanes: 1,
                    quota_temps: 0,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: 0,
                })?;
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "heat",
                    gear_model::SeatRole::Lane,
                    "w1:p1",
                    "opencode",
                    0,
                )?;
                gear_store::testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    gear_model::SeatRole::Captain,
                    "w9:p1",
                    "claude",
                    0,
                )?;
                tx.insert_task(&gear_store::TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-5".into(),
                    rank: 1,
                    state: TaskState::Ready,
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
                    issue: Some(3),
                    updated: 0,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-5", "p:heat", 1, 0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-5", 1, 0)?;
                gear_app::tasks::submit_result(tx, "p", "T-5", 1, 1, candidate, "report", 0)?;
                tx.insert_verdict(&gear_store::VerdictRow {
                    project: "p".into(),
                    task: "T-5".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: candidate.into(),
                    passed: true,
                    evidence: Some("ops/reviews/T-5.md".into()),
                    by_seat: "p:captain".into(),
                    created: 10,
                })?;
                Ok(())
            })
            .unwrap();
        claim_op(&mut store, "T-5", 7);
        let herdr = FakeHerdr::new();
        let mut cfg = EngineCfg {
            remote: std::sync::Arc::new(FetchRemote),
            merge_retry_wait: RetryWait::immediate(),
            ..Default::default()
        };
        for kind in ["opencode", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }
        (store, herdr, cfg)
    }

    /// A merge op for the task, in the state a restart leaves it in.
    fn merge_op_in(store: &mut Store, candidate: &str, state: OpState) -> String {
        let op = store
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
                        "candidate": candidate,
                        "review": "review/t5",
                    }),
                    created: 0,
                })
            })
            .unwrap();
        store
            .write(|tx| tx.set_op_state("p", &op.id, state, None, 0))
            .unwrap();
        op.id
    }

    /// The land ops of the task: the forge ops that merge its pull request.
    fn land_ops(store: &mut Store) -> Vec<gear_store::OpRow> {
        store
            .read(|tx| tx.ops_of_seat_kind("p:forge", OpKind::Forge))
            .unwrap()
            .into_iter()
            .filter(|op| op.payload.get("forge").and_then(|forge| forge.as_str()) == Some("land"))
            .collect()
    }

    /// The state of a merge op.
    fn state_of(store: &mut Store, id: &str) -> OpState {
        store.read(|tx| tx.get_op("p", id)).unwrap().unwrap().state
    }

    /// The result of a merge op.
    fn result_of(store: &mut Store, id: &str) -> String {
        store
            .read(|tx| tx.get_op("p", id))
            .unwrap()
            .unwrap()
            .result
            .unwrap_or_default()
    }

    /// One merge step and the wait for the worker it launched.
    fn pr_tick(store: &mut Store, herdr: &mut FakeHerdr, cfg: &EngineCfg, now: Ts) {
        let seen = cfg.merges.progress();
        merge_passed(store, herdr, cfg, "p", now).unwrap();
        if cfg.merges.running("p") {
            assert!(
                cfg.merges
                    .wait_progress(seen, std::time::Duration::from_secs(60)),
                "the merge worker did not report"
            );
        }
    }

    /// F1: a refused land is retried by the captain's `- remerge T-5`: a new
    /// land op is inserted (the old one failed), the forge is called again,
    /// and the merge is confirmed.
    #[test]
    fn a_refused_land_is_retried_by_a_remerge() {
        let scratch = Scratch::new("pr-remerge");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        let (mut store, mut herdr, cfg) = pr_setup(&candidate);
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);

        // The merge runs and the pull request's head is pushed.
        pr_tick(&mut store, &mut herdr, &cfg, 20);
        pr_tick(&mut store, &mut herdr, &cfg, 21);

        // The forge refuses the landing.
        let refusing = FakeForge::refusing("not mergeable: required checks are missing");
        gear_forge::sync_with(&mut store, "p", &refusing, 30).unwrap();
        assert_eq!(refusing.calls(), 1, "the forge was asked once");
        assert_eq!(land_ops(&mut store).len(), 1);
        assert_eq!(land_ops(&mut store)[0].state, OpState::Failed);

        // The captain remerges: a new merge op for the same attempt (what
        // `- remerge T-5` in the plan queues).
        let remerge = merge_op_in(&mut store, &candidate, OpState::Intended);
        pr_tick(&mut store, &mut herdr, &cfg, 40);
        pr_tick(&mut store, &mut herdr, &cfg, 41);

        // A new land op was inserted: the old one failed.
        let lands = land_ops(&mut store);
        assert_eq!(lands.len(), 2, "a new land op: {lands:?}");
        assert_eq!(lands[1].state, OpState::Intended, "{lands:?}");

        // The forge merges this time, and the merge is confirmed.
        let merging = FakeForge::merging();
        gear_forge::sync_with(&mut store, "p", &merging, 50).unwrap();
        assert_eq!(merging.calls(), 1, "the forge was asked again");
        pr_tick(&mut store, &mut herdr, &cfg, 60);
        assert_eq!(
            state_of(&mut store, &remerge),
            OpState::Confirmed,
            "{:?}",
            result_of(&mut store, &remerge)
        );
        assert!(
            result_of(&mut store, &remerge).contains("by PR #7"),
            "{}",
            result_of(&mut store, &remerge)
        );
        assert!(store
            .read(|tx| tx.attempt_times("p", "T-5", 1))
            .unwrap()
            .merged
            .is_some());
    }

    /// F2: a merge op the engine recovered (Unknown) becomes Submitted after
    /// the worker, and the merge is confirmed when the land op is.
    #[test]
    fn a_recovered_merge_op_becomes_submitted_and_confirms() {
        let scratch = Scratch::new("pr-recovered");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        let (mut store, mut herdr, cfg) = pr_setup(&candidate);
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);

        // The op is Unknown: the engine stopped while its worker ran.
        let id = merge_op_in(&mut store, &candidate, OpState::Unknown);
        pr_tick(&mut store, &mut herdr, &cfg, 20);
        pr_tick(&mut store, &mut herdr, &cfg, 21);

        // The recovered op carries the result of a pull-request merge.
        assert_eq!(
            state_of(&mut store, &id),
            OpState::Submitted,
            "{:?}",
            result_of(&mut store, &id)
        );
        let result: serde_json::Value = serde_json::from_str(&result_of(&mut store, &id)).unwrap();
        assert_eq!(result["mode"], "pr", "{result}");
        assert_eq!(result["pr"], 7, "{result}");
        // One land op: the one the merge inserted.
        assert_eq!(land_ops(&mut store).len(), 1);

        // The forge merges, and the next tick confirms the merge.
        gear_forge::sync_with(&mut store, "p", &FakeForge::merging(), 30).unwrap();
        pr_tick(&mut store, &mut herdr, &cfg, 40);
        assert_eq!(
            state_of(&mut store, &id),
            OpState::Confirmed,
            "{:?}",
            result_of(&mut store, &id)
        );
        assert!(store
            .read(|tx| tx.attempt_times("p", "T-5", 1))
            .unwrap()
            .merged
            .is_some());
    }

    /// F3: a head that was pushed and whose answer was lost is reused: the
    /// retry does not build a sibling merge commit and does not fail with a
    /// non-fast-forward.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn a_pushed_head_is_reused_after_a_restart() {
        let scratch = Scratch::new("pr-reuse");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        let (mut store, mut herdr, cfg) = pr_setup(&candidate);
        herdr.add_worktree("p/merge/between-voyages", &worktree.path, &worktree.pane_id);

        let id = merge_op_in(&mut store, &candidate, OpState::Intended);
        pr_tick(&mut store, &mut herdr, &cfg, 20);
        pr_tick(&mut store, &mut herdr, &cfg, 21);
        assert_eq!(
            state_of(&mut store, &id),
            OpState::Submitted,
            "{:?}",
            result_of(&mut store, &id)
        );
        let pushed = scratch.head_of("heat/t5");
        let result: serde_json::Value = serde_json::from_str(&result_of(&mut store, &id)).unwrap();
        assert_eq!(result["head"], pushed.as_str(), "{result}");

        // The head on the branch is rewritten with another timestamp: the same
        // tree and the same parents, another sha — what a lost answer leaves
        // behind.
        let older = sh(
            &scratch.root,
            "set -e\n\
         tree=$(git -C origin.git rev-parse 'heat/t5^{tree}')\n\
         p1=$(git -C origin.git rev-parse 'heat/t5^1')\n\
         p2=$(git -C origin.git rev-parse 'heat/t5^2')\n\
         new=$(GIT_AUTHOR_DATE='2001-01-01T00:00:00Z' GIT_COMMITTER_DATE='2001-01-01T00:00:00Z' \
           git -C origin.git commit-tree $tree -p $p1 -p $p2 -m 'merge(T-5): origin/review/t5')\n\
         git -C origin.git update-ref refs/heads/heat/t5 $new\n\
         echo $new",
        );
        let older = older.trim().to_string();
        assert_ne!(older, pushed, "the head on the branch has another sha");

        // The engine starts again: the op is Unknown, the merge runs again.
        store
            .write(|tx| tx.set_op_state("p", &id, OpState::Unknown, None, 30))
            .unwrap();
        pr_tick(&mut store, &mut herdr, &cfg, 40);
        pr_tick(&mut store, &mut herdr, &cfg, 41);

        // The head on the branch is reused: no non-fast-forward, no new sha.
        assert_eq!(
            state_of(&mut store, &id),
            OpState::Submitted,
            "{:?}",
            result_of(&mut store, &id)
        );
        let result: serde_json::Value = serde_json::from_str(&result_of(&mut store, &id)).unwrap();
        assert_eq!(
            result["head"],
            older.as_str(),
            "the head on the branch is reused: {result}"
        );
        assert_eq!(scratch.head_of("heat/t5"), older, "nothing was pushed");
    }

    #[test]
    #[should_panic(expected = "did not happen within")]
    fn a_fifo_handshake_that_never_happens_gives_up_instead_of_hanging() {
        let scratch = Scratch::new("fifo-bound");
        // Nobody opens the other end of this fifo: the await gives up at its
        // bound. The bound is what is tested, so it is short.
        let running = fifo(&scratch.root, "nobody-running");
        await_fifo_within(&running, std::time::Duration::from_millis(50));
    }

    /// T-395: a merge that lost its push race (Unknown) is not starved by newer
    /// merges. It runs in the same pass as the Intended ops, in the order the ops
    /// were created, rather than after every Intended op there will ever be.
    #[test]
    fn an_unknown_merge_is_not_starved_by_newer_intended_ones() {
        let mut store = gear_store::Store::open_in_memory().unwrap();
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
                })
            })
            .unwrap();

        // An op of the given kind, state and creation time; its id is the store's.
        fn queue(
            store: &mut gear_store::Store,
            kind: OpKind,
            state: OpState,
            created: Ts,
        ) -> String {
            let op = store
                .write(|tx| {
                    tx.insert_op(&gear_store::NewOp {
                        project: "p".into(),
                        seat: "p:heat".into(),
                        generation: 1,
                        kind,
                        task: Some("T-5".into()),
                        attempt: Some(1),
                        ref_msg: None,
                        plan_version: None,
                        payload: serde_json::json!({
                            "branch": "heat/t5",
                            "candidate": "abc1234",
                            "review": "review/t5",
                        }),
                        created,
                    })
                })
                .unwrap();
            if state != OpState::Intended {
                store
                    .write(|tx| tx.set_op_state("p", &op.id, state, None, created))
                    .unwrap();
            }
            op.id
        }

        // Two newer merges are queued; the older merge that lost its race is
        // Unknown. Another op is queued too: it is no merge and stays out.
        let newer_a = queue(&mut store, OpKind::Merge, OpState::Intended, 200);
        let newer_b = queue(&mut store, OpKind::Merge, OpState::Intended, 300);
        let raced = queue(&mut store, OpKind::Merge, OpState::Unknown, 100);
        let _other = queue(&mut store, OpKind::Doorbell, OpState::Intended, 50);

        let ready: Vec<String> = ready_merges(&mut store, "p")
            .unwrap()
            .into_iter()
            .map(|op| op.id)
            .collect();

        assert_eq!(
            ready,
            vec![raced, newer_a, newer_b],
            "oldest first, the raced one not starved"
        );
    }

    // ----------------------------------------------------------------
    // T-582: a feature's members merge into the feature's own branch, with
    // their own check; the feature reaches the integration branch through its
    // integration task's merge, which leaves the feature branch on the remote.
    // ----------------------------------------------------------------

    /// Put a feature branch on the fixture's origin: `base` plus one commit of
    /// its own, and return its tip (T-582).
    fn feature_branch_on(work: &Path, base: &str) -> String {
        sh(work, "git fetch -q origin");
        sh(
            work,
            &format!(
                "git switch -qC p/feature/f-1 origin/{base} && printf 'feature\\n' > f.txt && \
                 git add -A && git -c user.email=a@b -c user.name=a commit -qm 'feature'"
            ),
        );
        publish(work, "p/feature/f-1");
        sh(work, "git rev-parse origin/p/feature/f-1")
    }

    /// A store whose one task is Verifying with a passed attempt at
    /// `candidate`, carrying `base`/`gate` and, when `feature` names one, the
    /// task branch override (T-582).
    #[allow(clippy::too_many_arguments)]
    fn feature_store(
        repo: &Path,
        config_gate: &str,
        task: &str,
        base: Option<&str>,
        task_gate: Option<&str>,
        feature: Option<&str>,
        candidate: &str,
    ) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&gear_store::ProjectRow {
                    id: "p".into(),
                    herdr_session: "sess".into(),
                    config_toml: config_text_in(config_gate, repo),
                    config_version: 1,
                    quota_lanes: 1,
                    quota_temps: 0,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: 0,
                })?;
                for (name, role, pane) in [
                    ("heat", gear_model::SeatRole::Lane, "w1:p1"),
                    ("captain", gear_model::SeatRole::Captain, "w9:p1"),
                ] {
                    gear_store::testing::seed_active_seat(
                        tx, "p", name, role, pane, "opencode", 0,
                    )?;
                }
                tx.insert_task(&gear_store::TaskRow {
                    repo: None,
                    base: base.map(str::to_string),
                    gate: task_gate.map(str::to_string),
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: task.into(),
                    rank: 1,
                    state: TaskState::Verifying,
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
                    issue: None,
                    updated: 0,
                })?;
                tx.insert_attempt(&gear_store::AttemptRow {
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                    project: "p".into(),
                    task: task.into(),
                    n: 1,
                    seat: "p:heat".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: gear_model::AttemptState::Accepted,
                    candidate: Some(candidate.into()),
                    report: None,
                    safe_point: None,
                    started: 0,
                    updated: 0,
                })?;
                tx.insert_verdict(&gear_store::VerdictRow {
                    project: "p".into(),
                    task: task.into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: candidate.into(),
                    passed: true,
                    evidence: Some("ops/reviews/T-5.md".into()),
                    by_seat: "p:captain".into(),
                    created: 0,
                })?;
                if let Some(branch) = feature {
                    tx.set_task_branch_override("p", task, branch)?;
                }
                Ok(())
            })
            .unwrap();
        store
    }

    /// The merge op `merge_passed` queued for the project, in whatever state
    /// its worker has moved it to (the payload is what the tests read).
    fn queued_merge_op(store: &mut Store) -> OpRow {
        store
            .read(|tx| tx.ops_of_kind_in_states("p", OpKind::Merge, &[]))
            .unwrap()
            .pop()
            .expect("a merge op is queued")
    }

    /// The config the engine reads for the fixture (T-582).
    fn parsed_config(gate: &str, work: &Path) -> ProjectConfig {
        gear_app::config::parse(&config_text_in(gate, work)).unwrap()
    }

    /// The branches the fixture's origin still has.
    fn origin_branches(root: &Path) -> String {
        sh(
            root,
            "git -C origin.git for-each-ref --format='%(refname:short)' refs/heads",
        )
    }

    /// T-582: an integration task's merge merges the feature's branch into the
    /// integration branch, and the close keeps that branch on the remote.
    #[test]
    #[ignore = "T-601: a whole merge pass over real git, ~120 processes; no wait or sleep to cut, `make full` runs it"]
    fn an_integration_task_merges_the_feature_branch_and_keeps_it() {
        let scratch = Scratch::new("feature-integration");
        let candidate = feature_branch_on(&scratch.work, "review/t5");
        let mut store = feature_store(
            &scratch.work,
            "true",
            "T-5",
            None,
            None,
            Some("p/feature/f-1"),
            &candidate,
        );
        let config = parsed_config("true", &scratch.work);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        merge_passed(&mut store, &mut herdr, &test_cfg(), "p", 10).unwrap();
        let op = queued_merge_op(&mut store);
        assert_eq!(
            op.payload["branch"], "p/feature/f-1",
            "the merge names the feature's branch: {:?}",
            op.payload
        );
        assert_eq!(op.payload["keep_branch"], true, "{:?}", op.payload);
        let repo = repo_of_op(&mut store, "p", &op, &config).unwrap();
        assert_eq!(repo.integration, "dev", "the integration task lands on dev");

        let worktree = scratch.merge_worktree();
        let line = merge_one(&worktree, &repo, &op).unwrap();

        assert!(line.contains("merged T-5 into dev"), "{line}");
        let dev = scratch.dev_log();
        assert!(dev.contains("task"), "the feature's work is on dev: {dev}");
        assert!(dev.contains("feature"), "{dev}");
        assert!(!line.contains("closed on origin: p/feature/f-1"), "{line}");
        assert_eq!(
            scratch.head_of("p/feature/f-1"),
            candidate,
            "the feature's branch is still on the remote"
        );
    }

    /// T-582: a member of a feature merges into the feature's branch, and its
    /// own gate (`true`) runs instead of the project's — which fails here, so a
    /// merge that ran it could not land.
    #[test]
    fn a_member_merges_into_the_feature_branch_without_the_project_gate() {
        let pair = Pair::new("feature-member", false);
        feature_branch_on(&pair.work, "dev");
        let mut store = feature_store(
            &pair.work,
            "false",
            "T-1",
            Some("p/feature/f-1"),
            Some("true"),
            None,
            &pair.a,
        );
        let config = parsed_config("false", &pair.work);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        merge_passed(&mut store, &mut herdr, &test_cfg(), "p", 10).unwrap();
        let op = queued_merge_op(&mut store);
        assert_eq!(op.payload["branch"], "heat/t1", "{:?}", op.payload);
        assert!(op.payload.get("keep_branch").is_none(), "{:?}", op.payload);
        let repo = repo_of_op(&mut store, "p", &op, &config).unwrap();
        assert_eq!(repo.integration, "p/feature/f-1", "the member lands there");
        assert_eq!(
            repo.gate, "true",
            "the member's own check, not the project's"
        );

        let worktree = pair.merge_worktree();
        let line = merge_one(&worktree, &repo, &op).unwrap();

        assert!(line.contains("merged T-1 into p/feature/f-1"), "{line}");
        let feature = sh(
            &pair.root,
            "git -C origin.git log --format=%s p/feature/f-1",
        );
        assert!(
            feature.contains('a'),
            "the member's work is there: {feature}"
        );
        assert!(feature.contains("r1"), "{feature}");
        assert_eq!(
            pair.dev_log(),
            "dev",
            "dev is untouched by a member's merge"
        );
        let branches = origin_branches(&pair.root);
        assert!(!branches.contains("heat/t1"), "{branches}");
    }

    /// T-582: a member whose work conflicts with the feature's branch is
    /// refused as any conflict is, and the branch is not moved.
    #[test]
    fn a_conflicting_member_is_refused_and_leaves_the_feature_branch() {
        let scratch = Scratch::new("feature-conflict");
        // The feature branch appends to the README where the task's commit
        // does: the two cannot merge.
        sh(
            &scratch.work,
            "git fetch -q origin && git switch -qC p/feature/f-1 origin/dev && \
             printf 'dev\\nfeature\\n' > README.md && git add -A && \
             git -c user.email=a@b -c user.name=a commit -qm feature",
        );
        publish(&scratch.work, "p/feature/f-1");
        let before = sh(&scratch.work, "git rev-parse origin/p/feature/f-1");
        let mut store = feature_store(
            &scratch.work,
            "true",
            "T-5",
            Some("p/feature/f-1"),
            Some("true"),
            None,
            &scratch.head_of("heat/t5"),
        );
        let config = parsed_config("true", &scratch.work);
        let mut herdr = gear_herdr::fake::FakeHerdr::new();
        merge_passed(&mut store, &mut herdr, &test_cfg(), "p", 10).unwrap();
        let op = queued_merge_op(&mut store);
        let repo = repo_of_op(&mut store, "p", &op, &config).unwrap();

        let worktree = scratch.merge_worktree();
        let failed = merge_one(&worktree, &repo, &op).unwrap_err();

        assert!(
            failed.reason.contains("conflict merging"),
            "{}",
            failed.reason
        );
        assert_eq!(
            scratch.head_of("p/feature/f-1"),
            before,
            "the feature's branch is where it was"
        );
        assert!(origin_branches(&scratch.root).contains("heat/t5"));
    }
}
