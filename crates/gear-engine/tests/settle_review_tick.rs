//! Contract for the tick's review settlement (T-13): a review verdict that
//! arrived settles its task in the engine tick — no captain action is needed.

use gear_app::tasks::{on_delivery_confirmed, start_attempt, submit_result};
use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_model::{MsgKind, MsgState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, NewMessage, ReviewVerdictRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;

/// Project `p` with a captain, the lane `heat`, T-1 handed in and its review
/// (a pass) waiting in the store and in the captain's inbox.
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
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
            updated: T0,
        })?;
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-1".into(),
            version: 1,
            goal: "g".into(),
            accept: "a".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T0,
        })?;
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
        submit_result(tx, "p", "T-1", 1, 1, "abc1234", "Report: r", T0 + 1)?;
        tx.insert_review_verdict(&ReviewVerdictRow {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            passed: true,
            findings: "ops/reviews/T-1.md".into(),
            by_seat: "p:r-1".into(),
            created: T0 + 2,
        })?;
        tx.insert_message(&NewMessage {
            project: "p".into(),
            thread: "T-1:1".into(),
            task: Some("T-1".into()),
            attempt: Some(1),
            sender: "p:r-1".into(),
            sender_gen: 1,
            recipient: "p:captain".into(),
            kind: MsgKind::Review,
            urgent: false,
            body: "review pass on T-1 attempt 1\nops/reviews/T-1.md".into(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: T0 + 2,
        })?;
        Ok(())
    })
    .unwrap();
    s
}

#[test]
fn a_review_verdict_settles_the_task_in_the_tick() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();

    let report = tick(&mut s, &mut herdr, &EngineCfg::default(), T0 + 10).unwrap();
    assert_eq!(report.settled, vec!["p:T-1/1".to_string()], "{report:?}");

    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state,
        TaskState::Done
    );
    // The merge waits for the engine's merge op executor (T-12).
    let ops = s.read(|tx| tx.ops_in_state("p", OpState::Intended)).unwrap();
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!((ops[0].kind, ops[0].task.as_deref()), (OpKind::Merge, Some("T-1")));

    // The captain was not asked: the review item is closed, one line tells it.
    assert!(
        s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 20)).unwrap().is_empty(),
        "no action item is left for the captain"
    );
    let info = s.read(|tx| tx.messages_to_since("p", "p:captain", 0)).unwrap();
    assert_eq!(info.len(), 1, "{info:?}");

    // A second tick settles nothing again.
    let report = tick(&mut s, &mut herdr, &EngineCfg::default(), T0 + 30).unwrap();
    assert!(report.settled.is_empty(), "{report:?}");
    assert_eq!(
        s.read(|tx| tx.ops_in_state("p", OpState::Intended)).unwrap().len(),
        1,
        "no second merge op"
    );
}
