//! Tests for `gear_engine::ops::intend_doorbells` and `recover_on_start` (T-18).

use gear_engine::doorbell::DoorbellAction;
use gear_engine::ops::intend_doorbells;
use gear_model::{OpKind, OpState, SeatRole};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;

fn action(project: &str, seat: &str, generation: i64, covers: &[&str]) -> DoorbellAction {
    DoorbellAction {
        project: project.into(),
        seat: seat.into(),
        generation,
        pane_id: "w1:p1".into(),
        text: "GEAR: 1 item(s) waiting for you. Run `gear` to see them.".into(),
        covers: covers.iter().map(|c| c.to_string()).collect(),
    }
}

/// A store with two projects, each with an onboarded captain.
fn store_with_projects() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess-p", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            testing::seed_project(tx, "q", "sess-q", T0)?;
            testing::seed_active_seat(tx, "q", "captain", SeatRole::Captain, "w2:p1", "codex", T0)?;
            Ok(())
        })
        .unwrap();
    store
}

#[test]
fn one_intended_op_per_action() {
    let mut store = store_with_projects();
    let ops = store
        .write(|tx| {
            intend_doorbells(
                tx,
                &[
                    action("p", "p:captain", 1, &["M-1"]),
                    action("q", "q:captain", 3, &["M-2"]),
                ],
                T0 + 1,
            )
        })
        .unwrap();

    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0].id, "O-1");
    assert_eq!(ops[0].project, "p");
    assert_eq!(ops[0].seat, "p:captain");
    assert_eq!(ops[0].generation, 1);
    assert_eq!(ops[0].kind, OpKind::Doorbell);
    assert_eq!(ops[0].state, OpState::Intended);
    assert_eq!(ops[0].created, T0 + 1);

    assert_eq!(ops[1].id, "O-1", "op ids are per project");
    assert_eq!(ops[1].project, "q");
    assert_eq!(ops[1].seat, "q:captain");
    assert_eq!(ops[1].generation, 3);
    assert_eq!(ops[1].state, OpState::Intended);

    // The ops are in the store, not only in the returned rows.
    let stored = store.read(|tx| tx.get_op("q", "O-1")).unwrap().unwrap();
    assert_eq!(stored.state, OpState::Intended);
    assert_eq!(stored.seat, "q:captain");
}

#[test]
fn payload_has_text_and_covers() {
    let mut store = store_with_projects();
    let ops = store
        .write(|tx| intend_doorbells(tx, &[action("p", "p:captain", 1, &["M-3", "M-1"])], T0 + 1))
        .unwrap();

    assert_eq!(
        ops[0].payload["text"],
        "GEAR: 1 item(s) waiting for you. Run `gear` to see them."
    );
    assert_eq!(ops[0].payload["covers"], serde_json::json!(["M-3", "M-1"]));
}
