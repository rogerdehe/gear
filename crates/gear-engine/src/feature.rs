//! Feature integration tasks hand themselves in (T-581): a task whose branch is
//! the feature's own branch — its `task_branch_override`, set only on the
//! integration task a feature's plan creates — is never worked by its lane.
//! When it is Ready and every dependency is merged, this step hands it in the
//! way a lane hands a result in (`tasks::start_attempt`,
//! `tasks::on_delivery_confirmed`, `tasks::submit_result`): attempt `Submitted`
//! at the tip of the feature branch, read after a fetch, so the normal review
//! follows and the feature reaches the integration branch through one review
//! and one full gate.
//!
//! A failed review puts the task back on Ready; this step does not hand the same
//! branch tip in again — the captain re-plans the member work, the feature
//! branch moves with the members' merges, and the next tip is handed in.
//!
//! T-597: before the hand-in the feature branch catches up with the integration
//! branch — the branch is merged into it (`refresh(<feature>): <integration>`, in
//! the merge worktree, as a merge does) and pushed, so the review and the full
//! gate judge what will land. A conflict pushes nothing, hands nothing in, and
//! tells the captain once; the step tries again only when a branch moves.

use gear_app::config::{ProjectConfig, Repo};
use gear_herdr::HerdrAct;
use gear_model::{
    AttemptState, ErrorCode, GearError, GearResult, MsgKind, MsgState, SeatRole, TaskState, Ts,
};
use gear_store::{AttemptRow, NewMessage, SeatRow, Store, TaskRow};

use crate::merge;
use crate::review::{repo_tips, review_git, RepoTips};
use crate::EngineCfg;

/// What the feature step did in one tick: `project:task/attempt` of every
/// integration task handed in, and the ones it could not hand in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeatureReport {
    /// `project:task/attempt` of the feature tasks handed in for review this
    /// tick.
    pub handed_in: Vec<String>,
    /// `project:task: CODE: detail` of the feature tasks that could not be
    /// handed in this tick.
    pub failed: Vec<String>,
}

/// The feature branch a task is merged from (T-577: `task_branch_override`),
/// keyed as the plan's apply writes it. An empty value is none. T-602: the
/// store's accessor is the one place that key is read.
pub(crate) fn branch_override(
    tx: &gear_store::Tx<'_>,
    project: &str,
    task: &str,
) -> GearResult<Option<String>> {
    tx.task_branch_override(project, task)
}

/// Hand in every feature integration task of `project` whose dependencies are
/// merged (T-581). Such a task is never assigned to its lane, so this is the
/// only way its attempt exists.
pub fn start_features(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    tips: &mut RepoTips,
    config: &ProjectConfig,
    project: &str,
    now: Ts,
) -> GearResult<FeatureReport> {
    let mut report = FeatureReport::default();
    let tasks: Vec<String> = store.read(|tx| {
        let mut ids = Vec::new();
        for task in tx.tasks_of_project(project)? {
            if task.state != TaskState::Ready {
                continue;
            }
            if branch_override(tx, project, &task.id)?.is_none() {
                continue;
            }
            ids.push(task.id);
        }
        Ok(ids)
    })?;
    for task in tasks {
        match hand_in(store, herdr, cfg, tips, config, project, &task, now) {
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

/// Whether the next step of an integration task that already has an attempt
/// is its lane's rework (T-592): the last attempt is closed (a failed review,
/// or an accepted one whose full gate failed and was reopened), every member
/// is merged into the feature branch, and none merged after that attempt. A
/// member merged since is how the captain chose to fix it (Lawrence M2: T-103
/// carries T-97's findings), so the engine hands the branch in instead.
/// Dispatch and the hand-in read the same answer, so the lane and the engine
/// never both act (Lawrence M2: the reviewer's own commit moved the branch and
/// the engine handed T-97 in again while its rework waited for web).
pub(crate) fn rework_is_the_lanes(
    tx: &gear_store::Tx<'_>,
    project: &str,
    row: &TaskRow,
) -> GearResult<bool> {
    let attempts = tx.attempts_of_task(project, &row.id)?;
    let Some(last) = attempts.iter().max_by_key(|attempt| attempt.n) else {
        return Ok(false);
    };
    if !matches!(last.state, AttemptState::Rejected | AttemptState::Accepted) {
        return Ok(false);
    }
    for dep in &row.deps {
        match gear_app::tasks::merged_at(tx, project, dep)? {
            None => return Ok(false),
            Some(at) if at > last.updated => return Ok(false),
            Some(_) => {}
        }
    }
    Ok(true)
}

/// What the step needs to hand one integration task in.
struct Job {
    row: TaskRow,
    branch: String,
    seat: SeatRow,
    attempts: Vec<AttemptRow>,
}

/// Hand one integration task in; `None` when it is not ready to be (a
/// dependency is not merged, its lane has no seat, the branch has not moved
/// since a failed review, or its refresh conflicts and no branch has moved).
#[allow(clippy::too_many_arguments)]
fn hand_in(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    tips: &mut RepoTips,
    config: &ProjectConfig,
    project: &str,
    task: &str,
    now: Ts,
) -> GearResult<Option<i64>> {
    let job = store.read(|tx| {
        let row = tx.get_task(project, task)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("no task {task} in {project}"))
        })?;
        let Some(branch) = branch_override(tx, project, task)? else {
            return Ok(None);
        };
        // The same merged rule as a dependent task (T-236): a member's work is
        // available once it is merged into the feature branch.
        for dep in &row.deps {
            if gear_app::tasks::merged_at(tx, project, dep)?.is_none() {
                return Ok(None);
            }
        }
        let lane = row.lane.clone().unwrap_or_default();
        // The writer is the integration task's lane seat, as an audit's is
        // (T-505): the attempt names the seat the review reads.
        let Some(seat) = tx
            .seats_of_project(project)?
            .into_iter()
            .find(|seat| seat.role == SeatRole::Lane && seat.name == lane)
        else {
            return Ok(None);
        };
        let attempts = tx.attempts_of_task(project, task)?;
        Ok(Some(Job {
            row,
            branch,
            seat,
            attempts,
        }))
    })?;
    let Some(job) = job else {
        return Ok(None);
    };
    // The candidate is the feature branch's tip, fetched now: the review reads
    // the branch as it stands.
    let repo = config.repo_of_task(&job.row);
    let remote = config.conventions.remote.clone();
    // The fetch names every branch of the remote, as a merge's fetch does: the
    // integration branch's tip and the feature branch's are both judged here.
    let fetch = [
        "fetch",
        "--prune",
        &remote,
        &format!("+refs/heads/*:refs/remotes/{remote}/*"),
    ];
    if let Err(error) = review_git(cfg.review_git.as_ref(), &repo.path, &fetch) {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!("fetch {remote} in {}: {error}", repo.path),
        ));
    }
    let (candidate, integration_tip) = {
        let tips = repo_tips(tips, cfg, &repo.path, &remote);
        let candidate = tips.get(&job.branch).map(|(sha, _)| sha.clone());
        let integration = tips.get(&repo.integration).map(|(sha, _)| sha.clone());
        (candidate, integration)
    };
    let Some(candidate) = candidate else {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{remote}/{} cannot be read in {}, so the feature task has no candidate",
                job.branch, repo.path
            ),
        ));
    };
    let Some(integration_tip) = integration_tip else {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{remote}/{} cannot be read in {}, so the feature branch cannot be refreshed",
                repo.integration, repo.path
            ),
        ));
    };
    // T-581: a branch that did not move since a failed review is not handed in
    // again — the captain re-plans the member work, and the task waits.
    if job.attempts.iter().any(|attempt| {
        attempt.state == AttemptState::Rejected
            && attempt.candidate.as_deref() == Some(candidate.as_str())
    }) {
        return Ok(None);
    }
    // T-592: a rework of the integration task is the owner lane's, delivered by
    // dispatch in this same tick — the lane fixes the feature branch and hands
    // its tip in. An attempt still in flight holds the hand-in, so the engine
    // never opens a second attempt beside the lane's.
    if job
        .attempts
        .iter()
        .any(|attempt| attempt.state.occupies_seat())
    {
        return Ok(None);
    }
    // A rejected attempt with its rework still to do is the lane's, whatever
    // moved the branch since (Lawrence M2: the reviewer's own commit moved
    // lawrence/feature/f-2, and the engine handed T-97 in again while web was
    // busy, so web never got the rework and the reviewer re-read unchanged
    // code).
    if store.read(|tx| rework_is_the_lanes(tx, project, &job.row))? {
        return Ok(None);
    }
    // T-597: the feature branch catches up with the integration branch before
    // the review and the full gate: they judge what will land.
    let feature = feature_of_carrier(store, project, task)?.unwrap_or_else(|| job.branch.clone());
    let refresh = refresh(
        store,
        herdr,
        config,
        project,
        &RefreshJob {
            repo: &repo,
            task,
            branch: &job.branch,
            feature: &feature,
            feature_tip: &candidate,
            integration_tip: &integration_tip,
            remote: &remote,
        },
        now,
    )?;
    let (candidate, report) = match refresh {
        Refresh::NotNeeded => {
            let report = format!("the feature branch {} at {candidate}", job.branch);
            (candidate, report)
        }
        Refresh::Refreshed { tip, subject } => {
            let report = format!(
                "the feature branch {} at {tip}, refreshed with the integration branch ({subject})",
                job.branch
            );
            (tip, report)
        }
        // The refresh conflicts (or the merge worktree is busy): nothing is
        // pushed and nothing is handed in; a later tick tries again.
        Refresh::Wait => return Ok(None),
    };
    let n = job
        .attempts
        .iter()
        .map(|attempt| attempt.n)
        .max()
        .unwrap_or(0)
        + 1;
    store.write(|tx| {
        // The same store calls a lane's `gear send --kind result` makes: the
        // attempt is delivered, confirmed and handed in in one transaction, so
        // the review sees exactly the record a lane's hand-in leaves.
        gear_app::tasks::start_attempt(tx, project, task, &job.seat.id, job.seat.generation, now)?;
        gear_app::tasks::on_delivery_confirmed(tx, project, task, n, now)?;
        let attempt = tx.get_attempt(project, task, n)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("no attempt {n} of {task}"))
        })?;
        gear_app::tasks::submit_result(
            tx,
            project,
            task,
            n,
            attempt.adopted_version,
            &candidate,
            &report,
            now,
        )
    })?;
    Ok(Some(n))
}

/// What the refresh of a feature branch before its hand-in came to (T-597).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refresh {
    /// The feature branch already holds the integration branch: it is handed
    /// in as it is.
    NotNeeded,
    /// The integration branch was merged into the feature branch and the branch
    /// pushed: its new tip, and the refresh commit's subject.
    Refreshed { tip: String, subject: String },
    /// Nothing was merged or pushed this tick: the merge conflicts (the captain
    /// has been told, and the step waits for a branch to move), the merge
    /// worktree is busy, or the same conflict was already found.
    Wait,
}

/// What a refresh needs: the repo, the task that waits for it, the feature
/// branch to feed and the integration branch it takes.
struct RefreshJob<'a> {
    repo: &'a Repo,
    task: &'a str,
    branch: &'a str,
    feature: &'a str,
    /// The feature branch's tip as the fetch read it.
    feature_tip: &'a str,
    /// The integration branch's tip as the fetch read it.
    integration_tip: &'a str,
    remote: &'a str,
}

/// The feature a carrier task integrates (T-597): the plan records the carrier
/// by feature (`feature_carrier:{project}:{feature}`), and the feature's id is
/// what a refresh's commit message and captain item name.
pub(crate) fn feature_of_carrier(
    store: &mut Store,
    project: &str,
    task: &str,
) -> GearResult<Option<String>> {
    store.read(|tx| {
        let prefix = format!("feature_carrier:{project}:");
        for key in tx.meta_keys_with_prefix(&prefix)? {
            if tx.get_meta(&key)?.as_deref() == Some(task) {
                return Ok(key.strip_prefix(&prefix).map(str::to_string));
            }
        }
        Ok(None)
    })
}

/// Where a refresh's conflict of one feature is kept (T-597): the two tips it
/// found, so the same conflict is not merged again until a branch moves.
fn conflict_key(project: &str, feature: &str) -> String {
    format!("feature_refresh_conflict:{project}:{feature}")
}

/// The two tips a conflict was found at, as the meta holds them.
fn tips(integration_tip: &str, feature_tip: &str) -> String {
    format!("{integration_tip} {feature_tip}")
}

/// Whether `a` is an ancestor of `b` in `dir` (`git merge-base --is-ancestor`).
fn is_ancestor(dir: &std::path::Path, a: &str, b: &str) -> bool {
    merge::git(dir, &["merge-base", "--is-ancestor", a, b]).is_ok()
}

/// Merge the integration branch into the feature branch when the feature branch
/// does not hold it yet, and push the feature branch (T-597). The merge runs in
/// the merge worktree, under the merge queue's lock, as a merge's own work does.
///
/// A conflict aborts the merge, pushes nothing, records the two tips it found
/// and opens one captain item; a later tick tries again only when the
/// integration branch or the feature branch has moved since.
#[allow(clippy::too_many_arguments)]
fn refresh(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    config: &ProjectConfig,
    project: &str,
    job: &RefreshJob<'_>,
    now: Ts,
) -> GearResult<Refresh> {
    let repo = job.repo;
    let dir = std::path::Path::new(&repo.path);
    let integration_ref = format!("{}/{}", job.remote, repo.integration);
    let feature_ref = format!("{}/{}", job.remote, job.branch);
    // The feature branch already holds the integration branch: nothing to
    // merge, and the hand-in is today's.
    if is_ancestor(dir, &integration_ref, &feature_ref) {
        return Ok(Refresh::NotNeeded);
    }
    // The same conflict was found at these two tips: the last attempt recorded
    // them and the captain was told. Nothing is merged again until a branch
    // moves.
    let found = tips(job.integration_tip, job.feature_tip);
    if store.read(|tx| tx.get_meta(&conflict_key(project, job.feature)))? == Some(found.clone()) {
        return Ok(Refresh::Wait);
    }
    let worktree = match merge::open_integration_worktree(store, herdr, repo, project, config, now)
    {
        Ok(worktree) => worktree,
        Err(reason) => {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "the merge worktree for the refresh of {} ({feature}): {reason}",
                    job.branch,
                    feature = job.feature
                ),
            ))
        }
    };
    let work = std::path::Path::new(&worktree.path);
    // The merge queue shares this worktree: its lock keeps the two apart, and a
    // worktree another merge holds is a later attempt, not a failure.
    let lock = match merge::merge_lock(work) {
        Ok(Some(lock)) => lock,
        Ok(None) => return Ok(Refresh::Wait),
        Err(reason) => {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!("the merge lock for the refresh of {}: {reason}", job.branch),
            ))
        }
    };
    let git = |args: &[&str]| merge::git_with_lock(work, args, Some(&lock));
    if let Err(reason) = git(&["reset", "--hard", &feature_ref]) {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!("reset the merge worktree to {feature_ref}: {reason}"),
        ));
    }
    let subject = format!("refresh({}): {}", job.feature, repo.integration);
    if let Err(reason) = git(&["merge", "--no-ff", "-m", &subject, &integration_ref]) {
        // The files that conflict, from the merge git just left in the tree.
        let files: Vec<String> = git(&["diff", "--name-only", "--diff-filter=U"])
            .map(|out| {
                out.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        // Nothing is pushed: the worktree goes back to the feature branch.
        let _ = git(&["merge", "--abort"]);
        let _ = git(&["reset", "--hard", &feature_ref]);
        store.write(|tx| tx.set_meta(&conflict_key(project, job.feature), &found))?;
        tell_conflict(store, project, job, &files, &reason, now)?;
        return Ok(Refresh::Wait);
    }
    let tip = match git(&["rev-parse", "HEAD"]) {
        Ok(out) => out.trim().to_string(),
        Err(reason) => {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!("read the refresh commit of {}: {reason}", job.branch),
            ))
        }
    };
    // The feature branch is pushed as the merge left it. A push the remote
    // rejects (a member merged meanwhile) pushes nothing: the next tick fetches
    // and merges again.
    if let Err(reason) = git(&[
        "push",
        job.remote,
        &format!("HEAD:refs/heads/{}", job.branch),
    ]) {
        let _ = git(&["reset", "--hard", &feature_ref]);
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!("push {} to {}: {reason}", job.branch, job.remote),
        ));
    }
    // The push moved the remote's branch: the tip this read is the one the
    // review and the merge that follows see.
    let _ = git(&[
        "update-ref",
        &format!("refs/remotes/{}/{}", job.remote, job.branch),
        &tip,
    ]);
    Ok(Refresh::Refreshed { tip, subject })
}

/// One captain item on thread `feature-refresh:<feature>` (T-597): the feature
/// branch cannot take the integration branch, so nothing was pushed and the
/// integration task was not handed in. An open item of the same feature is
/// updated, never duplicated.
fn tell_conflict(
    store: &mut Store,
    project: &str,
    job: &RefreshJob<'_>,
    files: &[String],
    reason: &str,
    now: Ts,
) -> GearResult<()> {
    let thread = format!("feature-refresh:{}", job.feature);
    let conflicts = if files.is_empty() {
        format!("it does not merge cleanly ({reason})")
    } else {
        format!("merging it conflicts in {}", files.join(", "))
    };
    let body = format!(
        "The feature branch {} cannot take the integration branch {} in: {conflicts}. \
         Nothing was pushed and the integration task {} was not handed in. Settle a fail of {} \
         with a rework; its owner lane resolves the conflict on the feature branch {}. \
         This step tries again when {} or {} moves (now {} {}, {} {}).",
        job.branch,
        job.repo.integration,
        job.task,
        job.task,
        job.branch,
        job.repo.integration,
        job.branch,
        job.repo.integration,
        job.integration_tip,
        job.branch,
        job.feature_tip
    );
    store.write(|tx| {
        let Some(captain) = tx.captain_of(project)? else {
            return Ok(());
        };
        let open = tx
            .thread_messages(project, &thread)?
            .into_iter()
            .find(|message| matches!(message.state, MsgState::Open | MsgState::Waiting));
        match open {
            Some(item) => {
                if item.body != body {
                    tx.update_message_body(project, &item.id, &body)?;
                }
                Ok(())
            }
            None => {
                tx.insert_message(&NewMessage {
                    meta: None,
                    project: project.to_string(),
                    thread,
                    task: Some(job.task.to_string()),
                    attempt: None,
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: captain.id,
                    kind: MsgKind::System,
                    urgent: true,
                    body,
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: now,
                })?;
                Ok(())
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_model::{SeatRole, TaskState};
    use gear_store::{testing, AttemptRow, AttemptStep, TaskRow};
    use std::path::Path;

    const NOW: i64 = 1_800_000_000;

    fn sh(dir: &Path, command: &str) -> String {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{command}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo whose `origin` holds `dev` and the feature branch
    /// `p/feature/f-1`, and the checkout the project reads (T-581).
    fn fixture() -> (tempfile::TempDir, tempfile::TempDir) {
        let bare = tempfile::tempdir().unwrap();
        sh(bare.path(), "git init -q --bare -b dev .");
        let work = tempfile::tempdir().unwrap();
        sh(
            work.path(),
            &format!(
                "git init -q -b dev && git remote add origin {} && \
                 echo base > README.md && git add -A && \
                 git -c user.email=a@b -c user.name=a commit -qm base && \
                 git checkout -q -b p/feature/f-1 && echo member > f.txt && git add -A && \
                 git -c user.email=a@b -c user.name=a commit -qm member && \
                 git push -q origin dev p/feature/f-1 && git checkout -q dev",
                bare.path().display()
            ),
        );
        (bare, work)
    }

    /// A commit on `dev` after the feature branch was cut (T-597): the feature
    /// branch is behind the integration branch.
    fn dev_moves_on(work: &Path) {
        sh(
            work,
            "echo later > later.txt && git add -A && \
             git -c user.email=a@b -c user.name=a commit -qm later && git push -q origin dev",
        );
    }

    /// A commit on `dev` that changes `f.txt` the other way (T-597): the
    /// feature branch cannot take the integration branch.
    fn dev_conflicts(work: &Path) {
        sh(
            work,
            "echo theirs > f.txt && git add -A && \
             git -c user.email=a@b -c user.name=a commit -qm theirs && git push -q origin dev",
        );
    }

    /// A herdr whose merge worktree is a real checkout, opened as merge.rs
    /// opens it (T-597): `<project>/merge/<voyage>` off `origin/dev`.
    fn herdr_with_merge_worktree(work: &Path, root: &Path) -> FakeHerdr {
        let path = root.join("merge");
        sh(
            work,
            &format!(
                "git worktree add -q -b p/merge/between-voyages {} origin/dev",
                path.display()
            ),
        );
        let mut herdr = FakeHerdr::new();
        herdr.add_worktree(
            "p/merge/between-voyages",
            &path.display().to_string(),
            "wt-merge:p1",
        );
        herdr
    }

    fn config_toml(repo: &Path) -> String {
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"{}\"\n\
             [quota]\nlanes = 1\ntemps = 1\n\
             [captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             [[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]\nkind = \"pi\"\nprofile = \"pi\"\n\
             [conventions]\nintegration = \"dev\"\nrelease = \"main\"\ngate = \"true\"\n",
            repo.display()
        )
    }

    /// A task of the fixture.
    fn task(id: &str, rank: i64, state: TaskState, deps: &[&str]) -> TaskRow {
        TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: id.into(),
            rank,
            state,
            priority: "must".into(),
            lane: Some("heat".into()),
            milestone: None,
            required: false,
            contract_version: 1,
            deps: deps.iter().map(|dep| (*dep).to_string()).collect(),
            followup_of: None,
            wait_on: None,
            wait_cond: None,
            rework_note: None,
            estimate_min: Some(10),
            issue: None,
            updated: NOW,
        }
    }

    fn attempt(id: &str, state: AttemptState, candidate: Option<&str>) -> AttemptRow {
        AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(),
            task: id.into(),
            n: 1,
            seat: "p:heat".into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state,
            candidate: candidate.map(str::to_string),
            report: None,
            safe_point: None,
            started: NOW,
            updated: NOW,
        }
    }

    /// The project with `heat`, the two members `T-1`/`T-2` (merged ones carry
    /// a merge time) and the integration task `T-9` on the feature's branch.
    fn world(repo: &Path, merged: &[&str], rejected: Option<&str>) -> (Store, EngineCfg) {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", NOW)?;
                let mut row = tx.get_project("p")?.unwrap();
                row.config_toml = config_toml(repo);
                tx.update_project(&row)?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    SeatRole::Captain,
                    "w1:p1",
                    "codex",
                    NOW,
                )?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "pi", NOW)?;
                tx.insert_task(&task("T-1", 1, TaskState::Done, &[]))?;
                tx.insert_task(&task("T-2", 2, TaskState::Done, &[]))?;
                tx.insert_task(&task("T-9", 3, TaskState::Ready, &["T-1", "T-2"]))?;
                tx.set_meta("task_branch_override:p:T-9", "p/feature/f-1")?;
                tx.set_feature_carrier("p", "F-1", "T-9")?;
                for id in ["T-1", "T-2"] {
                    tx.insert_attempt(&attempt(id, AttemptState::Accepted, None))?;
                    if merged.contains(&id) {
                        tx.set_attempt_time("p", id, 1, AttemptStep::Merged, NOW)?;
                    }
                }
                if let Some(candidate) = rejected {
                    tx.insert_attempt(&attempt("T-9", AttemptState::Rejected, Some(candidate)))?;
                    tx.set_attempt_time("p", "T-9", 1, AttemptStep::HandedIn, NOW)?;
                }
                Ok(())
            })
            .unwrap();
        (store, EngineCfg::default())
    }

    /// The engine's one step over the project, as the tick calls it.
    fn step(store: &mut Store, cfg: &EngineCfg, herdr: &mut FakeHerdr) -> FeatureReport {
        let config = gear_app::config::latest(store, "p").unwrap();
        let mut tips = RepoTips::default();
        start_features(store, herdr, cfg, &mut tips, &config, "p", NOW + 10).unwrap()
    }

    /// The feature branch's tip as the remote holds it.
    fn tip(work: &Path) -> String {
        sh(work, "git rev-parse p/feature/f-1")
    }

    /// T-581: the integration task is never the lane's work, and with every
    /// member merged it hands itself in at the feature branch's tip.
    #[test]
    fn a_feature_task_with_every_member_merged_hands_itself_in() {
        let (bare, work) = fixture();
        let (mut store, cfg) = world(work.path(), &["T-1", "T-2"], None);
        let candidate = tip(work.path());
        // The lane's ready work holds no integration task: no delivery op is
        // ever intended for it.
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let ready = store
            .read(|tx| crate::dispatch::ready_tasks_of(tx, &seat))
            .unwrap();
        assert!(
            ready.iter().all(|task| task.id != "T-9"),
            "the integration task is not delivered: {:?}",
            ready
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>()
        );
        let report = step(&mut store, &cfg, &mut FakeHerdr::new());
        assert_eq!(report.handed_in, vec!["p:T-9/1".to_string()]);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let (state, handed) = store
            .read(|tx| {
                let task = tx.get_task("p", "T-9")?.unwrap();
                let attempt = tx.get_attempt("p", "T-9", 1)?.unwrap();
                Ok((task.state, attempt))
            })
            .unwrap();
        assert_eq!(state, TaskState::Verifying, "the review follows at once");
        assert_eq!(handed.state, AttemptState::Submitted);
        assert_eq!(handed.candidate.as_deref(), Some(candidate.as_str()));
        let expected = format!("the feature branch p/feature/f-1 at {candidate}");
        assert_eq!(handed.report.as_deref(), Some(expected.as_str()));
        // Nothing was pushed anywhere: the hand-in reads the branch as it is.
        assert_eq!(
            sh(
                bare.path(),
                "git for-each-ref --format='%(refname:short)' refs/heads"
            ),
            "dev\np/feature/f-1"
        );
    }

    /// T-581: one member not yet merged and the integration task waits.
    #[test]
    fn a_member_that_is_not_merged_holds_the_hand_in() {
        let (_bare, work) = fixture();
        let (mut store, cfg) = world(work.path(), &["T-1"], None);
        let report = step(&mut store, &cfg, &mut FakeHerdr::new());
        assert!(report.handed_in.is_empty(), "{:?}", report.handed_in);
        let (state, attempts) = store
            .read(|tx| {
                let task = tx.get_task("p", "T-9")?.unwrap();
                Ok((task.state, tx.attempts_of_task("p", "T-9")?))
            })
            .unwrap();
        assert_eq!(state, TaskState::Ready, "the task waits as it was");
        assert!(attempts.is_empty(), "no attempt is opened yet");
    }

    /// T-581: a failed review of the same branch tip is not handed in again —
    /// the captain re-plans the members and the task waits.
    #[test]
    fn a_failed_review_of_the_same_tip_waits_for_the_captain() {
        let (_bare, work) = fixture();
        let candidate = tip(work.path());
        let (mut store, cfg) = world(work.path(), &["T-1", "T-2"], Some(&candidate));
        let report = step(&mut store, &cfg, &mut FakeHerdr::new());
        assert!(report.handed_in.is_empty(), "{:?}", report.handed_in);
        assert!(
            report.failed.is_empty(),
            "a wait is not a failure: {:?}",
            report.failed
        );
        let attempts = store.read(|tx| tx.attempts_of_task("p", "T-9")).unwrap();
        assert_eq!(attempts.len(), 1, "the failed attempt is not replaced");
        assert_eq!(attempts[0].state, AttemptState::Rejected);
    }

    /// The feature branch's tip as the remote holds it: the bare repo's own ref.
    fn remote_tip(bare: &Path) -> String {
        sh(bare, "git rev-parse p/feature/f-1")
    }

    /// The one-item-on-the-thread reading of a feature's refresh item.
    fn refresh_items(store: &mut Store) -> Vec<gear_store::MessageRow> {
        store
            .read(|tx| tx.thread_messages("p", "feature-refresh:F-1"))
            .unwrap()
    }

    /// T-597: a feature branch behind the integration branch is refreshed before
    /// the hand-in — the integration branch merged into it, the branch pushed,
    /// and the hand-in names the refresh commit.
    #[test]
    fn a_feature_branch_behind_the_integration_branch_is_refreshed() {
        let (bare, work) = fixture();
        dev_moves_on(work.path());
        let (mut store, cfg) = world(work.path(), &["T-1", "T-2"], None);
        let before = tip(work.path());
        let worktrees = tempfile::tempdir().unwrap();
        let mut herdr = herdr_with_merge_worktree(work.path(), worktrees.path());
        let report = step(&mut store, &cfg, &mut herdr);
        assert_eq!(report.handed_in, vec!["p:T-9/1".to_string()]);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        // The feature branch moved on the remote and holds the integration
        // branch now, under a refresh commit.
        let refreshed = remote_tip(bare.path());
        assert_ne!(refreshed, before, "the refresh commit is the new tip");
        sh(
            bare.path(),
            "git merge-base --is-ancestor dev p/feature/f-1",
        );
        assert_eq!(
            sh(bare.path(), "git log -1 --format=%s p/feature/f-1"),
            "refresh(F-1): dev"
        );
        // The hand-in is at the new tip and names the refresh commit.
        let handed = store
            .read(|tx| tx.get_attempt("p", "T-9", 1))
            .unwrap()
            .unwrap();
        assert_eq!(handed.state, AttemptState::Submitted);
        assert_eq!(handed.candidate.as_deref(), Some(refreshed.as_str()));
        let report = handed.report.unwrap_or_default();
        assert!(
            report.contains("refresh(F-1): dev"),
            "the hand-in names the refresh commit: {report}"
        );
        assert!(
            report.contains(&refreshed),
            "it names the new tip: {report}"
        );
        // The conflict item is not opened by a refresh that merged.
        assert!(refresh_items(&mut store).is_empty());
    }

    /// T-597: a feature branch that cannot take the integration branch opens
    /// one item, hands nothing in, and is not tried again until a branch moves;
    /// once the owner lane resolves it there, the hand-in follows.
    #[test]
    fn a_conflicting_refresh_opens_the_item_once_and_waits_for_a_branch_to_move() {
        let (bare, work) = fixture();
        dev_conflicts(work.path());
        let (mut store, cfg) = world(work.path(), &["T-1", "T-2"], None);
        let before = remote_tip(bare.path());
        let worktrees = tempfile::tempdir().unwrap();
        let mut herdr = herdr_with_merge_worktree(work.path(), worktrees.path());
        let report = step(&mut store, &cfg, &mut herdr);
        assert!(report.handed_in.is_empty(), "nothing is handed in");
        assert!(
            report.failed.is_empty(),
            "a conflict the captain is told of is not a failure: {:?}",
            report.failed
        );
        let items = refresh_items(&mut store);
        assert_eq!(items.len(), 1, "one item: {items:?}");
        let body = items[0].body.clone();
        assert!(
            body.contains("f.txt"),
            "it names the conflicting file: {body}"
        );
        assert!(
            body.contains("Settle a fail of T-9 with a rework"),
            "it says how it is settled: {body}"
        );
        assert!(body.contains("owner lane"), "{body}");
        assert_eq!(
            remote_tip(bare.path()),
            before,
            "nothing was pushed to the feature branch"
        );
        assert!(
            store
                .read(|tx| tx.attempts_of_task("p", "T-9"))
                .unwrap()
                .is_empty(),
            "nothing was handed in"
        );
        // The same tick again, with neither branch moved: no second item and no
        // hand-in — the step does not merge and conflict again.
        let report = step(&mut store, &cfg, &mut herdr);
        assert!(report.handed_in.is_empty(), "{:?}", report.handed_in);
        assert_eq!(refresh_items(&mut store).len(), 1, "still one item");
        // The owner lane resolves the conflict on the feature branch: the
        // branch moved, so the step goes on and hands the feature in.
        sh(
            work.path(),
            "git checkout -q p/feature/f-1 && \
             git merge -q --no-ff -m 'fix: take dev' origin/dev >/dev/null 2>&1; \
             echo ours > f.txt && git add -A && \
             git -c user.email=a@b -c user.name=a commit -qm 'resolve f.txt' && \
             git push -q origin p/feature/f-1 && git checkout -q dev",
        );
        let report = step(&mut store, &cfg, &mut herdr);
        assert_eq!(report.handed_in, vec!["p:T-9/1".to_string()]);
        let handed = store
            .read(|tx| tx.get_attempt("p", "T-9", 1))
            .unwrap()
            .unwrap();
        assert_eq!(
            handed.candidate.as_deref(),
            Some(remote_tip(bare.path()).as_str())
        );
        assert_eq!(refresh_items(&mut store).len(), 1, "no second item");
    }

    /// T-597: a feature branch that already holds the integration branch is
    /// handed in as before — nothing is merged and no merge worktree is opened.
    #[test]
    fn a_feature_branch_that_already_holds_the_integration_branch_is_handed_in_as_is() {
        let (bare, work) = fixture();
        let (mut store, cfg) = world(work.path(), &["T-1", "T-2"], None);
        let candidate = tip(work.path());
        // A herdr with no merge worktree: a refresh that tried to merge would
        // fail here, so this proves none ran.
        let report = step(&mut store, &cfg, &mut FakeHerdr::new());
        assert_eq!(report.handed_in, vec!["p:T-9/1".to_string()]);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let handed = store
            .read(|tx| tx.get_attempt("p", "T-9", 1))
            .unwrap()
            .unwrap();
        assert_eq!(handed.candidate.as_deref(), Some(candidate.as_str()));
        assert_eq!(
            handed.report.as_deref(),
            Some(format!("the feature branch p/feature/f-1 at {candidate}").as_str())
        );
        assert_eq!(remote_tip(bare.path()), candidate, "nothing was pushed");
        assert!(refresh_items(&mut store).is_empty());
    }

    /// A rejected integration task whose rework is still to do is the lane's,
    /// even when its branch moved since (Lawrence M2: the reviewer's own commit
    /// moved the feature branch and T-97 was handed in again unchanged).
    #[test]
    fn a_rework_still_to_do_is_never_handed_in_by_the_engine() {
        let (_bare, work) = fixture();
        let old = tip(work.path());
        let (mut store, cfg) = world(work.path(), &["T-1", "T-2"], Some(&old));
        store
            .write(|tx| {
                let mut row = tx.get_task("p", "T-9")?.unwrap();
                row.rework_note = Some("Fix in one round: F1".into());
                tx.update_task(&row)
            })
            .unwrap();
        sh(
            work.path(),
            "git checkout -q p/feature/f-1 && echo review > r.txt && git add -A && \
             git -c user.email=a@b -c user.name=a commit -qm review && \
             git push -q origin p/feature/f-1 && git checkout -q dev",
        );
        let report = step(&mut store, &cfg, &mut FakeHerdr::new());
        assert!(report.handed_in.is_empty(), "{:?}", report.handed_in);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let ready = store
            .read(|tx| crate::dispatch::ready_tasks_of(tx, &seat))
            .unwrap();
        assert!(
            ready.iter().any(|task| task.id == "T-9"),
            "the rework is the lane's to take"
        );
    }

    /// A member merged after the rejection carries the fix (Lawrence M2: T-103
    /// for T-97): the engine hands the moved branch in, and the lane is not
    /// given the rework too.
    #[test]
    fn a_member_merged_after_the_rejection_hands_the_branch_in() {
        let (_bare, work) = fixture();
        let old = tip(work.path());
        let (mut store, cfg) = world(work.path(), &["T-1"], Some(&old));
        store
            .write(|tx| tx.set_attempt_time("p", "T-2", 1, AttemptStep::Merged, NOW + 5))
            .unwrap();
        sh(
            work.path(),
            "git checkout -q p/feature/f-1 && echo fix > fix.txt && git add -A && \
             git -c user.email=a@b -c user.name=a commit -qm 'merge(T-2): fix' && \
             git push -q origin p/feature/f-1 && git checkout -q dev",
        );
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let ready = store
            .read(|tx| crate::dispatch::ready_tasks_of(tx, &seat))
            .unwrap();
        assert!(
            ready.iter().all(|task| task.id != "T-9"),
            "the lane is not given the rework"
        );
        let report = step(&mut store, &cfg, &mut FakeHerdr::new());
        assert_eq!(report.handed_in, vec!["p:T-9/2".to_string()]);
    }

    /// T-592: the rework of an integration task is the owner lane's — an attempt
    /// still in flight holds the engine's hand-in, so the two never open an
    /// attempt each.
    #[test]
    fn a_rework_in_the_lanes_hands_holds_the_hand_in() {
        let (_bare, work) = fixture();
        let candidate = tip(work.path());
        let (mut store, cfg) = world(work.path(), &["T-1", "T-2"], Some(&candidate));
        store
            .write(|tx| {
                let mut attempt = attempt("T-9", AttemptState::Delivered, None);
                attempt.n = 2;
                tx.insert_attempt(&attempt)
            })
            .unwrap();
        let report = step(&mut store, &cfg, &mut FakeHerdr::new());
        assert!(report.handed_in.is_empty(), "{:?}", report.handed_in);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let attempts = store.read(|tx| tx.attempts_of_task("p", "T-9")).unwrap();
        assert_eq!(attempts.len(), 2, "no attempt is opened beside the lane's");
        assert_eq!(attempts[1].state, AttemptState::Delivered);
    }
}
