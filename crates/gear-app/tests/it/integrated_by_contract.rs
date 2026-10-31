//! Contract for `integrated-by=` (design §10.7 row B03-01): a pass may name
//! the task whose merge landed the work; the task is then marked merged by
//! that merge, and the verdict is refused unless that merge is merged and
//! the work is in the integration branch.
//!
//! The git side is exercised against a scratch repository with a bare origin;
//! the store side proves what the settle writes.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole};
use gear_store::{testing, AttemptStep, Store};
use std::path::{Path, PathBuf};
use std::process::Command;

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "gate")
        .env("GIT_AUTHOR_EMAIL", "gate@example.com")
        .env("GIT_COMMITTER_NAME", "gate")
        .env("GIT_COMMITTER_EMAIL", "gate@example.com")
        .output()
        .unwrap_or_else(|e| panic!("cannot run git: {e}"));
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A scratch repository with a bare origin: the repo the lane works in, its
/// `dev` branch, and the sha of a commit whose tree is on that branch.
struct Repos {
    _root: tempfile::TempDir,
    /// The repo the config names.
    repo: PathBuf,
    /// A commit whose tree is on `origin/dev`.
    candidate: String,
}

impl Repos {
    fn new() -> Repos {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "--bare", "--initial-branch=dev"]);
        let url = origin.to_str().unwrap().to_string();
        let repo = root.path().join("repo");
        git(root.path(), &["clone", "-q", &url, repo.to_str().unwrap()]);
        git(&repo, &["checkout", "-q", "-b", "dev"]);
        std::fs::write(repo.join("work.txt"), "the work\n").unwrap();
        git(&repo, &["add", "work.txt"]);
        git(&repo, &["commit", "-q", "-m", "the work"]);
        let candidate = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["push", "-q", "origin", "dev"]);
        Repos {
            _root: root,
            repo,
            candidate,
        }
    }

    /// The integration branch moves on: a commit on top of the candidate.
    fn advance_dev(&self) -> String {
        git(&self.repo, &["checkout", "-q", "dev"]);
        std::fs::write(self.repo.join("more.txt"), "more\n").unwrap();
        git(&self.repo, &["add", "more.txt"]);
        git(&self.repo, &["commit", "-q", "-m", "more work"]);
        git(&self.repo, &["push", "-q", "origin", "dev"]);
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    /// A commit with the same tree as the candidate but no ancestry: it is not
    /// the work that landed (contract v3 of T-165).
    fn unrelated_with_the_same_tree(&self) -> String {
        git(
            &self.repo,
            &[
                "commit-tree",
                &format!("{}^{{tree}}", self.candidate),
                "-m",
                "unrelated",
            ],
        )
    }

    /// A commit that is not on the integration branch: its tree is nowhere in
    /// `origin/dev`.
    fn other_candidate(&self) -> String {
        git(&self.repo, &["checkout", "-q", "-b", "side"]);
        std::fs::write(self.repo.join("other.txt"), "other\n").unwrap();
        git(&self.repo, &["add", "other.txt"]);
        git(&self.repo, &["commit", "-q", "-m", "other work"]);
        git(&self.repo, &["rev-parse", "HEAD"])
    }
}

fn config(repo: &Path) -> String {
    format!(
        r#"
[project]
id = "p"
herdr_session = "sess"
repo = "{}"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/heat/"]
kind = "opencode"
profile = "opencode"

[conventions]
integration = "dev"
release = "main"
gate = "make test"
"#,
        repo.display()
    )
}

/// A project with T-1 (worked on, handed in) and T-2 (merged: its attempt
/// carries the merge time).
fn world(repos: &Repos, candidate: &str) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        let mut project = testing::seed_project(tx, "p", "sess", NOW)?;
        project.config_toml = config(&repos.repo);
        tx.update_project(&project)?;
        testing::seed_active_seat(
            tx,
            "p",
            "captain",
            SeatRole::Captain,
            CAP_PANE,
            "codex",
            NOW,
        )?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", NOW)?;
        for id in ["T-1", "T-2"] {
            tx.insert_task(&gear_store::TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: id.into(),
                rank: 1,
                state: gear_model::TaskState::Ready,
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
                updated: NOW,
            })?;
            tx.insert_contract(&gear_store::ContractRow {
                project: "p".into(),
                task: id.into(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: NOW,
                mechanical: None,
            })?;
        }
        // T-1's work is handed in.
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, NOW)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, NOW)?;
        gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, candidate, "ops/reports/T-1.md", NOW)?;
        // T-2 is merged: its merge landed the work. The task is Done and its
        // merge op is confirmed (contract v3 of T-165: both are needed).
        gear_app::tasks::start_attempt(tx, "p", "T-2", "p:heat", 1, NOW)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-2", 1, NOW)?;
        gear_app::tasks::submit_result(tx, "p", "T-2", 1, 1, "c2", "ops/reports/T-2.md", NOW)?;
        let mut task = tx.get_task("p", "T-2")?.unwrap();
        task.state = gear_model::TaskState::Done;
        tx.update_task(&task)?;
        let merge = tx.insert_op(&gear_store::NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: gear_model::OpKind::Merge,
            task: Some("T-2".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({"branch": "heat/t2", "candidate": "c2"}),
            created: NOW,
        })?;
        tx.set_op_state(
            "p",
            &merge.id,
            gear_model::OpState::Confirmed,
            Some("merged T-2 into dev"),
            NOW + 10,
        )?;
        tx.set_attempt_time("p", "T-2", 1, AttemptStep::Merged, NOW + 10)
    })
    .unwrap();
    s
}

fn cap() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx {
                project: "p".into(),
                seat: "captain".into(),
                generation: 1,
            }),
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        },
        Fingerprint {
            pane_id: CAP_PANE.into(),
            kind: Some("codex".into()),
            session_ref: None,
        },
    )
}

fn run(
    s: &mut Store,
    text: &str,
    rid: &str,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let (env, fp) = cap();
    apply(
        s,
        &env,
        Some(&fp),
        None,
        &parse(text).unwrap(),
        text,
        rid,
        NOW + 20,
    )
}

/// A pass that names the task whose merge landed the work: the task is
/// marked merged by that merge.
#[test]
fn a_pass_naming_the_merge_that_landed_the_work_is_marked_merged_by_it() {
    let repos = Repos::new();
    let mut s = world(&repos, &repos.candidate);
    let text = format!(
        "<!-- gear-plan project=p base=0 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={} pass evidence=ops/reports/T-1.md integrated-by=T-2\n",
        repos.candidate
    );

    run(&mut s, &text, "r1").unwrap();

    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.merged, Some(NOW + 10), "marked merged by T-2's merge");
    let by = s
        .read(|tx| tx.get_meta("integrated-by:p:T-1"))
        .unwrap()
        .expect("the merge that landed the work is recorded");
    assert_eq!(by, "T-2");
}

/// The named task is not merged: the verdict is refused.
#[test]
fn a_pass_naming_a_task_that_is_not_merged_is_refused() {
    let repos = Repos::new();
    let mut s = world(&repos, &repos.candidate);
    // T-9 is a task that was never merged at all.
    let text = format!(
        "<!-- gear-plan project=p base=0 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={} pass evidence=ops/reports/T-1.md integrated-by=T-9\n",
        repos.candidate
    );

    let err = run(&mut s, &text, "r1").unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat, "{err:?}");
    assert!(err.detail.contains("T-9"), "{}", err.detail);
    // Nothing was settled: the attempt is still handed in.
    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.merged, None);
}

/// The work is not in the integration branch: the verdict is refused.
#[test]
fn a_pass_naming_a_merge_whose_work_is_not_merged_is_refused() {
    let repos = Repos::new();
    let other = repos.other_candidate();
    let mut s = world(&repos, &other);
    let text = format!(
        "<!-- gear-plan project=p base=0 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={other} pass evidence=ops/reports/T-1.md integrated-by=T-2\n"
    );

    let err = run(&mut s, &text, "r1").unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat, "{err:?}");
    assert!(err.detail.contains("not in dev"), "{}", err.detail);
    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.merged, None);
}

/// T-165 (review F1): the attribution may arrive **after** the attempt was
/// accepted — the normal path after an auto-settled review or a failed merge.
/// The second plan carries the same pass plus `integrated-by=T-2`: the
/// attempt must then carry T-2's merge time and the attribution, without a
/// second verdict and without a second state change. Replaying the same plan
/// changes nothing.
#[test]
fn an_attribution_added_after_the_pass_marks_the_merged_time() {
    let repos = Repos::new();
    let mut s = world(&repos, &repos.candidate);
    // The first plan: an ordinary pass. The attempt is accepted, but nothing
    // says which merge landed the work.
    let plain = format!(
        "<!-- gear-plan project=p base=0 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={} pass evidence=ops/reports/T-1.md\n",
        repos.candidate
    );
    run(&mut s, &plain, "r1").unwrap();
    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.merged, None, "no merge named the work yet");
    assert!(
        s.read(|tx| tx.get_meta("integrated-by:p:T-1"))
            .unwrap()
            .is_none(),
        "no attribution yet"
    );

    // The second plan: the same pass, now naming the merge that landed it.
    let named = format!(
        "<!-- gear-plan project=p base=1 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={} pass evidence=ops/reports/T-1.md integrated-by=T-2\n",
        repos.candidate
    );
    run(&mut s, &named, "r2").unwrap();

    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.merged, Some(NOW + 10), "marked merged by T-2's merge");
    let by = s
        .read(|tx| tx.get_meta("integrated-by:p:T-1"))
        .unwrap()
        .expect("the merge that landed the work is recorded");
    assert_eq!(by, "T-2");
    // The attempt was already accepted: no second verdict, no second change.
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, gear_model::AttemptState::Accepted);
    let verdicts = s.read(|tx| tx.verdicts_of_task("p", "T-1")).unwrap();
    assert_eq!(verdicts.len(), 1, "one verdict, not two: {verdicts:?}");

    // The same plan applied again changes nothing.
    let again = named.replace("base=1", "base=2");
    run(&mut s, &again, "r3").unwrap();
    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.merged, Some(NOW + 10), "the replay changes nothing");
    let by = s
        .read(|tx| tx.get_meta("integrated-by:p:T-1"))
        .unwrap()
        .expect("the attribution is kept");
    assert_eq!(by, "T-2");
    let verdicts = s.read(|tx| tx.verdicts_of_task("p", "T-1")).unwrap();
    assert_eq!(verdicts.len(), 1, "the replay writes no second verdict");
}

/// T-205 (the review of T-165, F4): a commit with the same tree as the
/// candidate, but no ancestry, is not the work that landed: the verdict is
/// refused.
#[test]
fn a_commit_with_the_same_tree_but_no_ancestry_is_refused() {
    let repos = Repos::new();
    let unrelated = repos.unrelated_with_the_same_tree();
    let mut s = world(&repos, &unrelated);
    let text = format!(
        "<!-- gear-plan project=p base=0 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={unrelated} pass evidence=ops/reports/T-1.md integrated-by=T-2\n"
    );

    let err = run(&mut s, &text, "r1").unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat, "{err:?}");
    assert!(err.detail.contains("not in dev"), "{}", err.detail);
    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.merged, None, "nothing was settled");
}

/// T-205 (F4): a candidate that is an ancestor of the integration branch is
/// accepted, even when the branch has moved on since.
#[test]
fn a_candidate_that_is_an_ancestor_of_the_integration_branch_is_accepted() {
    let repos = Repos::new();
    let tip = repos.advance_dev();
    assert_ne!(tip, repos.candidate);
    let mut s = world(&repos, &repos.candidate);
    let text = format!(
        "<!-- gear-plan project=p base=0 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={} pass evidence=ops/reports/T-1.md integrated-by=T-2\n",
        repos.candidate
    );

    run(&mut s, &text, "r1").unwrap();

    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.merged, Some(NOW + 10), "marked merged by T-2's merge");
}

/// T-205 (the review of T-165, F5): the named task must be Done. A task
/// whose merge is confirmed but that is not done yet has landed nothing.
#[test]
fn a_pass_naming_a_task_that_is_not_done_is_refused() {
    let repos = Repos::new();
    let mut s = world(&repos, &repos.candidate);
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-2")?.unwrap();
        task.state = gear_model::TaskState::Verifying;
        tx.update_task(&task)
    })
    .unwrap();
    let text = format!(
        "<!-- gear-plan project=p base=0 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={} pass evidence=ops/reports/T-1.md integrated-by=T-2\n",
        repos.candidate
    );

    let err = run(&mut s, &text, "r1").unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat, "{err:?}");
    assert!(err.detail.contains("not done"), "{}", err.detail);
    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.merged, None, "nothing was settled");
}

/// T-205 (F5): the named task must have a confirmed merge op. A merge time
/// without one is a leftover, not a merge.
#[test]
fn a_pass_naming_a_done_task_without_a_confirmed_merge_is_refused() {
    let repos = Repos::new();
    let mut s = world(&repos, &repos.candidate);
    // The merge op is not confirmed any more: the merge did not go through.
    s.write(|tx| {
        let op = tx
            .ops_in_state("p", gear_model::OpState::Confirmed)?
            .into_iter()
            .find(|op| op.kind == gear_model::OpKind::Merge)
            .expect("the merge op");
        tx.set_op_state(
            "p",
            &op.id,
            gear_model::OpState::Failed,
            Some("the gate said no"),
            NOW + 11,
        )
    })
    .unwrap();
    let text = format!(
        "<!-- gear-plan project=p base=0 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={} pass evidence=ops/reports/T-1.md integrated-by=T-2\n",
        repos.candidate
    );

    let err = run(&mut s, &text, "r1").unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat, "{err:?}");
    assert!(err.detail.contains("confirmed merge"), "{}", err.detail);
    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.merged, None, "nothing was settled");
}
