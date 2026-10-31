//! T-253: the engine names branches from the project's templates, and a
//! worktree Herdr returns must belong to the repo it is used for.
//!
//! The templates are `[conventions].task_branch`, `review_branch` and
//! `seat_branch` (T-252). Two different repositories can share one folder name
//! — Herdr places worktrees by the folder's name and the branch — so the
//! engine checks a worktree's git common dir before it uses the path.

use std::{fs, path::Path, process::Command};

use gear_app::{admin, tasks, Env};
use gear_engine::{
    dispatch::{intend_dispatch, Action},
    merge::merge_passed,
    onboard::onboard_queued,
    review::start_reviews,
    EngineCfg,
};
use gear_herdr::{
    builtin,
    fake::{Call, FakeHerdr},
};
use gear_model::{OpKind, OpState, SeatState, TaskState};
use gear_store::{ContractRow, ReviewVerdictRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;

/// Run a shell command in `dir`; it must succeed.
fn sh(dir: &Path, command: &str) -> String {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{command} in {}: {}{}",
        dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Move `dir`'s HEAD onto `branch` in its origin, from the origin's own side
/// (T-107): no engine test runs `git push`.
fn publish(dir: &Path, branch: &str) {
    sh(
        dir,
        &format!(
            "origin=$(git remote get-url origin); \
             git -C \"$origin\" fetch -q \"$(pwd)\" +HEAD:refs/heads/{branch} && \
             git fetch -q origin"
        ),
    );
}

/// One real Git repo: a bare `origin` on `dev` and the checkout that works.
struct Repo {
    root: tempfile::TempDir,
    origin: std::path::PathBuf,
    work: std::path::PathBuf,
}

impl Repo {
    fn new() -> Repo {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let work = root.path().join("work");
        sh(
            root.path(),
            &format!("git init -q --bare -b dev {}", origin.display()),
        );
        sh(
            root.path(),
            &format!("git init -q -b dev {}", work.display()),
        );
        fs::write(work.join("README.md"), "base\n").unwrap();
        sh(
            &work,
            "git add -A && git -c user.email=a@b -c user.name=a commit -qm base",
        );
        sh(
            &work,
            &format!("git remote add origin {}", origin.display()),
        );
        publish(&work, "dev");
        Repo { root, origin, work }
    }

    /// The lane's work on its task branch: a commit, pushed. Its id is the
    /// candidate.
    fn lane_work(&self, branch: &str) -> String {
        sh(
            &self.work,
            &format!(
                "git switch -qc {branch} dev && echo work >> work.txt && git add -A \
                 && git -c user.email=a@b -c user.name=a commit -qm work"
            ),
        );
        publish(&self.work, branch);
        sh(&self.work, &format!("git rev-parse {branch}"))
    }

    /// The reviewer's work: its review branch, pushed.
    fn review_work(&self, branch: &str) {
        sh(
            &self.work,
            &format!(
                "git switch -qc {branch} dev && echo review > review.txt && git add -A \
                 && git -c user.email=a@b -c user.name=a commit -qm review"
            ),
        );
        publish(&self.work, branch);
    }

    /// A checkout of the merge branch, as Herdr would return it.
    fn merge_worktree(&self, branch: &str) -> std::path::PathBuf {
        let path = self.root.path().join("merge-worktree");
        sh(
            self.root.path(),
            &format!(
                "git -C {} worktree add -q -b {branch} {} origin/dev",
                self.work.display(),
                path.display()
            ),
        );
        path
    }

    /// The project config with the given `[conventions]` lines.
    fn config(&self, conventions: &str) -> String {
        format!(
            r#"
[project]
id = "p"
herdr_session = "sess"
repo = "{repo}"

[quota]
lanes = 1
temps = 1

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

[temps]
kind = "claude"
profile = "claude"

[conventions]
gate = "true"
{conventions}
"#,
            repo = self.work.display(),
        )
    }
}

/// The engine knows the profiles the project uses.
fn cfg() -> EngineCfg {
    let mut cfg = EngineCfg {
        db_path: crate::temp("gear-engine-tests")
            .join("gear.db")
            .to_string_lossy()
            .into_owned(),
        // T-107: the merge's remote runner moves refs from origin's side; no
        // engine test runs `git push`.
        remote: std::sync::Arc::new(gear_engine::merge::FetchRemote),
        // A merge that retries does not wait on the clock in a test (T-127).
        merge_retry_wait: gear_engine::merge::RetryWait::immediate(),
        ..Default::default()
    };
    cfg.review_git = std::sync::Arc::new(crate::FakeReviewGit);
    for kind in ["opencode", "claude"] {
        cfg.profiles.insert(kind.into(), builtin(kind).unwrap());
    }
    cfg
}

/// Onboarded lane, a Ready task and its contract: the state a dispatch starts
/// from.
fn world(config: &str) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    admin::init_with_home(
        &mut store,
        &Env {
            ctx: None,
            pane_id: None,
            admin: true,
        },
        config,
        Some(crate::skill_home()),
        false,
        T0,
    )
    .unwrap();
    store
        .write(|tx| {
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
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
                issue: None,
                updated: T0,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-1".into(),
                version: 1,
                goal: "the goal".into(),
                accept: "the accept criteria".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
            Ok(())
        })
        .unwrap();
    store
}

/// The text of the last Deliver op for `seat`.
fn delivered(store: &mut Store, seat: &str) -> String {
    store
        .read(|tx| {
            Ok(tx
                .ops_of_seat_kind(seat, OpKind::Deliver)?
                .into_iter()
                .last()
                .map(|op| op.payload["text"].as_str().unwrap_or_default().to_string())
                .unwrap_or_default())
        })
        .unwrap()
}

/// The worktree the engine created for `branch`: (repo, branch, base, label).
fn created(herdr: &FakeHerdr, branch: &str) -> (String, String, String, String) {
    herdr
        .calls
        .iter()
        .find_map(|call| match call {
            Call::WorktreeCreate {
                repo,
                branch: made,
                base,
                label,
            } if made == branch => Some((repo.clone(), made.clone(), base.clone(), label.clone())),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "the engine created the worktree {branch}: {:?}",
                herdr.calls
            )
        })
}

/// Wait for the merge to finish; it must be confirmed.
fn wait_merge(store: &mut Store, herdr: &mut FakeHerdr, cfg: &EngineCfg) {
    const WAIT: std::time::Duration = std::time::Duration::from_secs(60);
    loop {
        let seen = cfg.merges.progress();
        merge_passed(store, herdr, cfg, "p", T0 + 40).unwrap();
        let op = store
            .read(|tx| {
                Ok(tx
                    .ops_in_state("p", OpState::Confirmed)?
                    .into_iter()
                    .find(|op| op.kind == OpKind::Merge))
            })
            .unwrap();
        if op.is_some() {
            return;
        }
        let failed = store
            .read(|tx| {
                Ok(tx
                    .ops_in_state("p", OpState::Failed)?
                    .into_iter()
                    .find(|op| op.kind == OpKind::Merge)
                    .map(|op| op.result))
            })
            .unwrap();
        assert!(failed.is_none(), "the merge failed: {failed:?}");
        if cfg.merges.running("p") {
            assert!(
                cfg.merges.wait_progress(seen, WAIT),
                "the merge worker did not report within {WAIT:?}"
            );
        } else {
            assert!(
                cfg.merges.progress() != seen,
                "the merge step had nothing to run"
            );
        }
    }
}

/// T-253: the dispatch packet and the lane's worktree come from
/// `task_branch` and `seat_branch`; with `seat_branch = "w/{seat}"` the lane's
/// worktree step names `w/heat`, not the default `p/heat`.
#[test]
fn the_dispatch_packet_and_the_lane_worktree_come_from_the_projects_templates() {
    let repo = Repo::new();
    let config = repo.config(
        r#"task_branch = "b/{project}/{lane}/t{n}"
review_branch = "b/review/t{n}"
seat_branch = "w/{seat}""#,
    );
    let mut store = world(&config);
    let mut herdr = FakeHerdr::new();
    let cfg = cfg();

    let active = onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1).unwrap();
    assert_eq!(active, vec!["p:heat".to_string()]);
    let (_, branch, base, _) = created(&herdr, "w/heat");
    assert_eq!(
        branch, "w/heat",
        "the project's seat_branch names the worktree"
    );
    assert_eq!(base, "dev");

    // The dispatch packet names the project's task branch.
    let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    let actions = vec![(seat, Action::Assign { task: "T-1".into() })];
    store
        .write(|tx| intend_dispatch(tx, "p", &actions, T0 + 10).map(|_| ()))
        .unwrap();
    let text = delivered(&mut store, "p:heat");
    assert!(
        text.contains("b/p/heat/t1"),
        "the dispatch packet names the branch: {text}"
    );
}

/// T-253: the review packet names `review_branch`, the review seat's worktree
/// is `seat_branch`, and the merge runs on the task branch and the review
/// branch of the project's templates.
#[test]
fn the_review_and_the_merge_come_from_the_projects_templates() {
    let repo = Repo::new();
    let config = repo.config(
        r#"task_branch = "b/{project}/{lane}/t{n}"
review_branch = "b/review/t{n}"
seat_branch = "w/{seat}""#,
    );
    let mut store = world(&config);
    let mut herdr = FakeHerdr::new();
    let cfg = cfg();

    let active = onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1).unwrap();
    assert_eq!(active, vec!["p:heat".to_string()]);
    let (_, branch, _, _) = created(&herdr, "w/heat");
    assert_eq!(branch, "w/heat");

    // The lane works on the task branch its template names, and hands in.
    let candidate = repo.lane_work("b/p/heat/t1");
    store
        .write(|tx| {
            tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
            tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
            tasks::submit_result(tx, "p", "T-1", 1, 1, &candidate, "ops/reports/T-1.md", T0)?;
            Ok(())
        })
        .unwrap();

    // The review packet names the project's review branch, and the review
    // seat's own worktree is its seat_branch.
    let report = start_reviews(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T0 + 10).unwrap();
    assert_eq!(report.started, vec!["p:review-1"]);
    let (_, branch, _, _) = created(&herdr, "w/review-1");
    assert_eq!(branch, "w/review-1");
    let text = delivered(&mut store, "p:review-1");
    assert!(
        text.contains("b/review/t1"),
        "the review packet names the branch: {text}"
    );

    // The reviewer's work, the verdict, the merge.
    repo.review_work("b/review/t1");
    store
        .write(|tx| {
            tx.insert_review_verdict(&ReviewVerdictRow {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                passed: true,
                findings: "all good".into(),
                by_seat: "p:review-1".into(),
                created: T0 + 25,
            })?;
            tasks::settle_review(tx, "p", "T-1", 1, T0 + 30)
        })
        .unwrap();

    let merge_worktree = repo.merge_worktree("w/merge");
    herdr.add_worktree("w/merge", &merge_worktree.to_string_lossy(), "wt-merge:p1");
    wait_merge(&mut store, &mut herdr, &cfg);

    // The queued merge op named both the task branch and the review branch of
    // the project's templates.
    let op = store
        .read(|tx| {
            Ok(tx
                .ops_of_seat_kind("p:heat", OpKind::Merge)?
                .into_iter()
                .last())
        })
        .unwrap()
        .expect("the task has a merge op");
    assert_eq!(op.payload["branch"], "b/p/heat/t1");

    // The merge opened the worktree of the project's seat_branch.
    assert!(
        herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::WorktreeOpen { branch, .. } if branch == "w/merge")),
        "the merge opened w/merge: {:?}",
        herdr.calls
    );
    let log = sh(&repo.origin, "git log --format=%s dev");
    assert!(log.contains("work"), "the task branch landed: {log}");
    assert!(
        log.contains("review"),
        "the review branch from the template landed: {log}"
    );
    // The task branch the merge consumed is closed on origin, under the name
    // the project's template gave it.
    assert_eq!(
        sh(&repo.origin, "git branch --list b/p/heat/t1"),
        "",
        "the task branch is closed"
    );
}

/// T-253 (F1 of the review): the cleanup after a merge closes **only** the
/// task's own branches. With `task_branch = "tasks/{n}/work"` the number is not
/// the final segment, so a last-segment match called `tasks/2/work` a branch of
/// T-1 and deleted another task's work.
#[test]
fn a_merge_closes_only_the_tasks_own_branches() {
    let repo = Repo::new();
    let config = repo.config(
        r#"task_branch = "tasks/{n}/work"
review_branch = "b/review/t{n}"
seat_branch = "w/{seat}""#,
    );
    let mut store = world(&config);
    let mut herdr = FakeHerdr::new();
    let cfg = cfg();

    let active = onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1).unwrap();
    assert_eq!(active, vec!["p:heat".to_string()]);

    // The task's work, plus three branches that share the candidate's tip:
    // the task's own rework suffix, and another task's branch under the same
    // template (`tasks/2/work` is T-2's, not T-1's).
    let candidate = repo.lane_work("tasks/1/work");
    for branch in ["tasks/1/work-v2", "tasks/2/work"] {
        sh(&repo.origin, &format!("git branch {branch} {candidate}"));
    }

    store
        .write(|tx| {
            tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
            tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
            tasks::submit_result(tx, "p", "T-1", 1, 1, &candidate, "ops/reports/T-1.md", T0)?;
            Ok(())
        })
        .unwrap();

    let report = start_reviews(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T0 + 10).unwrap();
    assert_eq!(report.started, vec!["p:review-1"]);
    repo.review_work("b/review/t1");
    store
        .write(|tx| {
            tx.insert_review_verdict(&ReviewVerdictRow {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                passed: true,
                findings: "all good".into(),
                by_seat: "p:review-1".into(),
                created: T0 + 25,
            })?;
            tasks::settle_review(tx, "p", "T-1", 1, T0 + 30)
        })
        .unwrap();

    let merge_worktree = repo.merge_worktree("w/merge");
    herdr.add_worktree("w/merge", &merge_worktree.to_string_lossy(), "wt-merge:p1");
    wait_merge(&mut store, &mut herdr, &cfg);

    // The task's own branch and its rework suffix are closed…
    assert_eq!(
        sh(&repo.origin, "git branch --list 'tasks/1/work*'"),
        "",
        "the task's own branches are closed"
    );
    // …and the other task's branch survives, still holding the shared commit.
    let unrelated = sh(&repo.origin, "git rev-parse tasks/2/work");
    assert_eq!(unrelated, candidate, "another task's branch is untouched");
}

/// T-253: a worktree Herdr returns for a seat must belong to the seat's repo.
/// Two repos can share one folder name, so Herdr can hand the engine another
/// repository's checkout. It is not used: the captain gets one item on thread
/// `worktree:<seat>` and no agent starts in it.
#[test]
fn a_worktree_of_another_repository_is_not_used_and_opens_a_captain_item() {
    let repo = Repo::new();
    // A second repository, whose checkout Herdr returns for this seat.
    let other = Repo::new();
    let foreign = other.root.path().join("foreign-worktree");
    sh(
        other.root.path(),
        &format!(
            "git -C {} worktree add -q -b {branch} {path} dev",
            other.work.display(),
            branch = "p/heat",
            path = foreign.display()
        ),
    );

    let config = repo.config("");
    let mut store = world(&config);
    let mut herdr = FakeHerdr::new();
    // Herdr answers the seat's branch with the other repository's worktree.
    herdr.add_worktree("p/heat", &foreign.to_string_lossy(), "other:p1");
    let cfg = cfg();

    let active = onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1).unwrap();
    assert!(active.is_empty(), "the seat is not onboarded: {active:?}");
    let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::NeedsCheck);
    let reason = seat.state_reason.unwrap_or_default();
    assert!(reason.contains("belongs to another repository"), "{reason}");

    // The path is not used: no shell, no agent in it.
    assert!(
        !herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::AgentStart { .. } | Call::PaneRun { .. })),
        "nothing starts in another repository's checkout: {:?}",
        herdr.calls
    );

    // The captain hears about it once, on the seat's own thread.
    let items = store
        .read(|tx| tx.thread_messages("p", "worktree:heat"))
        .unwrap();
    let body = items
        .last()
        .map(|message| message.body.clone())
        .unwrap_or_default();
    assert!(body.contains("belongs to another repository"), "{body}");
    assert!(
        body.contains("conventions.seat_branch with {project}"),
        "{body}"
    );
    assert_eq!(items.len(), 1, "one item, not one per tick: {items:?}");
}
