//! Contract for the time accounting (T-38): every step of an attempt writes
//! its own time, and a plan carries the estimate of a task.

use gear_app::draft;
use gear_app::identity::Fingerprint;
use gear_app::plan;
use gear_app::review::{self, Outcome, Review};
use gear_app::tasks::{on_delivery_confirmed, settle_review, start_attempt, submit_result};
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
    plan::apply(s, &env, Some(&fp), None, &draft, text, rid, T0)
}

/// The plan: one new task of the voyage M2, planned for 60 minutes.
fn plan_text(base: i64, queue: &str) -> String {
    // The voyage M2 the task sails on carries its block (T-333); a queue that
    // names no milestone needs none.
    let block = if queue.contains("milestone=") {
        "# Milestone M2: delivery\ndone: it ships\nbar: it holds\neta: 8h\n\n"
    } else {
        ""
    };
    format!("<!-- gear-plan project=p base={base} -->\n{block}# Queue\n{queue}")
}

const NEW_TASK: &str =
    "## NEW [must] lane=heat milestone=M2\ngoal: A\naccept: a\nestimate: 60\ndeps: -\n";

#[test]
fn a_plan_with_an_estimate_parses_and_a_new_task_without_one_is_refused() {
    // The field is known: a plan with an estimate parses and keeps the value.
    let parsed = draft::parse(&plan_text(
        0,
        "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 90\ndeps: -\n",
    ))
    .unwrap();
    assert_eq!(parsed.queue[0].estimate, Some(90));

    // A NEW task without an estimate is refused: the time of a new task is
    // planned, not guessed later.
    let mut s = world();
    let err = apply(
        &mut s,
        &plan_text(0, "## NEW [must] lane=heat\ngoal: A\naccept: a\ndeps: -\n"),
        "r1",
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat, "{}", err.detail);
    assert!(err.detail.contains("estimate"), "{}", err.detail);
    // Nothing was written.
    assert!(s.read(|tx| tx.tasks_of_project("p")).unwrap().is_empty());

    // With an estimate the task is created and carries it.
    let r = apply(&mut s, &plan_text(0, NEW_TASK), "r2").unwrap();
    assert_eq!(r.created, vec!["T-1".to_string()]);
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.estimate_min, Some(60));

    // An existing task may keep the estimate it has: a draft without one does
    // not clear it.
    apply(
        &mut s,
        &plan_text(1, "## T-1 [must] lane=heat\ngoal: A\naccept: a\ndeps: -\n"),
        "r3",
    )
    .unwrap();
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.estimate_min, Some(60), "the estimate is kept");
}

/// The whole loop: every step writes its own time, and no step moves another.
#[test]
fn every_step_writes_its_own_time() {
    let mut s = world();
    apply(&mut s, &plan_text(0, NEW_TASK), "r1").unwrap();

    // 1. the lane takes the task.
    s.write(|tx| start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + 60))
        .unwrap();
    // 2. the delivery is confirmed: the work begins.
    s.write(|tx| on_delivery_confirmed(tx, "p", "T-1", 1, T0 + 120))
        .unwrap();
    // 3. the lane hands the candidate in.
    s.write(|tx| {
        submit_result(
            tx,
            "p",
            "T-1",
            1,
            1,
            CANDIDATE,
            "Report: notes/T-1.md",
            T0 + 20 * 60,
        )
    })
    .unwrap();
    // 4. the reviewer hands the verdict in.
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
            T0 + 25 * 60,
        )
    })
    .unwrap();
    // 5. the attempt is settled.
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 30 * 60))
        .unwrap();

    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.delivered, Some(T0 + 60));
    assert_eq!(times.started, Some(T0 + 120));
    assert_eq!(times.handed_in, Some(T0 + 20 * 60));
    assert_eq!(times.verdict, Some(T0 + 25 * 60));
    assert_eq!(times.settled, Some(T0 + 30 * 60));
    assert_eq!(times.merged, None, "nothing is merged yet");

    // A later step does not move an earlier one: the row is replaced by the
    // settlement, and the times stay where they were.
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 40 * 60))
        .unwrap();
    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.started, Some(T0 + 120));
    assert_eq!(times.handed_in, Some(T0 + 20 * 60));
    assert_eq!(
        times.settled,
        Some(T0 + 30 * 60),
        "the first settle time stands"
    );
}
