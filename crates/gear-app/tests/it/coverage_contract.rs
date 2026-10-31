//! T-474: `gear report coverage` — the accepted attempts whose candidate is
//! not on the integration branch (accepted and never landed).

use gear_app::coverage;
use gear_app::identity::Fingerprint;
use gear_app::tasks::start_attempt;
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

const CONFIG: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
"#;

fn env(seat: &str, pane: &str) -> Env {
    Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: seat.into(),
            generation: 1,
        }),
        pane_id: Some(pane.into()),
        admin: false,
    }
}

fn fp(pane: &str, kind: &str) -> Fingerprint {
    Fingerprint {
        pane_id: pane.to_string(),
        kind: Some(kind.to_string()),
        session_ref: None,
    }
}

fn task(tx: &gear_store::Tx<'_>, id: &str, rank: i64) -> gear_model::GearResult<()> {
    tx.insert_task(&TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank,
        state: TaskState::Ready,
        priority: "must".into(),
        lane: Some("heat".into()),
        milestone: Some("M6".into()),
        required: true,
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
        task: id.into(),
        version: 1,
        goal: "g".into(),
        accept: "a".into(),
        evidence_files: vec![],
        evidence_due_min: None,
        body: String::new(),
        created: T0,
        mechanical: None,
    })
}

fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            let mut project = tx.get_project("p")?.unwrap();
            project.config_toml = CONFIG.into();
            tx.update_project(&project)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
            for (id, rank) in [("T-1", 1), ("T-2", 2), ("T-3", 3)] {
                task(tx, id, rank)?;
                let attempt = start_attempt(tx, "p", id, "p:heat", 1, T0)?;
                let mut task_row = tx.get_task("p", id)?.unwrap();
                task_row.state = TaskState::Done;
                tx.update_task(&task_row)?;
                let mut attempt = attempt;
                attempt.state = AttemptState::Accepted;
                // T-1's candidate landed; T-2's did not; T-3 was accepted
                // without a candidate (a mechanical task) and cannot be off.
                attempt.candidate = match id {
                    "T-1" => Some("aaaa1111".into()),
                    "T-2" => Some("bbbb2222".into()),
                    _ => None,
                };
                tx.update_attempt(&attempt)?;
            }
            Ok(())
        })
        .unwrap();
    store
}

#[test]
fn coverage_lists_the_accepted_attempts_whose_candidate_is_not_on_the_integration_branch() {
    let mut store = world();
    let report = coverage::build(
        &mut store,
        &env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        &|_repo, _remote, sha| sha == "aaaa1111",
    )
    .unwrap();
    assert_eq!(report.project, "p");
    assert_eq!(report.integration, "dev");
    assert_eq!(report.checked, 2, "the accepted attempts with a candidate");
    assert_eq!(report.off.len(), 1, "{report:?}");
    let off = &report.off[0];
    assert_eq!(off.task, "T-2");
    assert_eq!(off.attempt, 1);
    assert_eq!(off.candidate, "bbbb2222");
    assert_eq!(off.lane.as_deref(), Some("heat"));
    let text = coverage::render(&report);
    assert!(text.contains("Coverage report · project p"), "{text}");
    assert!(text.contains("1 not on it"), "{text}");
    assert!(text.contains("T-2 attempt 1: candidate bbbb2222"), "{text}");
    assert!(
        !text.contains("T-1 attempt"),
        "the landed one is not listed: {text}"
    );
    assert!(
        !text.contains("T-3 attempt"),
        "no candidate, nothing to check: {text}"
    );
}

#[test]
fn coverage_says_when_every_accepted_candidate_is_on_the_integration_branch() {
    let mut store = world();
    let report = coverage::build(
        &mut store,
        &env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        &|_repo, _remote, _sha| true,
    )
    .unwrap();
    assert_eq!(report.checked, 2);
    assert!(report.off.is_empty());
    let text = coverage::render(&report);
    assert!(text.contains("every candidate is on it"), "{text}");
}

/// A candidate the check cannot read is reported as off: a silent yes would
/// hide the lost merge the report exists for.
#[test]
fn a_candidate_the_check_cannot_read_is_listed() {
    let mut store = world();
    let report = coverage::build(
        &mut store,
        &env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        &|_repo, _remote, sha| sha == "aaaa1111",
    )
    .unwrap();
    assert_eq!(report.off.len(), 1);
    assert_eq!(report.off[0].candidate, "bbbb2222");
}

/// The check is handed the project's repo and remote, so the CLI can fetch the
/// integration branch it asks about.
#[test]
fn the_check_gets_the_repo_and_the_remote() {
    let mut store = world();
    let seen = std::cell::RefCell::new(Vec::new());
    coverage::build(
        &mut store,
        &env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        &|repo, remote, sha| {
            seen.borrow_mut()
                .push((repo.path.clone(), remote.to_string(), sha.to_string()));
            true
        },
    )
    .unwrap();
    let seen = seen.into_inner();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|(_, remote, _)| remote == "origin"));
    assert!(seen.iter().any(|(_, _, sha)| sha == "aaaa1111"));
    assert!(seen.iter().any(|(_, _, sha)| sha == "bbbb2222"));
}

#[test]
fn only_the_captain_reads_the_coverage_report() {
    let mut store = world();
    let err = coverage::build(
        &mut store,
        &env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        &|_repo, _remote, _sha| true,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
}
