//! T-151: the rounds bookkeeping (design §4.1) and the session of a check-in
//! (design §3.2 step 5).
//!
//! A round owns every agent record on its seat from its `start_at` until its
//! `end_at`: the start of the next round on the same seat, or the seat's
//! release. The tick of `rounds.rs` keeps that true: the session the seat
//! carries now joins its open round (once), and a round ends when the next
//! round starts or the seat is released.

use gear_engine::ops::{confirm_pending, intend_deliveries};
use gear_engine::dispatch::DeliverAction;
use gear_herdr::fake::FakeHerdr;
use gear_model::{AttemptState, OpKind, OpState, SeatRole, SeatState, TaskState};
use gear_store::{
    testing, AttemptRow, NewRound, RoundKey, Store, TaskRow, ViewRecord,
};

const T0: i64 = 1_800_000_000;
const LANE: &str = "p:heat";

/// A project with one lane whose agent runs in a pane, and a round that is
/// open on it.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            // The agent runs in a session of its own.
            tx.set_binding_session_ref(LANE, 1, "ses_A")?;
            Ok(())
        })
        .unwrap();
    store
}

/// A round on the lane, open since `start_at`.
fn round(store: &mut Store, task: &str, attempt: i64, role: &str, start_at: i64) {
    store
        .write(|tx| {
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: task.into(),
                    attempt,
                    role: role.into(),
                },
                seat: LANE.into(),
                generation: 1,
                cli: "opencode".into(),
                model: None,
                start_at,
            })
        })
        .unwrap();
}

/// The round of a task, as the store keeps it.
fn round_of(store: &mut Store, task: &str, attempt: i64, role: &str) -> gear_store::RoundRow {
    store
        .read(|tx| tx.rounds_of_task("p", task))
        .unwrap()
        .into_iter()
        .find(|row| row.attempt == attempt && row.role == role)
        .expect("the round")
}

/// T-151: a session that changes inside a round joins the round's sessions —
/// once, however often the tick runs.
#[test]
fn a_session_change_mid_round_lands_in_the_rounds_sessions_once() {
    let mut store = world();
    round(&mut store, "T-1", 1, "dev", T0);

    let mut herdr = FakeHerdr::new();
    // The agent is restarted inside the round: the binding carries a new
    // session.
    store
        .write(|tx| tx.set_binding_session_ref(LANE, 1, "ses_B"))
        .unwrap();

    let report = gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 10)
        .unwrap();
    assert_eq!(report.sessions, vec!["T-1/1/dev"], "{report:?}");

    let row = round_of(&mut store, "T-1", 1, "dev");
    assert_eq!(row.sessions, vec!["ses_B"], "the new session owns the records");

    // The next tick says it again — the list holds it once.
    let report = gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 20)
        .unwrap();
    assert!(report.sessions.is_empty(), "nothing new: {report:?}");
    let row = round_of(&mut store, "T-1", 1, "dev");
    assert_eq!(row.sessions, vec!["ses_B"]);
}

/// T-151: a second round on the seat closes the first at its start.
#[test]
fn a_second_round_on_the_seat_closes_the_first_at_its_start() {
    let mut store = world();
    round(&mut store, "T-1", 1, "dev", T0);
    round(&mut store, "T-2", 1, "dev", T0 + 100);

    let mut herdr = FakeHerdr::new();
    let report = gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 110)
        .unwrap();
    assert_eq!(report.closed, vec!["T-1/1/dev"], "{report:?}");

    let first = round_of(&mut store, "T-1", 1, "dev");
    assert_eq!(first.end_at, Some(T0 + 100), "the round ends at the next one's start");
    let second = round_of(&mut store, "T-2", 1, "dev");
    assert_eq!(second.end_at, None, "the newest round stays open");
}

/// T-151: a release closes the open round.
#[test]
fn a_release_closes_the_open_round() {
    let mut store = world();
    round(&mut store, "T-1", 1, "review", T0);
    // The reviewer is released.
    store
        .write(|tx| tx.set_seat_state(LANE, SeatState::Released, Some("released"), T0 + 50))
        .unwrap();

    let mut herdr = FakeHerdr::new();
    let report = gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 60)
        .unwrap();
    assert_eq!(report.closed, vec!["T-1/1/review"], "{report:?}");

    let row = round_of(&mut store, "T-1", 1, "review");
    assert_eq!(row.end_at, Some(T0 + 50), "the round ends when the seat is released");
}

/// T-151 (§3.2 step 5): the check-in sets the attempt's session and its CLI.
#[test]
fn the_check_in_sets_the_attempts_session() {
    let mut store = world();
    // A delivery for the lane, on its way to the pane.
    store
        .write(|tx| {
            tx.insert_task(&TaskRow {
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
                issue: None,
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                updated: T0,
            })?;
            tx.insert_attempt(&AttemptRow {
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: LANE.into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Delivered,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })?;
            let ops = intend_deliveries(
                tx,
                &[DeliverAction {
                    project: "p".into(),
                    seat: LANE.into(),
                    generation: 1,
                    task: "T-1".into(),
                    attempt: 1,
                    text: "New task T-1".into(),
                }],
                T0 + 10,
            )?;
            tx.set_op_state("p", &ops[0].id, OpState::Executing, None, T0 + 10)?;
            tx.set_op_state("p", &ops[0].id, OpState::Submitted, None, T0 + 11)?;
            Ok(())
        })
        .unwrap();

    // The lane checks in: the view shows the task and the attempt.
    store
        .write(|tx| {
            tx.insert_view(&ViewRecord {
                project: "p".into(),
                seat: LANE.into(),
                generation: 1,
                ts: T0 + 20,
                shown_msgs: vec![],
                shown_ops: vec![],
                shown_task: Some("T-1".into()),
                shown_attempt: Some(1),
                shown_contract: Some(1),
            })
        })
        .unwrap();

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 30).unwrap();
    assert_eq!(confirmed.len(), 1, "{confirmed:?}");

    let attempt = store.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Started);
    assert_eq!(attempt.session_ref.as_deref(), Some("ses_A"), "the session of the check-in");
    assert_eq!(attempt.session_cli.as_deref(), Some("opencode"), "the CLI of the seat");
}

/// T-151 (the review of attempt 1, F2): a round that is closed by the
/// release of its seat keeps the session of its last work: the tick closes the
/// superseded rounds, then gives the open round the session the seat carries
/// now, and closes the released round last.
#[test]
fn a_released_round_keeps_the_session_of_its_last_work() {
    let mut store = world();
    round(&mut store, "T-1", 1, "review", T0);
    let mut herdr = FakeHerdr::new();

    // The first tick: the round owns the session the seat carries now.
    gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 10).unwrap();
    assert_eq!(round_of(&mut store, "T-1", 1, "review").sessions, vec!["ses_A"]);

    // The agent is restarted (a new session) and the seat is released in the
    // same tick.
    store
        .write(|tx| tx.set_binding_session_ref(LANE, 1, "ses_B"))
        .unwrap();
    store
        .write(|tx| tx.set_seat_state(LANE, SeatState::Released, Some("released"), T0 + 50))
        .unwrap();

    let report = gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 60)
        .unwrap();
    assert_eq!(report.closed, vec!["T-1/1/review"], "{report:?}");

    let row = round_of(&mut store, "T-1", 1, "review");
    assert_eq!(row.sessions, vec!["ses_A", "ses_B"], "the last work is in the round");
    assert_eq!(row.end_at, Some(T0 + 50), "the release time is kept");
}

/// T-151 (the review of attempt 1, F1): a review delivery names the
/// **writer's** attempt, while its binding belongs to the reviewer. The
/// writer's session is the writer's: the review says nothing about it.
#[test]
fn a_review_confirmation_leaves_the_writers_session_alone() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            testing::seed_temp_seat(tx, "p", "review-1", "T-1", "w1:p3", "codex", T0)?;
            // The writer works in its own session.
            tx.set_binding_session_ref("p:heat", 1, "ses_A")?;
            tx.set_binding_session_ref("p:review-1", 1, "ses_REVIEWER")?;
            // The task the writer works on (its attempt waits for its
            // check-in, so the task is still Ready).
            tx.insert_task(&TaskRow {
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
                issue: None,
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                updated: T0,
            })?;
            // The writer's attempt, already in its session.
            tx.insert_attempt(&AttemptRow {
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Delivered,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
                session_ref: Some("ses_A".into()),
                session_cli: Some("opencode".into()),
                session_action: None,
                sent_at: None,
            })?;
            // The reviewer's delivery: it names the writer's attempt.
            let op = tx.insert_op(&gear_store::NewOp {
                project: "p".into(),
                seat: "p:review-1".into(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({"text": "Review T-1 attempt 1"}),
                created: T0 + 10,
            })?;
            tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 10)?;
            tx.set_op_state("p", &op.id, OpState::Submitted, None, T0 + 11)?;
            // The reviewer hands in its verdict: the delivery is confirmed.
            tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                passed: true,
                findings: "ops/reviews/T-1.md".into(),
                by_seat: "p:review-1".into(),
                created: T0 + 20,
            })?;
            Ok(())
        })
        .unwrap();

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 30).unwrap();
    assert_eq!(confirmed.len(), 1, "the review delivery is confirmed: {confirmed:?}");

    let attempt = store.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(
        attempt.session_ref.as_deref(),
        Some("ses_A"),
        "the writer keeps its session"
    );
    assert_eq!(
        attempt.session_cli.as_deref(),
        Some("opencode"),
        "the writer keeps its CLI"
    );
}
