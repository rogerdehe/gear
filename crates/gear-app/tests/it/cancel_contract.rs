//! T-93: cancelling a task from the plan finishes it.
//!
//! M3: T-80 was cancelled from the plan while running; its attempt stayed
//! `started`, the seat looked busy and the lane sat idle with six tasks
//! queued until the captain edited the store. Now the cancel abandons the
//! open attempt, tells the lane one line, and a plan that keeps saying
//! `state: cancelled` applies cleanly.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::tasks::{on_delivery_confirmed, start_attempt, submit_result};
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";

fn cap() -> (Env, Fingerprint) {
    (
        Env { ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }), pane_id: Some(CAP.into()), admin: false },
        Fingerprint { pane_id: CAP.into(), kind: Some("codex".into()), session_ref: None },
    )
}

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        Ok(())
    })
    .unwrap();
    run(&mut s, "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [must] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n", 0, "v1").unwrap();
    s
}

fn run(s: &mut Store, queue: &str, base: i64, rid: &str) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let text = format!("<!-- gear-plan project=p base={base} -->\n# Queue\n{queue}");
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), &parse(&text).unwrap(), &text, rid, T0 + 10)
}

fn attempt(s: &mut Store, task: &str, n: i64) -> AttemptState {
    s.read(|tx| tx.get_attempt("p", task, n)).unwrap().unwrap().state
}

fn state(s: &mut Store, task: &str) -> TaskState {
    s.read(|tx| tx.get_task("p", task)).unwrap().unwrap().state
}

fn lane_messages(s: &mut Store, task: &str) -> Vec<gear_store::MessageRow> {
    s.read(|tx| tx.thread_messages("p", &format!("{task}:cancel"))).unwrap()
}

const CANCEL_T1: &str = "## T-1 [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nstate: cancelled\n\n## T-2 [must] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n";

/// The task the lane is working on is cancelled: its attempt is abandoned —
/// the seat is free — and the lane gets one line telling it to stop.
#[test]
fn cancelling_a_started_task_abandons_its_attempt_and_tells_the_lane() {
    let mut s = world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)
    })
    .unwrap();
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Started);

    run(&mut s, CANCEL_T1, 1, "v2").unwrap();

    assert_eq!(state(&mut s, "T-1"), TaskState::Cancelled);
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Abandoned, "the seat is free");
    assert_eq!(
        s.read(|tx| tx.in_progress_attempts_of_seat("p:heat")).unwrap().len(),
        0,
        "no attempt holds the lane"
    );
    let messages = lane_messages(&mut s, "T-1");
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].recipient, "p:heat");
    assert_eq!(messages[0].kind, MsgKind::Instruction);
    assert_eq!(messages[0].state, MsgState::Open);
    assert_eq!(
        messages[0].body,
        "T-1 is cancelled; stop work on it, nothing to hand in"
    );
}

/// The plan keeps saying `state: cancelled` (it is the plan of record):
/// re-applying it is a no-op, not an error, and adds no second line.
#[test]
fn re_applying_the_same_cancel_is_a_no_op() {
    let mut s = world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)
    })
    .unwrap();
    run(&mut s, CANCEL_T1, 1, "v2").unwrap();
    let first = lane_messages(&mut s, "T-1");
    assert_eq!(first.len(), 1);

    let again = run(&mut s, CANCEL_T1, 2, "v3").unwrap();
    assert_eq!(again.version, 3);
    assert_eq!(again.settled.len(), 0);
    assert_eq!(state(&mut s, "T-1"), TaskState::Cancelled);
    assert_eq!(lane_messages(&mut s, "T-1").len(), 1, "one line, once");
}

/// Every open state closes: a handed-in attempt and a delivered one too.
#[test]
fn a_handed_in_or_delivered_attempt_is_abandoned_as_well() {
    let mut s = world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
        submit_result(tx, "p", "T-1", 1, 1, "abc1234", "ops/reports/T-1.md", T0)
    })
    .unwrap();
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Submitted);

    run(&mut s, CANCEL_T1, 1, "v2").unwrap();

    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Abandoned);
    assert_eq!(lane_messages(&mut s, "T-1").len(), 1);
}

/// An attempt whose outcome the engine could not read is closed too: it is
/// stopped first (the model never abandons UNKNOWN directly, so the engine
/// does not guess) and then abandoned, so the lane is free.
#[test]
fn an_unknown_attempt_is_closed_too() {
    let mut s = world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
        let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
        attempt.state = AttemptState::Unknown;
        tx.update_attempt(&attempt)
    })
    .unwrap();
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Unknown);

    run(&mut s, CANCEL_T1, 1, "v2").unwrap();

    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Abandoned);
    assert_eq!(lane_messages(&mut s, "T-1").len(), 1);
}

/// A task that is done is not cancelled after the fact; an idle task needs no
/// line (nothing was working on it).
#[test]
fn a_finished_task_cannot_be_cancelled_and_an_idle_one_gets_no_line() {
    let mut s = world();
    let text = "<!-- gear-plan project=p base=1 -->\n# Queue\n## T-2 [must] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: -\nstate: cancelled\n";
    let (env, fp) = cap();
    apply(&mut s, &env, Some(&fp), &parse(text).unwrap(), text, "v2", T0 + 10).unwrap();
    assert_eq!(state(&mut s, "T-2"), TaskState::Cancelled);
    assert!(lane_messages(&mut s, "T-2").is_empty(), "nothing was running");

    // A done task is refused, naming it.
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-1")?.unwrap();
        task.state = TaskState::Done;
        tx.update_task(&task)
    })
    .unwrap();
    let bad = "<!-- gear-plan project=p base=2 -->\n# Queue\n## T-1 [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nstate: cancelled\n";
    let error = apply(&mut s, &env, Some(&fp), &parse(bad).unwrap(), bad, "v3", T0 + 20).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(error.detail.contains("T-1"), "{}", error.detail);
}
