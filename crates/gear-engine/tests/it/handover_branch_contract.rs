//! T-513: a model-switch handover names the branch of the task the lane holds,
//! not whatever branch its worktree happens to be on.
//!
//! A switch right after a new task was assigned happens while the worktree is
//! still on the previous task's branch (F-284): the handover names the new
//! task's branch and adds one line saying which branch the worktree is on. A
//! lane already on its task's branch, and a reviewer, add no such line.

use std::path::{Path, PathBuf};
use std::process::Command;

use gear_engine::{handover, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::SeatState;
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, ContractRow, Store, TaskRow};

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

/// One real repository with the branches a lane works on.
struct Repo {
    _root: tempfile::TempDir,
    work: PathBuf,
}

impl Repo {
    fn new() -> Repo {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        sh(
            &root.path(),
            &format!("git init -q -b dev {}", work.display()),
        );
        std::fs::write(work.join("README.md"), "base\n").unwrap();
        sh(
            &work,
            "git add -A && git -c user.email=a@b -c user.name=a commit -qm base",
        );
        for branch in ["hull/t1", "hull/t2", "review/t2"] {
            sh(
                &work,
                &format!(
                    "git switch -qc {branch} && echo {branch} > {}.txt && git add -A \
                     && git -c user.email=a@b -c user.name=a commit -qm {branch}",
                    branch.replace('/', "-")
                ),
            );
        }
        Repo { _root: root, work }
    }

    /// Put the worktree on `branch`.
    fn on(&self, branch: &str) {
        sh(&self.work, &format!("git switch -q {branch}"));
    }
}

/// The git the handover reads: the fixture's own repository.
#[derive(Debug)]
struct RealGit;

impl gear_engine::review::ReviewGit for RealGit {
    fn run(&self, path: &str, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

fn config_toml(repo: &Path) -> String {
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
kind = "codex"
profile = "codex"

[[lanes]]
name = "hull"
duty = "everything"
owns = ["src/"]
kind = "pi"
profile = "pi"

[temps]
kind = "codex"
profile = "codex"

[conventions]
integration = "dev"
release = "main"
gate = "true"
task_branch = "hull/t{{n}}"
"#,
        repo = repo.display()
    )
}

/// A task of the fixture.
fn task(id: &str, rank: i64, state: TaskState) -> TaskRow {
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
        lane: Some("hull".into()),
        milestone: None,
        required: false,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(20),
        issue: None,
        updated: T0,
    }
}

/// An attempt row of a task on its seat.
fn attempt(id: &str, seat: &str, state: AttemptState) -> AttemptRow {
    AttemptRow {
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
        project: "p".into(),
        task: id.into(),
        n: 1,
        seat: seat.into(),
        generation: 1,
        contract_version: 1,
        adopted_version: 1,
        pending_update: None,
        state,
        candidate: None,
        report: None,
        safe_point: None,
        started: T0,
        updated: T0,
    }
}

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
    repo: Repo,
}

/// A project with the lane `hull` and a reviewer seat; T-1 and T-2 exist and
/// the lane's attempt is on T-2 (as it is right after a new task was assigned).
fn world(reviewer: bool) -> World {
    let repo = Repo::new();
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            let mut row = tx.get_project("p")?.unwrap();
            row.config_toml = config_toml(&repo.work);
            tx.update_project(&row)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", T0)?;
            for (id, rank, state) in [("T-1", 1, TaskState::Done), ("T-2", 2, TaskState::Running)] {
                tx.insert_task(&task(id, rank, state))?;
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: id.into(),
                    version: 1,
                    goal: format!("the goal of {id}"),
                    accept: "the acceptance".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                    mechanical: None,
                })?;
            }
            tx.insert_attempt(&attempt("T-2", "p:hull", AttemptState::Started))?;
            if reviewer {
                testing::seed_temp_seat(tx, "p", "review-1", "T-2", "w2:p1", "codex", T0)?;
            }
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named("w1:p1", "codex", AgentStatus::Idle, "p-captain");
    herdr.add_agent_named("w1:p2", "pi", AgentStatus::Working, "p-hull");
    herdr.add_agent_named("w2:p1", "codex", AgentStatus::Working, "p-review-1");
    let mut cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        ..Default::default()
    };
    cfg.review_git = std::sync::Arc::new(RealGit);
    World {
        store,
        herdr,
        cfg,
        repo,
    }
}

/// The handover of a seat, as the incoming agent's packet says it.
fn handover_of(w: &mut World, seat_id: &str) -> String {
    let seat = w.store.read(|tx| tx.get_seat(seat_id)).unwrap().unwrap();
    let worktree = w.repo.work.clone();
    w.store
        .write(|tx| {
            tx.set_meta(
                &format!("engine.worktree_path.{seat_id}"),
                &worktree.display().to_string(),
            )
        })
        .unwrap();
    let facts = handover::collect(
        &mut w.store,
        &w.cfg,
        &mut w.herdr,
        "p",
        &seat,
        "pi",
        "the old model",
        "codex",
        "the new model",
        T0 + 10,
    )
    .unwrap()
    .expect("the seat has a task to hand over");
    handover::block(&facts)
}

#[test]
fn a_lane_switched_on_the_previous_branch_gets_its_own_tasks_branch() {
    let mut w = world(false);
    w.repo.on("hull/t1");
    let block = handover_of(&mut w, "p:hull");
    assert!(
        block.contains("- Branch: hull/t2."),
        "the handover names the attempt's task branch: {block}"
    );
    assert!(
        block.contains(
            "- The worktree is on hull/t1; this task's branch is hull/t2: switch to it before you go on."
        ),
        "the switch line: {block}"
    );
}

#[test]
fn a_lane_already_on_its_tasks_branch_gets_no_switch_line() {
    let mut w = world(false);
    w.repo.on("hull/t2");
    let block = handover_of(&mut w, "p:hull");
    assert!(block.contains("- Branch: hull/t2."), "{block}");
    assert!(
        !block.contains("switch to it before you go on"),
        "no switch line when the worktree is on the task's branch: {block}"
    );
}

#[test]
fn a_reviewers_handover_names_the_branch_of_its_worktree() {
    let mut w = world(true);
    w.repo.on("review/t2");
    let block = handover_of(&mut w, "p:review-1");
    assert!(
        block.contains("- Branch: review/t2."),
        "a reviewer's branch is the one checked out for it: {block}"
    );
    assert!(
        !block.contains("switch to it before you go on"),
        "a reviewer gets no switch line: {block}"
    );
}

#[test]
fn the_facts_of_the_worktree_stay_those_of_the_worktree() {
    // T-513: only the branch comes from the attempt; the last commit and the
    // changed files are the worktree's own, whichever branch it is on.
    let mut w = world(false);
    w.repo.on("hull/t1");
    std::fs::write(w.repo.work.join("README.md"), "changed\n").unwrap();
    let block = handover_of(&mut w, "p:hull");
    assert!(block.contains("- Branch: hull/t2."), "{block}");
    assert!(
        block.contains("- Last commit: ") && block.contains("hull/t1"),
        "the last commit is the worktree's own: {block}"
    );
    assert!(
        block.contains("- Uncommitted changes: README.md"),
        "the changed file is the worktree's: {block}"
    );
}

#[test]
fn a_seat_between_tasks_gets_no_handover() {
    let mut w = world(false);
    w.store
        .write(|tx| {
            tx.set_attempt_time("p", "T-2", 1, gear_store::AttemptStep::Settled, T0)?;
            let mut row = tx.get_attempt("p", "T-2", 1)?.unwrap();
            row.state = AttemptState::Accepted;
            tx.update_attempt(&row)?;
            tx.set_seat_state("p:hull", SeatState::Released, Some("between tasks"), T0)
        })
        .unwrap();
    let seat = w.store.read(|tx| tx.get_seat("p:hull")).unwrap().unwrap();
    let facts = handover::collect(
        &mut w.store,
        &w.cfg,
        &mut w.herdr,
        "p",
        &seat,
        "pi",
        "the old model",
        "codex",
        "the new model",
        T0 + 10,
    )
    .unwrap();
    assert!(
        facts.is_none(),
        "a seat with no attempt has nothing to hand over"
    );
}
