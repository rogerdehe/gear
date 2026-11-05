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

use gear_app::config::ProjectConfig;
use gear_model::{AttemptState, ErrorCode, GearError, GearResult, SeatRole, TaskState, Ts};
use gear_store::{AttemptRow, SeatRow, Store, TaskRow};

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

/// What the step needs to hand one integration task in.
struct Job {
    row: TaskRow,
    branch: String,
    seat: SeatRow,
    attempts: Vec<AttemptRow>,
}

/// Hand one integration task in; `None` when it is not ready to be (a
/// dependency is not merged, its lane has no seat, or the branch has not moved
/// since a failed review).
fn hand_in(
    store: &mut Store,
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
    if let Err(error) = review_git(cfg.review_git.as_ref(), &repo.path, &["fetch", &remote]) {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!("fetch {remote} in {}: {error}", repo.path),
        ));
    }
    let candidate = repo_tips(tips, cfg, &repo.path, &remote)
        .get(&job.branch)
        .map(|(sha, _)| sha.clone());
    let Some(candidate) = candidate else {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{remote}/{} cannot be read in {}, so the feature task has no candidate",
                job.branch, repo.path
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
    let n = job
        .attempts
        .iter()
        .map(|attempt| attempt.n)
        .max()
        .unwrap_or(0)
        + 1;
    let report = format!("the feature branch {} at {candidate}", job.branch);
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn step(store: &mut Store, cfg: &EngineCfg) -> FeatureReport {
        let config = gear_app::config::latest(store, "p").unwrap();
        let mut tips = RepoTips::default();
        start_features(store, cfg, &mut tips, &config, "p", NOW + 10).unwrap()
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
        let report = step(&mut store, &cfg);
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
        let report = step(&mut store, &cfg);
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
        let report = step(&mut store, &cfg);
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
        let report = step(&mut store, &cfg);
        assert!(report.handed_in.is_empty(), "{:?}", report.handed_in);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let attempts = store.read(|tx| tx.attempts_of_task("p", "T-9")).unwrap();
        assert_eq!(attempts.len(), 2, "no attempt is opened beside the lane's");
        assert_eq!(attempts[1].state, AttemptState::Delivered);
    }
}
