//! A project run by its captain alone (M7.2): no lanes and no reviewers; the
//! captain plans, works and records its own tasks.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole};
use gear_store::{testing, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

const ALONE: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 0
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"
"#;

const WITH_A_LANE: &str = r#"
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
duty = "thermal calculation"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
"#;

fn world(config: &str) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        let mut project = testing::seed_project(tx, "p", "sess", NOW)?;
        project.config_toml = config.to_string();
        tx.update_project(&project)?;
        testing::seed_active_seat(
            tx,
            "p",
            "captain",
            SeatRole::Captain,
            CAP_PANE,
            "claude",
            NOW,
        )?;
        Ok(())
    })
    .unwrap();
    s
}

fn run(s: &mut Store, body: &str) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let base = s.read(|tx| tx.plan_version("p")).unwrap();
    let text = format!("<!-- gear-plan project=p base={base} -->\n{body}");
    let env = Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: "captain".into(),
            generation: 1,
        }),
        pane_id: Some(CAP_PANE.into()),
        admin: false,
    };
    let fp = Fingerprint {
        pane_id: CAP_PANE.into(),
        kind: Some("claude".into()),
        session_ref: None,
    };
    apply(
        s,
        &env,
        Some(&fp),
        None,
        &parse(&text).unwrap(),
        &text,
        &format!("r{base}"),
        NOW + 10,
    )
}

const NO_LANE: &str = "# Queue\n## NEW [must]\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";

#[test]
fn a_captain_alone_plans_a_task_without_a_lane() {
    let mut s = world(ALONE);
    let r = run(&mut s, NO_LANE).unwrap();
    assert_eq!(r.created, vec!["T-1".to_string()]);
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.lane, None, "the task is the captain's");
}

#[test]
fn a_project_with_lanes_still_needs_a_lane() {
    let mut s = world(WITH_A_LANE);
    let err = run(&mut s, NO_LANE).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("needs `lane=`"), "{}", err.detail);
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "gate")
        .env("GIT_AUTHOR_EMAIL", "gate@example.com")
        .env("GIT_COMMITTER_NAME", "gate")
        .env("GIT_COMMITTER_EMAIL", "gate@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A scratch repo with a bare origin: `landed` is pushed to origin/dev,
/// `local` is committed on a side branch and pushed nowhere.
struct Repo {
    _root: tempfile::TempDir,
    path: std::path::PathBuf,
    landed: String,
    local: String,
}

fn repo() -> Repo {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare", "--initial-branch=dev"]);
    let path = root.path().join("repo");
    git(
        root.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            path.to_str().unwrap(),
        ],
    );
    git(&path, &["checkout", "-q", "-b", "dev"]);
    std::fs::write(path.join("a.txt"), "a\n").unwrap();
    git(&path, &["add", "a.txt"]);
    git(&path, &["commit", "-q", "-m", "landed"]);
    let landed = git(&path, &["rev-parse", "HEAD"]);
    git(&path, &["push", "-q", "origin", "dev"]);
    git(&path, &["checkout", "-q", "-b", "side"]);
    std::fs::write(path.join("b.txt"), "b\n").unwrap();
    git(&path, &["add", "b.txt"]);
    git(&path, &["commit", "-q", "-m", "local"]);
    let local = git(&path, &["rev-parse", "HEAD"]);
    Repo {
        _root: root,
        path,
        landed,
        local,
    }
}

fn alone_in(repo: &Repo) -> Store {
    let config = ALONE.replace(
        "repo = \"/repo\"",
        &format!("repo = \"{}\"", repo.path.display()),
    );
    let mut s = world(&format!(
        "{config}\n[conventions]\nintegration = \"dev\"\nrelease = \"main\"\n"
    ));
    run(&mut s, NO_LANE).unwrap();
    s
}

#[test]
fn the_captains_done_line_records_a_landed_commit_as_done_and_merged() {
    let repo = repo();
    let mut s = alone_in(&repo);
    let line = format!(
        "# Verdicts\n- T-1 done commit={} evidence=make gate passes\n",
        repo.landed
    );
    run(&mut s, &line).unwrap();
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.state, gear_model::TaskState::Done);
    let merged = s.read(|tx| tx.merged_done_tasks("p")).unwrap();
    assert_eq!(
        merged,
        vec!["T-1".to_string()],
        "the work is on dev: done means merged"
    );
    // The same line again changes nothing.
    run(&mut s, &line).unwrap();
    assert_eq!(
        s.read(|tx| tx.attempts_of_task("p", "T-1")).unwrap().len(),
        1
    );
}

#[test]
fn a_done_line_for_a_commit_not_pushed_is_refused_and_writes_nothing() {
    let repo = repo();
    let mut s = alone_in(&repo);
    let line = format!("# Verdicts\n- T-1 done commit={} evidence=x\n", repo.local);
    let err = run(&mut s, &line).unwrap_err();
    assert!(
        err.detail.contains("is not on origin/dev"),
        "{}",
        err.detail
    );
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.state, gear_model::TaskState::Ready);
    assert!(s
        .read(|tx| tx.attempts_of_task("p", "T-1"))
        .unwrap()
        .is_empty());
}

#[test]
fn a_done_line_is_refused_for_a_lanes_task() {
    let repo = repo();
    let config = WITH_A_LANE.replace(
        "repo = \"/repo\"",
        &format!("repo = \"{}\"", repo.path.display()),
    );
    let mut s = world(&format!(
        "{config}\n[conventions]\nintegration = \"dev\"\nrelease = \"main\"\n"
    ));
    run(
        &mut s,
        "# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
    )
    .unwrap();
    let err = run(
        &mut s,
        &format!("# Verdicts\n- T-1 done commit={} evidence=x\n", repo.landed),
    )
    .unwrap_err();
    assert!(
        err.detail.contains("only the captain's own task"),
        "{}",
        err.detail
    );
}
