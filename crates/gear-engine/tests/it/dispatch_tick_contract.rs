//! Contract for plan_dispatch / intend_dispatch (T-46). Needs T-41 (lane_view)
//! and T-42 (start_attempt).

use gear_engine::dispatch::{continue_text, first_attempt_text, intend_dispatch, packet_ref, plan_dispatch, task_branch, Action, DispatchCfg};
use gear_engine::observe::{Presence, SeatObs};
use gear_herdr::AgentStatus;
use gear_model::{AttemptState, OpKind, SeatRole, TaskState};
use gear_store::{testing, ContractRow, SeatRow, Store, TaskRow};
use std::collections::BTreeSet;

const T0: i64 = 1_800_000_000;

fn world() -> (Store, SeatRow, SeatRow) {
    let mut s = Store::open_in_memory().unwrap();
    let (cap, heat) = s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        let cap = testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
        let heat = testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        tx.insert_task(&TaskRow { repo: None, base: None, gate: None, rework_estimate_min: None, project: "p".into(), id: "T-7".into(), rank: 1, state: TaskState::Ready, priority: "must".into(),
            lane: Some("heat".into()), milestone: None, required: false, contract_version: 1, deps: vec![], followup_of: None,
            wait_on: None, wait_cond: None, rework_note: None, estimate_min: None,  issue: None,updated: T0 })?;
        tx.insert_contract(&ContractRow { project: "p".into(), task: "T-7".into(), version: 1, goal: "g".into(), accept: "a".into(),
            evidence_files: vec![], evidence_due_min: None, body: String::new(), created: T0, mechanical: None, })?;
        Ok((cap, heat))
    }).unwrap();
    (s, cap, heat)
}

fn idle(seat: &SeatRow) -> SeatObs {
    SeatObs { seat: seat.clone(), presence: Presence::Present, status: Some(AgentStatus::Idle), flags: BTreeSet::new(), idle: true, screen: None, session: None }
}

#[test]
fn helpers() {
    assert_eq!(packet_ref("T-7"), "`gear --show T-7`");
    assert_eq!(task_branch(gear_model::branch::TASK_BRANCH, "p", "heat", "T-7"), "heat/t7");
}

#[test]
fn idle_lane_is_assigned_and_delivered() {
    let (mut s, cap, heat) = world();
    let obs = vec![idle(&cap), idle(&heat)];
    let actions = s.read(|tx| plan_dispatch(tx, "p", &obs, &DispatchCfg::default(), T0 + 100)).unwrap();
    assert_eq!(actions.len(), 1, "only lane seats");
    assert_eq!(actions[0].1, Action::Assign { task: "T-7".into() });
    let ops = s.write(|tx| intend_dispatch(tx, "p", &actions, T0 + 100)).unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!((ops[0].kind, ops[0].task.as_deref(), ops[0].attempt), (OpKind::Deliver, Some("T-7"), Some(1)));
    assert_eq!(ops[0].payload["text"], first_attempt_text("T-7", 1, "heat/t7", "origin", "dev", "`gear --show T-7`").as_str());
    assert_eq!(s.read(|tx| tx.get_attempt("p", "T-7", 1)).unwrap().unwrap().state, AttemptState::Delivered);
    // the pending delivery now blocks a second decision
    let again = s.read(|tx| plan_dispatch(tx, "p", &obs, &DispatchCfg::default(), T0 + 1000)).unwrap();
    assert!(matches!(again[0].1, Action::None { .. }));
}

#[test]
fn continue_uses_the_open_attempt() {
    let (mut s, _cap, heat) = world();
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-7", 1, T0)
    }).unwrap();
    let actions = vec![(heat.clone(), Action::Continue { task: "T-7".into() })];
    let ops = s.write(|tx| intend_dispatch(tx, "p", &actions, T0 + 200)).unwrap();
    assert_eq!(ops[0].payload["text"], continue_text("T-7", 1, "`gear --show T-7`").as_str());
    assert_eq!(ops[0].attempt, Some(1));
    let none = vec![(heat, Action::None { reason: "x".into() })];
    assert!(s.write(|tx| intend_dispatch(tx, "p", &none, T0 + 300)).unwrap().is_empty());
}
