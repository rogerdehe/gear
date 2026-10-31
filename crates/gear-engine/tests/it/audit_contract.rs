//! T-505: a voyage audit is handed in by the engine, at the integration
//! branch's tip, and reviewed as a hand-in — never worked by its lane.

use std::path::{Path, PathBuf};
use std::process::Command;

use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{AttemptState, OpKind, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const LANE_PANE: &str = "w1:p2";

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

/// One real repository: a bare origin on `dev` and the checkout the project
/// reads.
struct Repo {
    _root: tempfile::TempDir,
    work: PathBuf,
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
        std::fs::write(work.join("README.md"), "base\n").unwrap();
        sh(
            &work,
            "git add -A && git -c user.email=a@b -c user.name=a commit -qm base",
        );
        sh(
            &work,
            &format!("git remote add origin {}", origin.display()),
        );
        // The tip of `dev` reaches the origin and comes back as a remote
        // branch, which is what the audit reads.
        sh(
            &work,
            "origin=$(git remote get-url origin); \
             git -C \"$origin\" fetch -q \"$(pwd)\" +HEAD:refs/heads/dev && git fetch -q origin",
        );
        Repo { _root: root, work }
    }

    fn tip(&self) -> String {
        sh(&self.work, "git rev-parse origin/dev")
    }
}

/// The git the engine runs: a fetch and a checkout succeed, and the branch tips
/// are the fixture's own.
#[derive(Debug)]
struct AuditGit {
    repo: PathBuf,
}

impl gear_engine::review::ReviewGit for AuditGit {
    fn run(&self, path: &str, args: &[&str]) -> Result<String, String> {
        // Only the project's own checkout is asked for its branch tips; a
        // reviewer's worktree is served whatever it asks (the review fixture).
        if Path::new(path) == self.repo.as_path() && args.first() == Some(&"for-each-ref") {
            let tip = sh(&self.repo, "git rev-parse origin/dev");
            return Ok(format!("origin/dev {tip} 0\n"));
        }
        Ok(String::new())
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
name = "heat"
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
"#,
        repo = repo.display()
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
        estimate_min: Some(30),
        issue: None,
        updated: T0,
    }
}

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
    repo: Repo,
}

/// A lane, a merged task T-1, the audit T-2 that checks it and a task T-3 that
/// depends on the audit.
fn world() -> World {
    let repo = Repo::new();
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            let mut row = tx.get_project("p")?.unwrap();
            row.config_toml = config_toml(&repo.work);
            tx.update_project(&row)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "pi", T0)?;
            tx.set_meta("engine.session_fresh.p:heat", "1")?;
            for (id, rank, state, deps, goal) in [
                ("T-1", 1, TaskState::Done, &[][..], "the fixed thing"),
                (
                    "T-2",
                    2,
                    TaskState::Ready,
                    &["T-1"][..],
                    "the voyage holds end to end",
                ),
                ("T-3", 3, TaskState::Ready, &["T-2"][..], "what comes after"),
            ] {
                tx.insert_task(&task(id, rank, state, deps))?;
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: id.into(),
                    version: 1,
                    goal: goal.into(),
                    accept: "each promise met".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                    mechanical: None,
                })?;
            }
            // T-1 is merged: its attempt carries the merge time the dependency
            // rule reads.
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Accepted,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Merged, T0)?;
            tx.set_task_audit("p", "T-2", true)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named("w1:p1", "codex", AgentStatus::Idle, "p-captain");
    herdr.add_agent_named(LANE_PANE, "pi", AgentStatus::Idle, "p-heat");
    let mut cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        ..Default::default()
    };
    cfg.review_git = std::sync::Arc::new(AuditGit {
        repo: repo.work.clone(),
    });
    World {
        store,
        herdr,
        cfg,
        repo,
    }
}

/// The texts typed into a pane.
fn prompts(w: &mut World, pane: &str) -> Vec<String> {
    w.herdr
        .prompts_to(pane)
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[test]
fn an_audit_is_handed_in_at_the_integration_tip_and_never_worked_by_its_lane() {
    let mut w = world();
    let tip = w.repo.tip();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    // The attempt: Submitted, at the tip of the integration branch, written by
    // the lane's seat.
    let attempt = w
        .store
        .read(|tx| tx.get_attempt("p", "T-2", 1))
        .unwrap()
        .expect("the audit is handed in");
    assert_eq!(attempt.state, AttemptState::Submitted);
    assert_eq!(attempt.candidate.as_deref(), Some(tip.as_str()));
    assert_eq!(attempt.seat, "p:heat");
    // The task is Verifying, which is what the review step looks for.
    let row = w.store.read(|tx| tx.get_task("p", "T-2")).unwrap().unwrap();
    assert_eq!(row.state, TaskState::Verifying);

    // The lane was never assigned it: no delivery for T-2, and T-3 waits for
    // the audit.
    let lane_texts = prompts(&mut w, LANE_PANE);
    assert!(
        !lane_texts.iter().any(|text| text.contains("T-2")),
        "the audit is never delivered to its lane: {lane_texts:?}"
    );
    assert!(
        !lane_texts.iter().any(|text| text.contains("T-3")),
        "T-3 waits for the audit: {lane_texts:?}"
    );
}

#[test]
fn the_audit_packet_asks_for_an_audit_not_a_diff() {
    let mut w = world();
    let tip = w.repo.tip();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    // The review packet is the Deliver op of the reviewer's seat.
    let packet = w
        .store
        .read(|tx| {
            let mut found = None;
            for seat in tx.seats_of_project("p")? {
                if seat.role != SeatRole::Temp {
                    continue;
                }
                for op in tx.ops_of_seat_kind(&seat.id, OpKind::Deliver)? {
                    found = op.payload["text"].as_str().map(str::to_string).or(found);
                }
            }
            Ok(found)
        })
        .unwrap();
    let packet = packet.expect("the reviewer gets its packet");
    assert!(
        packet.contains("Audit T-2: no diff."),
        "the audit text: {packet}"
    );
    assert!(packet.contains(&tip), "the candidate is named: {packet}");
    assert!(packet.contains("met or not"), "{packet}");
    assert!(
        !packet.contains("Diff to review:"),
        "an audit has no diff: {packet}"
    );
}

#[test]
fn a_captain_pass_merges_the_audit_and_starts_what_waits_for_it() {
    let mut w = world();
    let tip = w.repo.tip();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    // The captain rules the audit passed.
    w.store
        .write(|tx| {
            gear_app::tasks::settle(
                tx,
                &gear_app::tasks::Verdict {
                    project: "p".into(),
                    task: "T-2".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: tip.clone(),
                    pass: true,
                    evidence: Some("each promise met".into()),
                    rework: None,
                    integrated_by: None,
                    by_seat: "p:captain".into(),
                },
                T0 + 20,
            )
        })
        .unwrap();
    let row = w.store.read(|tx| tx.get_task("p", "T-2")).unwrap().unwrap();
    assert_eq!(row.state, TaskState::Done);
    assert!(
        w.store
            .read(|tx| gear_app::tasks::merged_at(tx, "p", "T-2"))
            .unwrap()
            .is_some(),
        "the audit is merged at the candidate it checked"
    );
    assert!(
        w.store
            .read(|tx| tx.ops_of_seat_kind("p:heat", OpKind::Merge))
            .unwrap()
            .is_empty(),
        "an audit merges with no merge op"
    );

    // T-3 depended on the audit: the lane gets it on the next tick.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 30).unwrap();
    let lane_texts = prompts(&mut w, LANE_PANE);
    assert!(
        lane_texts
            .iter()
            .any(|text| text.starts_with("New task T-3")),
        "the dependant starts after the audit's pass: {lane_texts:?}"
    );
}

#[test]
fn a_task_without_the_field_still_goes_to_its_lane() {
    let mut w = world();
    w.store
        .write(|tx| tx.set_task_audit("p", "T-2", false))
        .unwrap();
    // T-1 is merged, so T-2's `deps:` are met: a plain task is dispatched.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    let lane_texts = prompts(&mut w, LANE_PANE);
    assert!(
        lane_texts
            .iter()
            .any(|text| text.starts_with("New task T-2")),
        "a task without the field is dispatched as today: {lane_texts:?}"
    );
    assert!(
        w.store
            .read(|tx| tx.get_attempt("p", "T-2", 1))
            .unwrap()
            .is_some_and(|attempt| attempt.seat == "p:heat"),
        "the lane works it"
    );
}

/// The captain rules the audit failed, adds the fix task to its `deps:` in the
/// same plan apply, and the next attempt waits for the fix and then goes to the
/// same reviewer seat (T-480).
#[test]
fn a_failed_audit_waits_for_the_fix_tasks_and_then_repeats_on_its_reviewer() {
    let mut w = world();
    let tip = w.repo.tip();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    let first_reviewer = w
        .store
        .read(|tx| {
            Ok(tx
                .rounds_of_task("p", "T-2")?
                .into_iter()
                .find(|round| round.role == "review" && round.attempt == 1)
                .map(|round| round.seat))
        })
        .unwrap()
        .expect("the audit's first review opened a round");

    // The captain fails the audit and adds the fix task T-4 to its deps in the
    // same plan apply.
    w.store
        .write(|tx| {
            gear_app::tasks::settle(
                tx,
                &gear_app::tasks::Verdict {
                    project: "p".into(),
                    task: "T-2".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: tip.clone(),
                    pass: false,
                    evidence: None,
                    rework: Some("the promise is not met".into()),
                    integrated_by: None,
                    by_seat: "p:captain".into(),
                },
                T0 + 20,
            )
        })
        .unwrap();
    let row = w.store.read(|tx| tx.get_task("p", "T-2")).unwrap().unwrap();
    assert_eq!(row.state, TaskState::Ready, "the audit goes back to Ready");
    assert_eq!(
        w.store
            .read(|tx| tx.get_attempt("p", "T-2", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Rejected
    );

    // The plan line adds the fix task and names it in the audit's deps.
    // The fix task exists (a lane task with its own attempt), and the plan line
    // names it in the audit's deps.
    w.store
        .write(|tx| {
            tx.insert_task(&task("T-4", 4, TaskState::Done, &[]))?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-4".into(),
                version: 1,
                goal: "the fix".into(),
                accept: "each promise met".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-4".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Accepted,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })
        })
        .unwrap();
    let plan = "# Queue\n## T-2 [must] lane=heat\naudit: true\ndeps: T-1, T-4\n";
    let text = format!("<!-- gear-plan project=p base=0 -->\n{plan}");
    let env = gear_app::Env {
        ctx: Some(gear_model::GearCtx {
            project: "p".into(),
            seat: "captain".into(),
            generation: 1,
        }),
        pane_id: Some("w1:p1".into()),
        admin: false,
    };
    let fp = gear_app::identity::Fingerprint {
        pane_id: "w1:p1".into(),
        kind: Some("codex".into()),
        session_ref: None,
    };
    gear_app::plan::apply(
        &mut w.store,
        &env,
        Some(&fp),
        None,
        &gear_app::draft::parse(&text).unwrap(),
        &text,
        "audit-fix",
        T0 + 25,
    )
    .unwrap();
    let deps = w
        .store
        .read(|tx| tx.get_task("p", "T-2"))
        .unwrap()
        .unwrap()
        .deps;
    assert_eq!(deps, ["T-1", "T-4"], "the fix is in the audit's deps");

    // T-4 is not merged: the audit waits.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 30).unwrap();
    assert!(
        w.store
            .read(|tx| tx.get_attempt("p", "T-2", 2))
            .unwrap()
            .is_none(),
        "the audit waits for its fix"
    );

    // The fix merges: attempt 2 is handed in, and its review goes to the seat
    // that reviewed attempt 1.
    w.store
        .write(|tx| tx.set_attempt_time("p", "T-4", 1, AttemptStep::Merged, T0 + 40))
        .unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 50).unwrap();
    let second = w
        .store
        .read(|tx| tx.get_attempt("p", "T-2", 2))
        .unwrap()
        .expect("attempt 2 is handed in once the fix is merged");
    assert_eq!(second.state, AttemptState::Submitted);
    assert_eq!(second.candidate.as_deref(), Some(tip.as_str()));

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 60).unwrap();
    let second_reviewer = w
        .store
        .read(|tx| {
            Ok(tx
                .rounds_of_task("p", "T-2")?
                .into_iter()
                .find(|round| round.role == "review" && round.attempt == 2)
                .map(|round| round.seat))
        })
        .unwrap();
    assert_eq!(
        second_reviewer.as_deref(),
        Some(first_reviewer.as_str()),
        "the re-audit goes to the reviewer of the attempt before it"
    );
}
