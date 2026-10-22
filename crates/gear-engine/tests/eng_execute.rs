//! Tests for `gear_engine::ops::execute_intended` (T-19).

use gear_engine::ops::execute_intended;
use gear_herdr::fake::{FakeHerdr, Inject};
use gear_herdr::AgentStatus;
use gear_model::{OpKind, OpState, SeatRole, SeatState};
use gear_store::{testing, NewOp, Store, Tx};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const OTHER_PANE: &str = "w2:p1";
const TEXT: &str = "GEAR: 1 item(s) waiting for you. Run `gear` to see them.";

/// A store with project `p` (captain `p:captain` on `w1:p1`) and project `q`
/// (captain `q:captain` on `w2:p1`).
fn store_with_projects() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess-p", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
            testing::seed_project(tx, "q", "sess-q", T0)?;
            testing::seed_active_seat(tx, "q", "captain", SeatRole::Captain, OTHER_PANE, "codex", T0)?;
            Ok(())
        })
        .unwrap();
    store
}

/// An Intended doorbell op for `seat` in `project`.
fn insert_doorbell(tx: &Tx<'_>, project: &str, seat: &str, generation: i64, now: i64) -> String {
    tx.insert_op(&NewOp {
        project: project.into(),
        seat: seat.into(),
        generation,
        kind: OpKind::Doorbell,
        task: None,
        attempt: None,
        ref_msg: None,
        plan_version: None,
        payload: serde_json::json!({ "text": TEXT, "covers": ["M-1"] }),
        created: now,
    })
    .unwrap()
    .id
}

fn herdr_with_agent(pane: &str) -> FakeHerdr {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(pane, "codex", AgentStatus::Idle);
    herdr
}

fn op_state(store: &mut Store, project: &str, id: &str) -> OpState {
    store.read(|tx| tx.get_op(project, id)).unwrap().unwrap().state
}

#[test]
fn ok_is_submitted_and_prompts_text() {
    let mut store = store_with_projects();
    store
        .write(|tx| {
            insert_doorbell(tx, "p", "p:captain", 1, T0 + 1);
            Ok(())
        })
        .unwrap();
    let mut herdr = herdr_with_agent(CAP_PANE);

    let done = execute_intended(&mut store, &mut herdr, "p", T0 + 2).unwrap();

    assert_eq!(done, vec![("O-1".to_string(), "submitted".to_string())]);
    assert_eq!(op_state(&mut store, "p", "O-1"), OpState::Submitted);
    assert_eq!(herdr.prompts_to(CAP_PANE), vec![TEXT], "the doorbell text is prompted");
}

#[test]
fn refused_is_failed() {
    let mut store = store_with_projects();
    store
        .write(|tx| {
            insert_doorbell(tx, "p", "p:captain", 1, T0 + 1);
            Ok(())
        })
        .unwrap();
    let mut herdr = herdr_with_agent(CAP_PANE);
    herdr.inject_next = Some(Inject::Refuse { code: "agent_blocked".into() });

    let done = execute_intended(&mut store, &mut herdr, "p", T0 + 2).unwrap();

    assert_eq!(done, vec![("O-1".to_string(), "failed".to_string())]);
    assert_eq!(op_state(&mut store, "p", "O-1"), OpState::Failed);
}

#[test]
fn uncertain_is_unknown() {
    let mut store = store_with_projects();
    store
        .write(|tx| {
            insert_doorbell(tx, "p", "p:captain", 1, T0 + 1);
            Ok(())
        })
        .unwrap();
    let mut herdr = herdr_with_agent(CAP_PANE);
    herdr.inject_next = Some(Inject::Uncertain);

    let done = execute_intended(&mut store, &mut herdr, "p", T0 + 2).unwrap();

    assert_eq!(done, vec![("O-1".to_string(), "unknown".to_string())]);
    assert_eq!(op_state(&mut store, "p", "O-1"), OpState::Unknown);
}

#[test]
fn stale_generation_is_void_not_sent() {
    let mut store = store_with_projects();
    store
        .write(|tx| {
            insert_doorbell(tx, "p", "p:captain", 1, T0 + 1);
            // The seat was re-onboarded: its generation moved on.
            tx.set_seat_generation("p:captain", 2).unwrap();
            Ok(())
        })
        .unwrap();
    let mut herdr = herdr_with_agent(CAP_PANE);

    let done = execute_intended(&mut store, &mut herdr, "p", T0 + 2).unwrap();

    assert_eq!(done, vec![("O-1".to_string(), "void".to_string())]);
    assert_eq!(op_state(&mut store, "p", "O-1"), OpState::Void);
    assert!(herdr.calls.is_empty(), "a stale op is never sent");
}

#[test]
fn unreachable_seat_is_void() {
    let mut store = store_with_projects();
    store
        .write(|tx| {
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            insert_doorbell(tx, "p", "p:heat", 1, T0 + 1);
            // Parked is not reachable (design/08): no doorbell may go out.
            tx.set_seat_state("p:heat", SeatState::Parked, Some("parked"), T0 + 1).unwrap();
            Ok(())
        })
        .unwrap();
    let mut herdr = herdr_with_agent("w1:p2");

    let done = execute_intended(&mut store, &mut herdr, "p", T0 + 2).unwrap();

    assert_eq!(done, vec![("O-1".to_string(), "void".to_string())]);
    assert_eq!(op_state(&mut store, "p", "O-1"), OpState::Void);
    assert!(herdr.calls.is_empty(), "an unreachable seat is never prompted");
}

#[test]
fn other_projects_untouched() {
    let mut store = store_with_projects();
    store
        .write(|tx| {
            insert_doorbell(tx, "p", "p:captain", 1, T0 + 1);
            insert_doorbell(tx, "q", "q:captain", 1, T0 + 1);
            Ok(())
        })
        .unwrap();
    let mut herdr = herdr_with_agent(CAP_PANE);
    herdr.add_agent(OTHER_PANE, "codex", AgentStatus::Idle);

    let done = execute_intended(&mut store, &mut herdr, "p", T0 + 2).unwrap();

    assert_eq!(done, vec![("O-1".to_string(), "submitted".to_string())]);
    assert_eq!(op_state(&mut store, "p", "O-1"), OpState::Submitted);
    assert_eq!(op_state(&mut store, "q", "O-1"), OpState::Intended, "another project waits");
    assert!(herdr.prompts_to(OTHER_PANE).is_empty(), "no prompt outside the project");
}
