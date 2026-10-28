//! Contract: Deliver ops are confirmed by a view showing their task/attempt (T-48).

use gear_app::tasks::start_attempt;
use gear_engine::ops::confirm_pending;
use gear_model::{AttemptState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, NewOp, Store, TaskRow, ViewRecord};

const T0: i64 = 1_800_000_000;

fn world() -> (Store, String) {
    let mut s = Store::open_in_memory().unwrap();
    let id = s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        tx.insert_task(&TaskRow { repo: None, base: None, gate: None, rework_estimate_min: None, project: "p".into(), id: "T-7".into(), rank: 1, state: TaskState::Ready, priority: "must".into(),
            lane: Some("heat".into()), milestone: None, required: false, contract_version: 1, deps: vec![], followup_of: None,
            wait_on: None, wait_cond: None, rework_note: None, estimate_min: None,  issue: None,updated: T0 })?;
        tx.insert_contract(&ContractRow { project: "p".into(), task: "T-7".into(), version: 1, goal: "g".into(), accept: "a".into(),
            evidence_files: vec![], evidence_due_min: None, body: String::new(), created: T0, mechanical: None, })?;
        start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?;
        let op = tx.insert_op(&NewOp { project: "p".into(), seat: "p:heat".into(), generation: 1, kind: OpKind::Deliver,
            task: Some("T-7".into()), attempt: Some(1), ref_msg: None, plan_version: None,
            payload: serde_json::json!({"text": "New task T-7"}), created: T0 + 10 })?;
        tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 10)?;
        tx.set_op_state("p", &op.id, OpState::Submitted, None, T0 + 11)?;
        Ok(op.id)
    }).unwrap();
    (s, id)
}

fn view(s: &mut Store, ts: i64, task: Option<&str>, attempt: Option<i64>) {
    s.write(|tx| tx.insert_view(&ViewRecord { project: "p".into(), seat: "p:heat".into(), generation: 1, ts,
        shown_msgs: vec![], shown_ops: vec![], shown_task: task.map(String::from), shown_attempt: attempt, shown_contract: attempt.map(|_| 1) })).unwrap();
}

#[test]
fn a_view_of_the_task_confirms_the_delivery_and_starts_the_attempt() {
    let (mut s, op) = world();
    view(&mut s, T0 + 20, Some("T-7"), Some(1));
    let (confirmed, flagged) = confirm_pending(&mut s, "p", 3, T0 + 30).unwrap();
    assert_eq!((confirmed, flagged), (vec![op.clone()], vec![]));
    assert_eq!(s.read(|tx| tx.get_op("p", &op)).unwrap().unwrap().state, OpState::Confirmed);
    assert_eq!(s.read(|tx| tx.get_attempt("p", "T-7", 1)).unwrap().unwrap().state, AttemptState::Started);
    assert_eq!(s.read(|tx| tx.get_task("p", "T-7")).unwrap().unwrap().state, TaskState::Running);
}

#[test]
fn views_without_the_task_or_too_early_do_not_confirm() {
    let (mut s, op) = world();
    view(&mut s, T0 + 5, Some("T-7"), Some(1));   // before the op
    view(&mut s, T0 + 20, None, None);            // home without the task
    view(&mut s, T0 + 21, Some("T-7"), Some(2));  // other attempt
    let (confirmed, _) = confirm_pending(&mut s, "p", 3, T0 + 30).unwrap();
    assert!(confirmed.is_empty());
    assert_eq!(s.read(|tx| tx.get_op("p", &op)).unwrap().unwrap().state, OpState::Submitted);
    assert_eq!(s.read(|tx| tx.get_attempt("p", "T-7", 1)).unwrap().unwrap().state, AttemptState::Delivered);
}
