//! T-59: the M2.1 done-sentence, proven end to end with real Git. A lane
//! bound to a child repo is onboarded in that repo, reviewed there and
//! merged into that repo's own integration branch, with that repo's gate.
//! The project root is untouched.
//!
//! The Herdr side is the fake of the other tests; every git call is real: the
//! repos, the branches, the merge, the push and the gate.

use std::{fs, path::Path, process::Command};

use gear_app::{admin, tasks, Env};
use gear_engine::{
    dispatch::task_branch, merge::merge_passed, onboard::onboard_queued, review::start_reviews,
    EngineCfg,
};
use gear_herdr::{builtin, fake::{Call, FakeHerdr}};
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

/// Move `dir`'s HEAD onto `branch` in its origin, from the origin's own side
/// (T-107): the engine's tests never run `git push`.
fn publish(dir: &Path, branch: &str) {
    sh(
        dir,
        &format!(
            "origin=$(git remote get-url origin); \
             git -C \"$origin\" fetch -q \"$(pwd)\" +HEAD:refs/heads/{branch}"
        ),
    );
}

/// The real Git world of the test: a parent repo (a bare `origin`, the branch
/// `dev`) and a child repo cloned into a gitignored `child/` path of it (its
/// own bare `origin`, the branch `feat`).
struct World {
    root: tempfile::TempDir,
    parent: std::path::PathBuf,
    parent_origin: std::path::PathBuf,
    child: std::path::PathBuf,
    child_origin: std::path::PathBuf,
}

impl World {
    fn new() -> World {
        let root = tempfile::tempdir().unwrap();
        let parent_origin = root.path().join("parent-origin.git");
        let parent = root.path().join("parent");
        let child_origin = root.path().join("child-origin.git");
        let seed = root.path().join("child-seed");

        // The parent: a bare origin and a checkout on `dev`.
        sh(root.path(), &format!("git init -q --bare {}", parent_origin.display()));
        sh(root.path(), &format!("git init -q -b dev {}", parent.display()));
        fs::write(parent.join(".gitignore"), "child/\n").unwrap();
        fs::write(parent.join("README.md"), "parent\n").unwrap();
        sh(&parent, "git add -A && git -c user.email=a@b -c user.name=a commit -qm base");
        sh(&parent, &format!("git remote add origin {}", parent_origin.display()));
        publish(&parent, "dev");

        // The child: its own bare origin and its own branch `feat`.
        sh(root.path(), &format!("git init -q --bare -b feat {}", child_origin.display()));
        sh(root.path(), &format!("git init -q -b feat {}", seed.display()));
        fs::write(seed.join("README.md"), "child\n").unwrap();
        sh(&seed, "git add -A && git -c user.email=a@b -c user.name=a commit -qm base");
        sh(&seed, &format!("git remote add origin {}", child_origin.display()));
        publish(&seed, "feat");

        // … cloned into the parent's gitignored path: the parent never sees
        // the child's files.
        sh(&parent, &format!("git clone -q {} child", child_origin.display()));
        let child = parent.join("child");
        assert_eq!(sh(&child, "git rev-parse --abbrev-ref HEAD"), "feat");
        assert_eq!(
            sh(&parent, "git status --porcelain"),
            "",
            "the parent ignores the child repo"
        );
        World { root, parent, parent_origin, child, child_origin }
    }

    /// The lane's work: a commit on the lane's task branch, pushed to the
    /// child's origin. The commit is the candidate the reviewer reads.
    fn lane_work(&self, branch: &str) -> String {
        sh(
            &self.child,
            &format!(
                "git switch -qc {branch} feat && echo work >> work.txt && git add -A \
                 && git -c user.email=a@b -c user.name=a commit -qm work"
            ),
        );
        publish(&self.child, branch);
        sh(&self.child, &format!("git rev-parse {branch}"))
    }

    /// The reviewer's work: the review branch, pushed to the child's origin.
    /// The merge publishes it with the candidate.
    fn review_work(&self, branch: &str) {
        sh(
            &self.child,
            &format!(
                "git switch -qc {branch} feat && echo review > review.txt && git add -A \
                 && git -c user.email=a@b -c user.name=a commit -qm review"
            ),
        );
        publish(&self.child, branch);
    }

    /// The engine's integration worktree of the child repo: `p/merge`,
    /// cut from the child's `feat`.
    fn merge_worktree(&self) -> std::path::PathBuf {
        let path = self.root.path().join("child-merge");
        sh(
            self.root.path(),
            &format!(
                "git -C {} worktree add -q -b p/merge {} origin/feat",
                self.child.display(),
                path.display()
            ),
        );
        path
    }

    /// The project config: the parent is the project, the child is a repo of
    /// it with its own branch and its own gate.
    fn config(&self, gate: &str) -> String {
        format!(
            r#"
[project]
id = "p"
herdr_session = "sess"
repo = "{parent}"

[[repos]]
name = "child"
path = "child"
integration = "feat"
gate = "{gate}"

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
repo = "child"

[temps]
kind = "claude"
profile = "claude"
"#,
            parent = self.parent.display(),
        )
    }
}

/// The engine knows the profiles the project uses.
fn cfg() -> EngineCfg {
    let mut cfg = EngineCfg {
        db_path: crate::temp("gear-engine-tests").join("gear.db").to_string_lossy().into_owned(),
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

/// The worktree the engine created for `branch`: (repo, branch, base, label).
fn created(herdr: &FakeHerdr, branch: &str) -> (String, String, String, String) {
    herdr.calls
        .iter()
        .find_map(|call| match call {
            Call::WorktreeCreate { repo, branch: made, base, label } if made == branch => {
                Some((repo.clone(), made.clone(), base.clone(), label.clone()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("the engine created the worktree {branch}: {:?}", herdr.calls))
}

/// The worktree the engine opened for `branch`: (repo, branch, label).
fn opened(herdr: &FakeHerdr, branch: &str) -> (String, String, String) {
    herdr.calls
        .iter()
        .find_map(|call| match call {
            Call::WorktreeOpen { repo, branch: made, label } if made == branch => {
                Some((repo.clone(), made.clone(), label.clone()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("the engine opened the worktree {branch}: {:?}", herdr.calls))
}

/// Wait for the merge to finish; it must be confirmed.
///
/// The wait is on the merge worker's own report — a condition, never the clock
/// (T-127); only a worker that hung fails the test.
fn wait_merge(store: &mut Store, herdr: &mut FakeHerdr, cfg: &EngineCfg) {
    const WAIT: std::time::Duration = std::time::Duration::from_secs(60);
    loop {
        let seen = cfg.merges.progress();
        merge_passed(store, herdr, cfg, "p", T0 + 40).unwrap();
        let op = store
            .read(|tx| {
                Ok(tx.ops_in_state("p", OpState::Confirmed)?
                    .into_iter()
                    .find(|op| op.kind == OpKind::Merge))
            })
            .unwrap();
        if let Some(op) = op {
            assert_eq!(op.state, OpState::Confirmed, "{op:?}");
            return;
        }
        let states = store
            .read(|tx| {
                Ok(tx.ops_in_state("p", OpState::Failed)?
                    .into_iter()
                    .find(|op| op.kind == OpKind::Merge)
                    .map(|op| op.result))
            })
            .unwrap();
        assert!(states.is_none(), "the merge failed: {states:?}");
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

/// T-59: the done-sentence, end to end. A lane bound to a child repo works in
/// that repo: its worktree is there, its review is there, and its work is
/// merged into that repo's own branch with that repo's gate. The project
/// root is not touched.
#[test]
fn a_lane_in_a_child_repo_runs_from_onboard_to_merge_in_that_repo() {
    let world = World::new();
    let marker = world.root.path().join("gate-marker");
    // The child's own gate: it writes where it ran and which repo it is.
    let gate = format!(
        "pwd > '{marker}'; git remote get-url origin >> '{marker}'",
        marker = marker.display()
    );
    let mut store = Store::open_in_memory().unwrap();
    admin::init_with_home(
        &mut store,
        &Env { ctx: None, pane_id: None, admin: true },
        &world.config(&gate),
        Some(crate::skill_home()),
        false,
        T0,
    )
    .unwrap();
    let mut herdr = FakeHerdr::new();
    let cfg = cfg();

    // 1. onboard: the lane's worktree is in the child repo, cut from the
    //    child's own branch.
    let active = onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1).unwrap();
    assert_eq!(active, vec!["p:heat".to_string()]);
    let (repo, branch, base, label) = created(&herdr, "p/heat");
    assert_eq!(repo, world.child.to_string_lossy().to_string(), "the child repo");
    assert_eq!(branch, "p/heat");
    assert_eq!(base, "feat", "the child's own integration branch");
    assert_eq!(label, "p-heat");

    // 2. the lane works in its repo and hands the result in.
    let branch = task_branch(gear_model::branch::TASK_BRANCH, "p", "heat", "T-1");
    let candidate = world.lane_work(&branch);
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
                 issue: None,updated: T0,
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
            tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
            tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
            tasks::submit_result(
                tx,
                "p",
                "T-1",
                1,
                1,
                &candidate,
                "ops/reports/T-1.md",
                T0,
            )?;
            Ok(())
        })
        .unwrap();

    // 3. the review: the reviewer works in the task's repo.
    let report = start_reviews(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T0 + 10).unwrap();
    assert_eq!(report.started, vec!["p:review-1"]);
    let (repo, branch, base, label) = created(&herdr, "p/review-1");
    assert_eq!(repo, world.child.to_string_lossy().to_string(), "the child repo");
    assert_eq!(branch, "p/review-1");
    assert_eq!(base, "feat", "the child's own integration branch");
    assert_eq!(label, "p-review-1-child", "the label names the repo");
    let checkout = crate::REVIEW_GIT_CALLS.with(|calls| calls.borrow().clone());
    assert!(checkout.iter().any(|args| args == &["checkout", "-f", "-B", "review/t1", candidate.as_str()]));
    // The packet asks for the diff against the child's integration branch.
    let packet = store
        .read(|tx| {
            Ok(tx.ops_of_seat_kind("p:review-1", OpKind::Deliver)?
                .into_iter()
                .last()
                .map(|op| op.payload["text"].as_str().unwrap_or_default().to_string())
                .unwrap_or_default())
        })
        .unwrap();
    assert!(
        packet.contains(&format!(
            "Diff to review: `git fetch origin && git diff origin/feat...{candidate}` (three dots: the change since the merge base)"
        )),
        "{packet}"
    );

    // 4. the reviewer passes; the task is settled and its merge is queued.
    world.review_work("review/t1");
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

    // 5. the merge: in the child repo, on the child's branch, with the
    //    child's gate.
    let merge_worktree = world.merge_worktree();
    herdr.add_worktree("p/merge", &merge_worktree.to_string_lossy(), "wt-merge:p1");
    wait_merge(&mut store, &mut herdr, &cfg);
    let (repo, branch, label) = opened(&herdr, "p/merge");
    assert_eq!(repo, world.child.to_string_lossy().to_string(), "the child repo");
    assert_eq!(branch, "p/merge");
    assert_eq!(label, "p-merge-child", "the label names the repo");

    // The child's branch holds the lane's work and the review.
    let log = sh(&world.child_origin, "git log --format=%s feat");
    assert!(log.contains("work"), "{log}");
    assert!(log.contains("review"), "{log}");

    // The child's gate ran, in the child's merge worktree, on the child's
    // repo.
    let ran = fs::read_to_string(&marker).expect("the child's gate ran");
    let mut lines = ran.lines();
    let where_ = lines.next().expect("the gate wrote where it ran");
    let origin = lines.next().expect("the gate wrote which repo it is");
    assert_eq!(
        fs::canonicalize(where_).unwrap(),
        fs::canonicalize(&merge_worktree).unwrap(),
        "the gate ran in the child's merge worktree"
    );
    assert_eq!(origin, world.child_origin.to_string_lossy(), "the child's repo");

    // The parent is untouched: its branch never moved.
    assert_eq!(
        sh(&world.parent_origin, "git log --format=%s dev"),
        "base",
        "the parent's dev was merged into"
    );
}
