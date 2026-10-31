//! T-262: the engine works through the project's own remote.
//!
//! A team that calls its remote `upstream` names it in `[conventions].remote`;
//! the engine fetches, merges and pushes through that name, and closes the
//! branches it consumed there. The fixture here is such a repo: its remote is
//! `upstream`, and its config says so.

use std::{fs, path::Path, process::Command};

use gear_app::{admin, tasks};
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
use gear_model::{OpKind, OpState, TaskState};
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

/// One real Git repo whose remote is called `upstream` (T-262): a bare
/// `upstream.git` on `dev` and the checkout that works.
struct Repo {
    root: tempfile::TempDir,
    upstream: std::path::PathBuf,
    work: std::path::PathBuf,
}

impl Repo {
    fn new() -> Repo {
        let root = tempfile::tempdir().unwrap();
        let upstream = root.path().join("upstream.git");
        let work = root.path().join("work");
        sh(
            root.path(),
            &format!("git init -q --bare -b dev {}", upstream.display()),
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
            &format!("git remote add upstream {}", upstream.display()),
        );
        Repo::publish(&work, "dev");
        Repo {
            root,
            upstream,
            work,
        }
    }

    /// Move `dir`'s HEAD onto `branch` on `upstream`, from the remote's own
    /// side (T-107): no engine test runs `git push`.
    fn publish(dir: &Path, branch: &str) {
        sh(
            dir,
            &format!(
                "upstream=$(git remote get-url upstream); \
                 git -C \"$upstream\" fetch -q \"$(pwd)\" +HEAD:refs/heads/{branch} && \
                 git fetch -q upstream"
            ),
        );
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
        Repo::publish(&self.work, branch);
        sh(&self.work, &format!("git rev-parse {branch}"))
    }

    /// The reviewer's work: its review branch, cut at the candidate the lane
    /// handed in, pushed.
    fn review_work(&self, branch: &str, candidate: &str) {
        sh(
            &self.work,
            &format!(
                "git switch -qc {branch} {candidate} && echo review > review.txt && git add -A \
                 && git -c user.email=a@b -c user.name=a commit -qm review"
            ),
        );
        Repo::publish(&self.work, branch);
    }

    /// A checkout of the merge branch, as Herdr would return it.
    fn merge_worktree(&self, branch: &str) -> std::path::PathBuf {
        let path = self.root.path().join("merge-worktree");
        sh(
            self.root.path(),
            &format!(
                "git -C {} worktree add -q -b {branch} {} upstream/dev",
                self.work.display(),
                path.display()
            ),
        );
        path
    }

    /// The project config: its remote is `upstream` (T-262).
    fn config(&self) -> String {
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
remote = "upstream"
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
        // T-107: the merge's remote runner moves refs from the remote's own
        // side; no engine test runs `git push`.
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

/// Onboarded project, a Ready task and its contract.
fn world(config: &str) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    admin::init_with_home(
        &mut store,
        &gear_app::Env {
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
                milestone: Some("M5".into()),
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

/// T-262: with `[conventions].remote = "upstream"` the engine reads the task
/// branch from `upstream`, merges, pushes the integration branch to
/// `upstream`, and closes the branches it consumed there.
#[test]
fn the_engine_fetches_merges_and_pushes_through_the_projects_remote() {
    let repo = Repo::new();
    let config = repo.config();
    assert_eq!(
        gear_app::config::parse(&config).unwrap().conventions.remote,
        "upstream",
        "the fixture's config names its remote"
    );
    let mut store = world(&config);
    let mut herdr = FakeHerdr::new();
    let cfg = cfg();

    let active = onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1).unwrap();
    assert_eq!(active, vec!["p:heat".to_string()]);
    let (_, branch, base, _) = created(&herdr, "p/heat");
    assert_eq!(branch, "p/heat");
    assert_eq!(base, "dev");

    // The dispatch names the project's remote: the lane fetches and starts
    // from `upstream`.
    let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    let actions = vec![(seat, Action::Assign { task: "T-1".into() })];
    store
        .write(|tx| intend_dispatch(tx, "p", &actions, T0 + 10).map(|_| ()))
        .unwrap();
    let text = store
        .read(|tx| {
            Ok(tx
                .ops_of_seat_kind("p:heat", OpKind::Deliver)?
                .into_iter()
                .last()
                .map(|op| op.payload["text"].as_str().unwrap_or_default().to_string())
                .unwrap_or_default())
        })
        .unwrap();
    assert!(
        text.contains("git fetch upstream"),
        "the packet names the remote: {text}"
    );
    assert!(
        text.contains("upstream/dev"),
        "the packet starts from the remote's branch: {text}"
    );

    // The lane works, hands in, the reviewer reviews, the review passes.
    let candidate = repo.lane_work("heat/t1");
    store
        .write(|tx| {
            // The dispatch above already started the attempt.
            tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
            tasks::submit_result(
                tx,
                "p",
                "T-1",
                1,
                1,
                &candidate,
                "gear/voyages/M5/reports/T-1.md",
                T0,
            )?;
            Ok(())
        })
        .unwrap();
    let report = start_reviews(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T0 + 10).unwrap();
    assert_eq!(report.started, vec!["p:review-1"]);
    repo.review_work("review/t1", &candidate);
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

    // The merge runs on the fixture whose remote is `upstream`.
    let merge_worktree = repo.merge_worktree("p/merge");
    herdr.add_worktree("p/merge", &merge_worktree.to_string_lossy(), "wt-merge:p1");
    wait_merge(&mut store, &mut herdr, &cfg);

    // What landed is on `upstream`'s integration branch.
    let log = sh(&repo.upstream, "git log --format=%s dev");
    assert!(
        log.contains("work"),
        "the task branch landed on upstream: {log}"
    );
    assert!(
        log.contains("review"),
        "the review branch landed on upstream: {log}"
    );
    // The branches the merge consumed are closed on `upstream`.
    assert_eq!(
        sh(&repo.upstream, "git branch --list heat/t1"),
        "",
        "the task branch is closed on upstream"
    );
    assert_eq!(
        sh(&repo.upstream, "git branch --list review/t1"),
        "",
        "the review branch is closed on upstream"
    );
}
