//! Contract for the time report (T-38): the captain reads where the time
//! went — per task and per voyage.

use gear_app::draft;
use gear_app::identity::Fingerprint;
use gear_app::plan;
use gear_app::review::{self, Outcome, Review};
use gear_app::tasks::{on_delivery_confirmed, settle_review, start_attempt, submit_result};
use gear_app::time::{self, Filter};
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const REVIEW_PANE: &str = "w1:p3";
const CANDIDATE: &str = "abc1234";

fn captain() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: CAP_PANE.into(), kind: Some("codex".into()), session_ref: None },
    )
}

/// The world: a captain, the lane `heat` and a reviewer bound to T-1.
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
        testing::seed_temp_seat(tx, "p", "r-1", "T-1", REVIEW_PANE, "codex", T0)?;
        Ok(())
    })
    .unwrap();
    s
}

/// Apply a plan as the captain.
fn apply(s: &mut Store, text: &str, rid: &str) -> gear_model::GearResult<plan::ApplyResult> {
    let draft = draft::parse(text).unwrap();
    let (env, fp) = captain();
    plan::apply(s, &env, Some(&fp), &draft, text, rid, T0)
}

/// The plan: one new task of the voyage M2, planned for 60 minutes.
const NEW_TASK: &str =
    "## NEW [must] lane=heat milestone=M2\ngoal: A\naccept: a\nestimate: 60\ndeps: -\n";

/// The report on a seeded store: the durations and the ratio.
#[test]
fn the_report_prints_the_durations_and_the_ratio() {
    let mut s = world();
    apply(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n# Queue\n{NEW_TASK}"),
        "r1",
    )
    .unwrap();
    s.write(|tx| start_attempt(tx, "p", "T-1", "p:heat", 1, T0)).unwrap();
    s.write(|tx| on_delivery_confirmed(tx, "p", "T-1", 1, T0 + 5 * 60)).unwrap();
    s.write(|tx| {
        submit_result(tx, "p", "T-1", 1, 1, CANDIDATE, "Report: notes/T-1.md", T0 + 15 * 60)
    })
    .unwrap();
    s.write(|tx| {
        review::submit_review(
            tx,
            &Review {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                outcome: Outcome::Pass,
                findings: "F1: looks good".into(),
                blocking: String::new(),
                by_seat: "p:r-1".into(),
            },
            T0 + 20 * 60,
        )
    })
    .unwrap();
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 25 * 60)).unwrap();

    let report = s
        .read(|tx| time::report(tx, "p", &Filter::default()))
        .unwrap();
    let task = &report.tasks[0];
    assert_eq!(task.estimate_min, Some(60));
    assert_eq!(task.dev(), Some(10), "start → hand-in");
    assert_eq!(task.review(), Some(5), "hand-in → verdict");
    assert_eq!(task.wait(), Some(5), "verdict → settle");
    assert_eq!(task.merge(), None, "nothing is merged yet");
    assert_eq!(task.actual_min, Some(20), "the start of the work → the settle");
    assert_eq!(task.ratio(), Some(20.0 / 60.0));

    let text = time::render(&report);
    assert!(text.contains("T-1 · estimate 60m"), "{text}");
    assert!(text.contains("dev 10m · review 5m · wait 5m"), "{text}");
    assert!(text.contains("actual 20m"), "{text}");
    assert!(text.contains("ratio 0.33"), "{text}");
    // The voyage of the task is reported too.
    assert!(text.contains("Voyage M2 · 1 task · estimate 60m"), "{text}");
    assert!(text.contains("ratio 0.33"), "{text}");
}

/// The report is the captain's: a lane may not read it.
#[test]
fn only_the_captain_may_read_the_report() {
    let mut s = world();
    apply(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n# Queue\n{NEW_TASK}"),
        "r1",
    )
    .unwrap();
    let lane = (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 1 }),
            pane_id: Some(LANE_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: LANE_PANE.into(), kind: Some("opencode".into()), session_ref: None },
    );
    let err = time::build(&mut s, &lane.0, Some(&lane.1), &Filter::default()).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.detail);
}
