//! The engine merges a passed task (design/08, conventions 17 §8, plan T-12).
//!
//! A task whose current attempt has a passing verdict and that has no merge op
//! yet gets one `merge` op. The engine executes it in its own integration
//! worktree (`home/merge`, created once from the integration branch, reopened
//! afterwards):
//!
//! 1. reset to `origin/<integration>`: a half-done earlier run is discarded,
//!    and a retry of the same op cannot merge twice;
//! 2. `git merge --no-ff origin/<task branch>`, then `origin/review/t<n>`;
//! 3. refuse a merge that left conflict markers, abort a conflicted merge;
//! 4. run the project's gate ([`Conventions::gate`]) in that worktree;
//! 5. push `<integration>`, then delete the task and review branches on origin.
//!
//! Any failure before the push leaves the remote integration branch untouched
//! (`git reset --hard origin/<integration>`), records the op Failed with the
//! output tail and puts one item in the captain's inbox. An op the previous
//! engine left `Executing` becomes `Unknown` on start; a merge op in `Unknown`
//! re-runs the same idempotent sequence, so a restart can neither merge twice
//! nor lose the push.

use std::path::Path;
use std::process::Command;

use gear_app::config::ProjectConfig;
use gear_herdr::{HerdrAct, Outcome};
use gear_model::{
    ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, OpState, TaskState, Ts,
};
use gear_store::{NewMessage, NewOp, OpRow, Store};

use crate::EngineCfg;

/// A merge that went through: the task and the branches it closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub task: String,
    pub branch: String,
}

/// Queue and execute the merges of `project`: the engine's integration step.
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

    // 2. execute the ready ones (Intended, and Unknown from a restart).
    let ready = ready_merges(store, project)?;
    if ready.is_empty() {
        return Ok(merged);
    }
    let worktree = match integration_worktree(herdr, &config, project) {
        Ok(worktree) => worktree,
        Err(reason) => {
            // No worktree, no merge: the ops stay where they are, the captain
            // hears about it.
            tell_captain(store, project, &format!("merge waits: {reason}"), now)?;
            return Ok(merged);
        }
    };
    for op in ready {
        // The task may have been cancelled after the merge was queued: then
        // nothing is merged and the op is void (T-12 adversarial case).
        let task = op.task.clone().unwrap_or_default();
        if let Some(row) = store.read(|tx| tx.get_task(project, &task))? {
            if !matches!(row.state, TaskState::Verifying | TaskState::Done) {
                store.write(|tx| neutrino_void(tx, project, &op.id, now))?;
                continue;
            }
        } else {
            store.write(|tx| neutrino_void(tx, project, &op.id, now))?;
            continue;
        }
        // The op must still describe the current attempt and candidate: a
        // newer attempt, or a re-hand-in, makes it stale (P1 of the T-12
        // review round 2).
        if let Err(reason) = still_current(store, project, &op)? {
            store.write(|tx| neutrino_void(tx, project, &op.id, now))?;
            let _ = reason;
            continue;
        }
        let outcome = merge_one(&worktree, &config, &op);
        match outcome {
            Ok(lines) => {
                store.write(|tx| {
                    let state = tx.get_op(project, &op.id)?;
                    let from = state.map(|op| op.state).unwrap_or(OpState::Intended);
                    if from == OpState::Intended {
                        tx.set_op_state(project, &op.id, OpState::Executing, None, now)?;
                        tx.set_op_state(project, &op.id, OpState::Submitted, None, now)?;
                    }
                    tx.set_op_state(project, &op.id, OpState::Confirmed, Some(&lines), now)?;
                    Ok(())
                })?;
                merged.push(Merged {
                    task: op.task.clone().unwrap_or_default(),
                    branch: op.payload["branch"].as_str().unwrap_or("").to_string(),
                });
            }
            Err(reason) if reason.starts_with(RACE) => {
                // Lost a push race: try again next tick, a few times.
                let used = store.write(|tx| tx.bump_op_check_budget(project, &op.id, now))?;
                if used < RACE_RETRIES {
                    continue;
                }
                fail_merge(store, project, &op, &reason[RACE.len()..], now)?;
            }
            Err(reason) => {
                fail_merge(store, project, &op, &reason, now)?;
            }
        }
    }
    let _ = cfg;
    Ok(merged)
}

/// A push rejected because the integration branch moved meanwhile.
const RACE: &str = "push race: ";
/// How many ticks a merge that lost a push race is retried before it fails.
const RACE_RETRIES: i64 = 5;

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
    tell_captain(store, project, &format!("merge of {task} failed: {reason}\nnothing was pushed"), now)
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
    if from == OpState::Intended {
        tx.set_op_state(project, id, OpState::Void, Some("the task may not be merged"), now)?;
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

/// The engine's integration worktree: `home/merge`, created once from the
/// integration branch and reopened after that (17 §4: worktrees are Herdr's).
fn integration_worktree(
    herdr: &mut dyn HerdrAct,
    config: &ProjectConfig,
    project: &str,
) -> Result<gear_herdr::Worktree, String> {
    let label = format!("{project}-merge");
    match herdr.worktree_open(&config.project.repo, "home/merge", &label) {
        Outcome::Ok(worktree) => Ok(worktree),
        Outcome::Refused { code, .. } if code == "worktree_not_found" => {
            match herdr.worktree_create(
                &config.project.repo,
                "home/merge",
                &config.conventions.integration,
                &label,
            ) {
                Outcome::Ok(worktree) => Ok(worktree),
                other => Err(step_failure("worktree_create", &other)),
            }
        }
        other => Err(step_failure("worktree_open", &other)),
    }
}

/// Merge one op's task branches and run the gate. `Ok(lines)` = merged and
/// pushed; `Err(reason)` = nothing was pushed and the worktree is clean again.
fn merge_one(
    worktree: &gear_herdr::Worktree,
    config: &ProjectConfig,
    op: &OpRow,
) -> Result<String, String> {
    let dir = Path::new(&worktree.path);
    let integration = config.conventions.integration.clone();
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
    git(dir, &["fetch", "--prune", "origin"])?;
    git(dir, &["reset", "--hard", &format!("origin/{integration}")])?;

    // P0: the approved candidate, never whatever the branch points at now.
    let candidate = op.payload["candidate"].as_str().unwrap_or("").to_string();
    if candidate.is_empty() {
        return Err(format!("{task}: the merge op names no candidate"));
    }
    // The branch may have moved past the approved commit: then this needs a
    // new verdict, not a merge.
    match git(dir, &["rev-parse", "--verify", &format!("origin/{branch}")]) {
        Ok(head) => {
            let head = git(dir, &["rev-parse", &head.trim().to_string()])
                .unwrap_or_else(|_| head.trim().to_string());
            let candidate_sha = git(dir, &["rev-parse", &candidate])
                .unwrap_or_else(|_| candidate.clone());
            if head.trim() != candidate_sha.trim() {
                return Err(format!(
                    "{task}: origin/{branch} is at {} but the approved candidate is {}\n\
                     the branch moved after the review: a new verdict is needed",
                    head.trim(),
                    candidate_sha.trim()
                ));
            }
        }
        Err(_) => {
            // A missing branch is not proof of a merge: only an integration
            // branch that already contains the candidate is.
            if git(dir, &["merge-base", "--is-ancestor", &candidate, &format!("origin/{integration}")])
                .is_ok()
            {
                return Ok(format!("{task} was already merged: {candidate} is on origin/{integration}"));
            }
            return Err(format!(
                "{task}: origin/{branch} is gone and {candidate} is not on origin/{integration}"
            ));
        }
    }

    // 2. the two merges, in order (the approved candidate, then the review
    //    branch); the candidate is pinned by sha, so a branch that moves later
    //    cannot slip an unreviewed commit in.
    //    The review branch is *required*: a merge that publishes the task
    //    without its review is a partial merge (F6 of the T-12 review). The
    //    integration branch is not touched: the merge fails before it, so
    //    dev stays where it was.
    let review_ref = format!("origin/{review}");
    if let Err(reason) = git(dir, &["rev-parse", "--verify", &review_ref]) {
        return Err(format!(
            "{task}: the review branch {review_ref} is missing on origin\n{reason}\n\
             a merge publishes the approved candidate and its review branch together"
        ));
    }
    for pinned in [candidate.clone(), review_ref] {
        if let Err(reason) = git(
            dir,
            &[
                "merge",
                "--no-ff",
                &pinned,
                "-m",
                &format!("merge({task}): {pinned}"),
            ],
        ) {
            // Abort and leave the worktree exactly at the integration branch.
            let _ = git(dir, &["merge", "--abort"]);
            let _ = git(dir, &["reset", "--hard", &format!("origin/{integration}")]);
            return Err(format!("conflict merging {pinned}: {reason}"));
        }
    }

    // 3. conflict markers that a merge left behind without reporting a
    //    conflict (a file committed with markers in it).
    if git(dir, &["grep", "-n", "-e", "^<<<<<<<", "-e", "^>>>>>>>"]).is_ok() {
        let _ = git(dir, &["reset", "--hard", &format!("origin/{integration}")]);
        return Err("conflict markers in the merged tree".to_string());
    }

    // 4. the gate, before anything is pushed.
    let gate = config.conventions.gate.clone();
    let output = gate_command(&gate, dir)
        .output()
        .map_err(|error| format!("gate: {error}"))?;
    if !output.status.success() {
        // Both streams: cargo reports test failures on stdout, a shell's own
        // errors on stderr.
        let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
        let tail = tail(&combined, 12);
        let _ = git(dir, &["reset", "--hard", &format!("origin/{integration}")]);
        return Err(format!("gate failed ({gate}):\n{tail}"));
    }

    // 5. push, then close the branches on origin.
    // A push the remote rejects because the integration branch moved (the
    // captain or another merge pushed meanwhile) is a race, not a failure: the
    // next tick starts again from the new tip.
    if let Err(reason) = git(dir, &["push", "origin", &format!("HEAD:{integration}")]) {
        if reason.contains("non-fast-forward") || reason.contains("fetch first") {
            return Err(format!("{RACE}{reason}"));
        }
        return Err(reason);
    }
    let _ = Command::new("git")
        .args(["push", "origin", "--delete", &branch, &review])
        .current_dir(dir)
        .output();

    let head = git(dir, &["log", "-1", "--oneline"]).unwrap_or_default();
    Ok(format!("merged {task} into {integration}: {head}"))
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
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
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

/// Why a Herdr step did not succeed.
fn step_failure<T>(step: &str, outcome: &Outcome<T>) -> String {
    match outcome {
        Outcome::Refused { code, message } => format!("{step}: refused ({code}): {message}"),
        Outcome::Uncertain { reason } => format!("{step}: uncertain: {reason}"),
        Outcome::Ok(_) => format!("{step}: succeeded"),
    }
}

/// One open item in the captain's inbox.
fn tell_captain(store: &mut Store, project: &str, body: &str, now: Ts) -> GearResult<()> {
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
            task: None,
            attempt: None,
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
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
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

    /// A project config with the given gate.
    fn config(gate: &str) -> ProjectConfig {
        gear_app::config::parse(&config_text(gate)).unwrap()
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
                    updated: 0,
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

    #[test]
    fn a_clean_merge_is_pushed_and_the_branches_are_gone() {
        let scratch = Scratch::new("clean");
        let worktree = scratch.merge_worktree();

        let line = merge_one(&worktree, &config("true"), &op(&scratch.head_of("heat/t5"))).unwrap();

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
    /// is a race the next tick retries, not a failed merge.
    #[test]
    fn dev_moving_during_the_gate_is_a_race() {
        let scratch = Scratch::new("race");
        let worktree = scratch.merge_worktree();
        let racer = format!(
            "cd {} && git fetch -q origin && git switch -q dev && git reset -q --hard origin/dev && echo x >> other.txt && git add other.txt && git -c user.email=a@b -c user.name=a commit -qm racer && git push -q origin dev",
            scratch.work.display()
        );

        let error = merge_one(&worktree, &config(&racer), &op(&scratch.head_of("heat/t5"))).unwrap_err();

        assert!(error.starts_with(RACE), "{error}");
        assert!(!scratch.dev_has_task_commit(), "nothing of T-5 was pushed");
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

        let error = merge_one(&worktree, &config("true"), &op(&scratch.head_of("heat/t5"))).unwrap_err();

        assert!(error.contains("conflict merging"), "{error}");
        assert_eq!(scratch.dev_log(), before, "dev is untouched");
        let branches = scratch.origin_branches();
        assert!(branches.contains("heat/t5"), "{branches}");
    }

    #[test]
    fn a_failing_gate_leaves_dev_untouched_and_fails() {
        let scratch = Scratch::new("gate");
        let worktree = scratch.merge_worktree();
        let before = scratch.dev_log();

        let error = merge_one(&worktree, &config("echo the gate said no >&2; exit 1"), &op(&scratch.head_of("heat/t5"))).unwrap_err();

        assert!(error.contains("gate failed"), "{error}");
        assert!(error.contains("the gate said no"), "{error}");
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
    fn a_branch_that_moved_past_the_candidate_is_refused() {
        // P0 of the review round 2: the approved candidate is what merges. A
        // commit pushed to the task branch after the verdict is not merged,
        // dev stays as it was and the branches stay.
        let scratch = Scratch::new("moved");
        let worktree = scratch.merge_worktree();
        let candidate = scratch.head_of("heat/t5");
        // An unreviewed commit lands on the task branch.
        sh(
            &scratch.work,
            "git switch -q heat/t5 && echo sneaky > sneaky.txt && git add -A \
             && git -c user.email=a@b -c user.name=a commit -qm sneaky && git push -q origin heat/t5",
        );
        let before = scratch.dev_log();

        let error = merge_one(&worktree, &config("true"), &op(&candidate)).unwrap_err();

        assert!(error.contains("moved after the review"), "{error}");
        assert_eq!(scratch.dev_log(), before, "dev is untouched");
        assert!(!scratch.dev_has_task_commit(), "the unreviewed commit is not on dev");
        let branches = scratch.origin_branches();
        assert!(branches.contains("heat/t5"), "{branches}");
        assert!(branches.contains("review/t5"), "{branches}");
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

        let line = merge_one(&worktree, &config("true"), &settled).unwrap();

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

        let error =
            merge_one(&worktree, &config("true"), &op(&scratch.head_of("heat/t5"))).unwrap_err();

        assert!(error.contains("review branch"), "{error}");
        assert!(error.contains("origin/review/t5"), "{error}");
        assert!(error.contains("rev-parse"), "the git diagnostic: {error}");
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
        let merged = merge_passed(&mut store, &mut herdr, &cfg, "p", 20).unwrap();

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
        let inbox = store.read(|tx| tx.actionable_for("p", "p:captain", 21)).unwrap();
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
        let config = config("true");

        let candidate = scratch.head_of("heat/t5");
        merge_one(&worktree, &config, &op(&candidate)).unwrap();
        let first = scratch.dev_log();
        // The second run: the branches are gone, and the candidate is on dev.
        merge_one(&worktree, &config, &op(&candidate)).unwrap();
        let second = scratch.dev_log();

        assert_eq!(first, second, "no second merge, no lost push");
        assert_eq!(
            second.lines().filter(|line| line.starts_with("merge(T-5)")).count(),
            2,
            "exactly the two merge commits"
        );
    }
}
