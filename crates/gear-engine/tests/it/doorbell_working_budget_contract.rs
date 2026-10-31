//! T-213: a doorbell queued behind a working agent is not a stall.
//!
//! The push reminder and the overrun question rang a lane in the middle of a
//! 4-7 min build or suite; pi queued the doorbell behind the turn, the check
//! budget ran out and the engine opened "lane is stuck". A doorbell that
//! reached a working agent spends no check budget: the budget counts only the
//! checks made while the agent is idle or done. A delivery keeps its
//! wall-clock window.

use gear_engine::ops::confirm_pending_with;
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{OpKind, OpState, SeatRole};
use gear_store::{testing, NewOp, Store};

const T0: i64 = 1_800_000_000;
const LANE: &str = "p:heat";
const PANE: &str = "w1:p2";

/// A project with one lane and a Submitted doorbell op for it: the doorbell
/// waits for the lane to look at its home page.
fn world() -> (Store, String) {
    let mut s = Store::open_in_memory().unwrap();
    let id = s
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "pi", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T0)?;
            let op = tx.insert_op(&NewOp {
                project: "p".into(),
                seat: LANE.into(),
                generation: 1,
                kind: OpKind::Doorbell,
                task: None,
                attempt: None,
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({"text": "GEAR: 1 item(s) waiting for you", "covers": ["M-1"]}),
                created: T0 + 1,
            })?;
            tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 1)?;
            tx.set_op_state("p", &op.id, OpState::Submitted, None, T0 + 2)?;
            Ok(op.id)
        })
        .unwrap();
    (s, id)
}

fn working() -> Vec<String> {
    vec![LANE.to_string()]
}

#[test]
fn a_doorbell_to_a_working_seat_spends_no_budget_and_flags_nothing() {
    let (mut s, op) = world();

    // More checks than the budget (2) — the lane works through all of them.
    for at in [T0 + 10, T0 + 20, T0 + 30, T0 + 40] {
        let (confirmed, flagged) =
            confirm_pending_with(&mut s, "p", 2, 180, &working(), at).unwrap();
        assert!(confirmed.is_empty(), "{confirmed:?}");
        assert!(
            flagged.is_empty(),
            "a working lane is not flagged: {flagged:?}"
        );
    }

    let stored = s.read(|tx| tx.get_op("p", &op)).unwrap().unwrap();
    assert_eq!(stored.state, OpState::Submitted, "the doorbell still waits");
    assert_eq!(stored.check_budget, 0, "no check was spent on it");
}

#[test]
fn the_budget_counts_again_once_the_agent_is_idle() {
    let (mut s, op) = world();

    // The lane works: no budget is spent.
    let (_, flagged) = confirm_pending_with(&mut s, "p", 2, 180, &working(), T0 + 10).unwrap();
    assert!(flagged.is_empty());

    // The agent is idle now: the budget counts as before.
    let (confirmed, flagged) = confirm_pending_with(&mut s, "p", 2, 180, &[], T0 + 20).unwrap();
    assert!(confirmed.is_empty());
    assert!(flagged.is_empty(), "one check is not yet the budget");
    assert_eq!(
        s.read(|tx| tx.get_op("p", &op))
            .unwrap()
            .unwrap()
            .check_budget,
        1
    );

    // The second check spends the budget: the seat is flagged.
    let (_, flagged) = confirm_pending_with(&mut s, "p", 2, 180, &[], T0 + 30).unwrap();
    assert_eq!(flagged, vec![LANE.to_string()]);
    assert_eq!(
        s.read(|tx| tx.get_op("p", &op))
            .unwrap()
            .unwrap()
            .check_budget,
        2
    );
}

#[test]
fn a_delivery_keeps_its_window_while_the_agent_works() {
    let mut s = Store::open_in_memory().unwrap();
    let op = s
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T0)?;
            let op = tx.insert_op(&NewOp {
                project: "p".into(),
                seat: LANE.into(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some("T-7".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({"text": "New task T-7"}),
                created: T0 + 1,
            })?;
            tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 1)?;
            tx.set_op_state("p", &op.id, OpState::Submitted, None, T0 + 2)?;
            Ok(op.id)
        })
        .unwrap();

    // The window (8 s) is wall clock: a working agent is not spared it.
    let (confirmed, flagged) = confirm_pending_with(&mut s, "p", 3, 8, &working(), T0 + 5).unwrap();
    assert!(confirmed.is_empty());
    assert!(
        flagged.is_empty(),
        "the window is not over yet: {flagged:?}"
    );

    let (_, flagged) = confirm_pending_with(&mut s, "p", 3, 8, &working(), T0 + 30).unwrap();
    assert_eq!(
        flagged,
        vec![LANE.to_string()],
        "the delivery failed as before"
    );
    assert_eq!(
        s.read(|tx| tx.get_op("p", &op)).unwrap().unwrap().state,
        OpState::Failed
    );
}

/// The whole tick: a lane that works through the doorbell's budget opens no
/// stall item.
#[test]
fn a_working_lane_opens_no_stall_item() {
    let (mut s, op) = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p1", "pi", AgentStatus::Idle);
    herdr.add_agent(PANE, "pi", AgentStatus::Working);
    let cfg = gear_engine::EngineCfg {
        profiles: gear_herdr::builtins(),
        doorbell_check_budget: 2,
        ..Default::default()
    };

    for at in [T0 + 10, T0 + 20, T0 + 30] {
        let report = gear_engine::tick(&mut s, &mut herdr, &cfg, at).unwrap();
        assert!(
            !report.flagged.contains(&LANE.to_string()),
            "the working lane is not flagged: {:?}",
            report.flagged
        );
    }

    let items = s
        .read(|tx| tx.thread_messages("p", "stall:p:heat"))
        .unwrap();
    assert!(items.is_empty(), "no stall item was opened: {items:?}");
    let stored = s.read(|tx| tx.get_op("p", &op)).unwrap().unwrap();
    assert_eq!(stored.state, OpState::Submitted, "the doorbell still waits");
    assert_eq!(stored.check_budget, 0, "no check was spent on it");
}
