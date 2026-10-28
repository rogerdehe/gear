//! Contract for the project's cap on a task's planned time (T-279): a lane
//! works unattended and is reviewed only at its hand-in, so a task longer than
//! `[conventions].max_task_min` (default 60) is refused and the captain cuts
//! it into tasks each with its own test cycle. A rework estimate (`estimate=`
//! on a `# Verdicts` fail) is the price of finishing a task that is already
//! open, so it is not capped.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, SeatRole};
use gear_store::{testing, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

/// A config of two lanes, with the project's cap on a task's planned time.
fn config(max_task_min: u32) -> String {
    format!(
        r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 2
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

[[lanes]]
name = "wire"
duty = "price engine"
owns = ["src/wire/"]
kind = "opencode"
profile = "opencode"

[conventions]
gate = "make test"
max_task_min = {max_task_min}
"#
    )
}

/// The world without a stored config: the cap is the default 60.
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", NOW)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", NOW)?;
        Ok(())
    })
    .unwrap();
    s
}

/// The same world with a stored project config, for the project's own cap.
fn world_with_cap(max_task_min: u32) -> Store {
    let mut s = world();
    s.write(|tx| {
        let mut project = tx.get_project("p")?.unwrap();
        project.config_toml = config(max_task_min);
        tx.update_project(&project)
    })
    .unwrap();
    s
}

fn cap() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: CAP_PANE.into(), kind: Some("codex".into()), session_ref: None },
    )
}

fn run(s: &mut Store, text: &str, rid: &str) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    run_at(s, text, rid, NOW)
}

fn run_at(
    s: &mut Store,
    text: &str,
    rid: &str,
    now: i64,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), None, &parse(text).unwrap(), text, rid, now)
}

fn new_block(estimate: u32) -> String {
    format!("<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: {estimate}\ndeps: -\n")
}

/// The exact reason a plan over the cap is refused.
fn over_cap(estimate: u32, cap: u32) -> String {
    format!(
        "estimate {estimate} min is over the project's cap of {cap} min: cut it into tasks of at most {cap} min, each with its own test cycle ([conventions] max_task_min)"
    )
}

#[test]
fn a_new_block_over_the_default_cap_is_refused_and_nothing_is_written() {
    let mut s = world();
    let err = run(&mut s, &new_block(61), "r1").unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat, "{}", err.detail);
    assert!(err.detail.ends_with(&format!("NEW: {}", over_cap(61, 60))), "{}", err.detail);
    // Nothing was written: no task, no plan version.
    assert!(s.read(|tx| tx.tasks_of_project("p")).unwrap().is_empty());
    assert_eq!(s.read(|tx| tx.plan_version("p")).unwrap(), 0);
}

#[test]
fn an_estimate_of_exactly_the_cap_applies() {
    let mut s = world();
    let r = run(&mut s, &new_block(60), "r1").unwrap();
    assert_eq!(r.created, vec!["T-1".to_string()]);
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.estimate_min, Some(60));
}

#[test]
fn a_project_may_plan_longer_tasks_with_its_own_max_task_min() {
    // With `max_task_min = 90`, an estimate of 90 applies and 91 is refused.
    let mut s = world_with_cap(90);
    let r = run(&mut s, &new_block(90), "r1").unwrap();
    assert_eq!(r.created, vec!["T-1".to_string()]);
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.estimate_min, Some(90));

    let err = run(
        &mut s,
        &new_block(91).replace("base=0", "base=1"),
        "r2",
    )
    .unwrap_err();
    assert!(err.detail.ends_with(&format!("NEW: {}", over_cap(91, 90))), "{}", err.detail);
    assert_eq!(
        s.read(|tx| tx.tasks_of_project("p")).unwrap().len(),
        1,
        "the refused draft wrote nothing"
    );
}

#[test]
fn raising_an_existing_task_over_the_cap_is_refused() {
    let mut s = world();
    run(&mut s, &new_block(30), "r1").unwrap();
    let err = run(
        &mut s,
        "<!-- gear-plan project=p base=1 -->\n# Queue\n## T-1 [must] lane=heat\nestimate: 61\n",
        "r2",
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat, "{}", err.detail);
    assert!(err.detail.ends_with(&format!("T-1: {}", over_cap(61, 60))), "{}", err.detail);
    // Nothing was written: the task keeps the estimate it had.
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.estimate_min, Some(30));
    assert_eq!(s.read(|tx| tx.plan_version("p")).unwrap(), 1);

    // The cap itself is allowed on an existing task too.
    run(
        &mut s,
        "<!-- gear-plan project=p base=1 -->\n# Queue\n## T-1 [must] lane=heat\nestimate: 60\n",
        "r3",
    )
    .unwrap();
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.estimate_min, Some(60));
}

/// A task handed in and failed its review: the captain's rework item.
fn world_with_a_failed_review() -> Store {
    let mut s = world();
    run(&mut s, &new_block(30), "r1").unwrap();
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, NOW)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, NOW)?;
        gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, "abc1234", "ops/reports/T-1.md", NOW)?;
        tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            passed: false,
            findings: "F1: the edge case is not handled".into(),
            by_seat: "p:review-1".into(),
            created: NOW,
        })
    })
    .unwrap();
    s
}

#[test]
fn a_rework_estimate_over_the_cap_applies() {
    // The rework estimate is the price of finishing a task that is already
    // open, not a new task: it is not capped.
    let mut s = world_with_a_failed_review();
    run_at(
        &mut s,
        "<!-- gear-plan project=p base=1 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc1234 fail estimate=120 rework=\"handle the edge case\"\n",
        "r2",
        NOW + 60,
    )
    .unwrap();
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.rework_estimate_min, Some(120));
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap().state,
        AttemptState::Rejected
    );
}
