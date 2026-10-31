//! Contract (L-52): a flagged lane reaches the captain's inbox, once.

use gear_engine::stall::{alert, alert_fact};
use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::{AgentStatus, HerdrAct};
use gear_model::{AttemptState, MsgKind, MsgState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, NewMessage, NewOp, Store, TaskRow, ViewRecord};

const T0: i64 = 1_800_000_000;

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "claude", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        Ok(())
    })
    .unwrap();
    s
}

fn captain_items(s: &mut Store) -> Vec<(String, MsgState)> {
    s.read(|tx| tx.thread_messages("p", "stall:p:heat"))
        .unwrap()
        .into_iter()
        .map(|m| (m.recipient, m.state))
        .collect()
}

#[test]
fn a_flagged_lane_opens_one_captain_item() {
    let mut s = world();
    assert!(s.write(|tx| alert(tx, "p:heat", T0 + 10)).unwrap());
    // Flagged again while the item is open: nothing new.
    assert!(!s.write(|tx| alert(tx, "p:heat", T0 + 20)).unwrap());
    assert_eq!(
        captain_items(&mut s),
        vec![("p:captain".to_string(), MsgState::Open)]
    );
}

#[test]
fn the_captain_itself_is_not_alerted() {
    let mut s = world();
    assert!(!s.write(|tx| alert(tx, "p:captain", T0)).unwrap());
}

fn body(s: &mut Store) -> String {
    s.read(|tx| tx.thread_messages("p", "stall:p:heat"))
        .unwrap()
        .pop()
        .expect("the captain's item")
        .body
}

/// T-293: the stuck item says what it is about: the work in flight, the last
/// check-in and the delivery op the lane waits on.
#[test]
fn a_stuck_item_says_what_the_lane_is_about() {
    let mut s = world();
    let now = T0 + 600;
    s.write(|tx| {
        tx.insert_task(&TaskRow {
            project: "p".into(),
            id: "T-1".into(),
            rank: 1,
            state: TaskState::Running,
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
            estimate_min: Some(30),
            issue: None,
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            updated: T0,
        })?;
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
            state: AttemptState::Started,
            candidate: None,
            report: None,
            safe_point: None,
            started: T0,
            updated: T0,
        })?;
        // The lane checked in 7 min ago.
        tx.insert_view(&ViewRecord {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            ts: now - 7 * 60,
            shown_msgs: vec![],
            shown_ops: vec![],
            shown_task: Some("T-1".into()),
            shown_attempt: Some(1),
            shown_contract: Some(1),
        })?;
        // And a deliver op was submitted 4 min ago and is still unconfirmed.
        let op = tx.insert_op(&NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: OpKind::Deliver,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({"text": "New task T-1"}),
            created: now - 4 * 60,
        })?;
        tx.set_op_state("p", &op.id, OpState::Executing, None, now - 4 * 60)?;
        tx.set_op_state("p", &op.id, OpState::Submitted, None, now - 4 * 60)
    })
    .unwrap();

    // The lane waits for the provider: a back-off closes nothing, so the item
    // is raised as it would be for any stall.
    s.write(|tx| alert_fact(tx, "p:heat", now, "a delivery was not confirmed"))
        .unwrap();

    let body = body(&mut s);
    assert!(
        body.contains("About: T-1 attempt 1 (started); last check-in 7 min ago; waits on O-"),
        "{body}"
    );
    assert!(body.contains("(deliver, submitted, 4 min)"), "{body}");
}

/// T-293: a lane with no attempt, no view and no slot op says so.
#[test]
fn a_lane_with_nothing_says_no_task_and_no_check_in() {
    let mut s = world();
    s.write(|tx| alert_fact(tx, "p:heat", T0 + 10, "a delivery was not confirmed"))
        .unwrap();
    let body = body(&mut s);
    assert!(
        body.contains("About: no task; no check-in this generation"),
        "{body}"
    );
}

/// A project whose lane the engine flags: a doorbell it never looks at, so the
/// check budget (1) runs out on the first tick. The captain has asked the lane
/// for something and it has not answered.
fn flagged_world() -> (Store, FakeHerdr, EngineCfg) {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "pi", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "pi", T0)?;
            tx.insert_message(&NewMessage {
                meta: None, project: "p".into(), thread: "hull:175".into(), task: None,
                attempt: None, sender: "p:captain".into(), sender_gen: 1,
                recipient: "p:heat".into(), kind: MsgKind::Instruction, urgent: false,
                body: "please look at the merge".into(), reply_to: None, needs_ack: false,
                state: MsgState::Open, created: T0,
            })?;
            let op = tx.insert_op(&NewOp {
                project: "p".into(), seat: "p:heat".into(), generation: 1,
                kind: OpKind::Doorbell, task: None, attempt: None, ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({"text": "GEAR: 1 item(s) waiting for you", "covers": ["M-1"]}),
                created: T0 + 1,
            })?;
            tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 1)?;
            tx.set_op_state("p", &op.id, OpState::Submitted, None, T0 + 2)
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p1", "pi", AgentStatus::Idle);
    herdr.add_agent("w1:p2", "pi", AgentStatus::Idle);
    let cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        doorbell_check_budget: 1,
        ..Default::default()
    };
    (store, herdr, cfg)
}

/// T-324: a flagged lane that helps on an unanswered captain instruction is
/// busy on purpose: no stall item while its agent is present.
#[test]
fn a_flagged_helping_lane_whose_agent_is_present_gets_no_stall_item() {
    let (mut store, mut herdr, cfg) = flagged_world();
    let report = tick(&mut store, &mut herdr, &cfg, T0 + 10).unwrap();
    assert!(
        report.flagged.iter().any(|seat| seat == "p:heat"),
        "the doorbell budget flags the lane: {:?}",
        report.flagged
    );
    assert!(
        captain_items(&mut store).is_empty(),
        "a helping lane is not stuck: {:?}",
        captain_items(&mut store)
    );
}

/// T-324: a missing agent ("its agent is gone from its pane") is still
/// alerted, whatever the lane was helping with.
#[test]
fn a_flagged_helping_lane_whose_agent_is_missing_still_gets_the_stall_item() {
    let (mut store, mut herdr, cfg) = flagged_world();
    let _ = herdr.pane_close("w1:p2");
    tick(&mut store, &mut herdr, &cfg, T0 + 10).unwrap();
    let items = captain_items(&mut store);
    assert_eq!(items.len(), 1, "{items:?}");
    let body = body(&mut store);
    assert!(body.contains("its agent is gone from its pane"), "{body}");
}
