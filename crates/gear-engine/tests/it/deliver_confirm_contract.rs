//! Contract: Deliver ops are confirmed by a view showing their task/attempt (T-48).

use gear_app::tasks::start_attempt;
use gear_engine::ops::confirm_pending;
use gear_model::{AttemptState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, NewOp, Store, TaskRow, ViewRecord};

const T0: i64 = 1_800_000_000;

fn world() -> (Store, String) {
    let mut s = Store::open_in_memory().unwrap();
    let id = s
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-7".into(),
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
                updated: T0,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-7".into(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
            start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?;
            let op = tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some("T-7".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({"text": "New task T-7"}),
                created: T0 + 10,
            })?;
            tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 10)?;
            tx.set_op_state("p", &op.id, OpState::Submitted, None, T0 + 11)?;
            Ok(op.id)
        })
        .unwrap();
    (s, id)
}

fn view(s: &mut Store, ts: i64, task: Option<&str>, attempt: Option<i64>) {
    s.write(|tx| {
        tx.insert_view(&ViewRecord {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            ts,
            shown_msgs: vec![],
            shown_ops: vec![],
            shown_task: task.map(String::from),
            shown_attempt: attempt,
            shown_contract: attempt.map(|_| 1),
        })
    })
    .unwrap();
}

#[test]
fn a_view_of_the_task_confirms_the_delivery_and_starts_the_attempt() {
    let (mut s, op) = world();
    view(&mut s, T0 + 20, Some("T-7"), Some(1));
    let (confirmed, flagged) = confirm_pending(&mut s, "p", 3, T0 + 30).unwrap();
    assert_eq!((confirmed, flagged), (vec![op.clone()], vec![]));
    assert_eq!(
        s.read(|tx| tx.get_op("p", &op)).unwrap().unwrap().state,
        OpState::Confirmed
    );
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-7", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Started
    );
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-7")).unwrap().unwrap().state,
        TaskState::Running
    );
}

#[test]
fn views_without_the_task_or_too_early_do_not_confirm() {
    let (mut s, op) = world();
    view(&mut s, T0 + 5, Some("T-7"), Some(1)); // before the op
    view(&mut s, T0 + 20, None, None); // home without the task
    view(&mut s, T0 + 21, Some("T-7"), Some(2)); // other attempt
    let (confirmed, _) = confirm_pending(&mut s, "p", 3, T0 + 30).unwrap();
    assert!(confirmed.is_empty());
    assert_eq!(
        s.read(|tx| tx.get_op("p", &op)).unwrap().unwrap().state,
        OpState::Submitted
    );
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-7", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Delivered
    );
}

/// The world of the T-407 tests: the same fixture under a task of its own, so
/// a test that reads the engine log never mixes in another test's lines.
fn probe_world(task: &str) -> (Store, String) {
    let mut s = Store::open_in_memory().unwrap();
    let id = s
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: task.into(),
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
                updated: T0,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: task.into(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
            start_attempt(tx, "p", task, "p:heat", 1, T0)?;
            let op = tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some(task.into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({"text": format!("New task {task}")}),
                created: T0 + 10,
            })?;
            tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 10)?;
            tx.set_op_state("p", &op.id, OpState::Submitted, None, T0 + 11)?;
            Ok(op.id)
        })
        .unwrap();
    (s, id)
}

/// T-407: a delivery confirmed 40 s after it was typed is one probe line with
/// that step's time in milliseconds, from the op's own durable times.
#[test]
fn a_delivery_confirmed_after_forty_seconds_logs_its_step_in_ms() {
    let (mut s, _) = probe_world("T-707");
    view(&mut s, T0 + 51, Some("T-707"), Some(1));
    let (confirmed, _) = confirm_pending(&mut s, "p", 3, T0 + 51).unwrap();
    assert_eq!(confirmed.len(), 1, "the delivery is confirmed");
    let log: Vec<String> = gear_engine::ops::probe_log()
        .into_iter()
        .filter(|line| line.starts_with("probe deliver T-707 "))
        .collect();
    // The step, then the delivery's own completion line (T-407): typed at
    // T0+11, confirmed at T0+51, intended at T0+10.
    assert_eq!(
        log,
        [
            "probe deliver T-707 confirmed 40000 total=41000".to_string(),
            "probe deliver T-707 done 41000 total=41000".to_string()
        ],
        "{log:?}"
    );
}

/// T-407: a delivery the lane never showed fails in the engine log the same
/// way, with its own step.
#[test]
fn a_delivery_that_was_never_read_logs_failed() {
    // Its own task, so the other probe test's lines never mix in (T-407).
    let (mut s, _) = probe_world("T-708");
    let (confirmed, flagged) = confirm_pending(&mut s, "p", 3, T0 + 191).unwrap();
    assert!(confirmed.is_empty());
    assert!(
        flagged.is_empty(),
        "the lane's own retry comes first: {flagged:?}"
    );
    let log: Vec<String> = gear_engine::ops::probe_log()
        .into_iter()
        .filter(|line| line.starts_with("probe deliver T-708 "))
        .collect();
    assert_eq!(
        log,
        [
            "probe deliver T-708 failed 180000 total=181000".to_string(),
            "probe deliver T-708 done 181000 total=181000".to_string()
        ],
        "{log:?}"
    );
}

/// T-407: a delivery that is typed logs both boundaries of the one Herdr call
/// that types the text and sends the Enter — `typed` and `enter` — and then
/// the delivery's own operation line.
#[test]
fn a_typed_delivery_logs_its_typed_and_enter_boundaries() {
    use gear_herdr::fake::FakeHerdr;
    use gear_herdr::AgentStatus;

    let (mut s, _) = probe_world("T-709");
    // The packet of T-709 is still to be typed: an Intended op.
    let op = s
        .write(|tx| {
            tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some("T-709".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({"text": "New task T-709"}),
                created: T0 + 100,
            })
        })
        .unwrap();
    let mut h = FakeHerdr::new();
    h.add_agent_named("w1:p2", "opencode", AgentStatus::Idle, "p-heat");
    let executed = gear_engine::ops::execute_intended(&mut s, &mut h, "p", T0 + 130).unwrap();
    assert_eq!(
        executed,
        vec![(op.id.clone(), OpState::Submitted.as_str().to_string())],
        "the packet was typed"
    );
    let log: Vec<String> = gear_engine::ops::probe_log()
        .into_iter()
        .filter(|line| line.starts_with("probe deliver T-709 "))
        .collect();
    // The typing runs from the intent (30 s); the Enter follows it in the same
    // call, so its own step is 0 ms. Both carry the delivery's time so far.
    assert_eq!(
        log,
        [
            "probe deliver T-709 typed 30000 total=30000".to_string(),
            "probe deliver T-709 enter 0 total=30000".to_string()
        ],
        "{log:?}"
    );
}
