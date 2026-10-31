//! Tests for `gear_engine::ops::intend_doorbells` and `recover_on_start` (T-18).

use gear_engine::doorbell::DoorbellAction;
use gear_engine::ops::{intend_doorbells, recover_on_start};
use gear_model::{OpKind, OpState, SeatRole};
use gear_store::{testing, NewOp, Store, Tx};

const T0: i64 = 1_800_000_000;

fn action(project: &str, seat: &str, generation: i64, covers: &[&str]) -> DoorbellAction {
    DoorbellAction {
        project: project.into(),
        seat: seat.into(),
        generation,
        pane_id: "w1:p1".into(),
        text: "GEAR: 1 item(s) waiting for you: M-1. Run the command `gear` (no arguments) in your own shell; it shows only your items.".into(),
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

/// Insert a bare op with the store's raw writer (state Intended) and return its
/// id.
fn insert_op(tx: &Tx<'_>, project: &str, seat: &str, now: i64) -> String {
    tx.insert_op(&NewOp {
        project: project.into(),
        seat: seat.into(),
        generation: 1,
        kind: OpKind::Doorbell,
        task: None,
        attempt: None,
        ref_msg: None,
        plan_version: None,
        payload: serde_json::json!({ "text": "x", "covers": [] }),
        created: now,
    })
    .unwrap()
    .id
}

/// Walk an op to `state` through legal transitions.
fn walk_to(tx: &Tx<'_>, project: &str, id: &str, state: OpState, now: i64) {
    match state {
        OpState::Intended => {}
        OpState::Executing => tx
            .set_op_state(project, id, OpState::Executing, None, now)
            .unwrap(),
        OpState::Submitted => {
            tx.set_op_state(project, id, OpState::Executing, None, now)
                .unwrap();
            tx.set_op_state(project, id, OpState::Submitted, None, now)
                .unwrap();
        }
        OpState::Confirmed => {
            tx.set_op_state(project, id, OpState::Executing, None, now)
                .unwrap();
            tx.set_op_state(project, id, OpState::Submitted, None, now)
                .unwrap();
            tx.set_op_state(project, id, OpState::Confirmed, None, now)
                .unwrap();
        }
        OpState::Failed | OpState::Unknown => {
            tx.set_op_state(project, id, OpState::Executing, None, now)
                .unwrap();
            tx.set_op_state(project, id, state, None, now).unwrap();
        }
        OpState::Void => tx
            .set_op_state(project, id, OpState::Void, None, now)
            .unwrap(),
    }
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
        "GEAR: 1 item(s) waiting for you: M-1. Run the command `gear` (no arguments) in your own shell; it shows only your items."
    );
    assert_eq!(ops[0].payload["covers"], serde_json::json!(["M-3", "M-1"]));
}

#[test]
fn executing_becomes_unknown() {
    let mut store = store_with_projects();
    store
        .write(|tx| {
            intend_doorbells(
                tx,
                &[
                    action("p", "p:captain", 1, &["M-1"]),
                    action("q", "q:captain", 1, &["M-2"]),
                ],
                T0 + 1,
            )
        })
        .unwrap();
    store
        .write(|tx| {
            walk_to(tx, "p", "O-1", OpState::Executing, T0 + 2);
            walk_to(tx, "q", "O-1", OpState::Executing, T0 + 2);
            Ok(())
        })
        .unwrap();

    let recovered = recover_on_start(&mut store, T0 + 3).unwrap();

    // Every project is covered, and the pairs say which project each id is in.
    assert_eq!(
        recovered,
        vec![
            ("p".to_string(), "O-1".to_string()),
            ("q".to_string(), "O-1".to_string())
        ],
        "op ids are only unique within a project"
    );
    let states = store
        .read(|tx| {
            Ok((
                tx.get_op("p", "O-1")?.unwrap().state,
                tx.get_op("q", "O-1")?.unwrap().state,
            ))
        })
        .unwrap();
    assert_eq!(states, (OpState::Unknown, OpState::Unknown));
}

#[test]
fn two_projects_with_the_same_op_id_are_distinguishable() {
    let mut store = store_with_projects();
    store
        .write(|tx| {
            intend_doorbells(
                tx,
                &[
                    action("p", "p:captain", 1, &["M-1"]),
                    action("q", "q:captain", 1, &["M-2"]),
                ],
                T0 + 1,
            )
        })
        .unwrap();
    store
        .write(|tx| {
            walk_to(tx, "p", "O-1", OpState::Executing, T0 + 2);
            walk_to(tx, "q", "O-1", OpState::Executing, T0 + 2);
            Ok(())
        })
        .unwrap();

    let recovered = recover_on_start(&mut store, T0 + 3).unwrap();

    // Both projects have an op called O-1; the pairs keep them apart.
    assert_eq!(
        recovered,
        vec![
            ("p".to_string(), "O-1".to_string()),
            ("q".to_string(), "O-1".to_string())
        ]
    );
    let states = store
        .read(|tx| {
            Ok((
                tx.get_op("p", "O-1")?.unwrap().state,
                tx.get_op("q", "O-1")?.unwrap().state,
            ))
        })
        .unwrap();
    assert_eq!(states, (OpState::Unknown, OpState::Unknown));
}

#[test]
fn other_states_untouched() {
    let mut store = store_with_projects();
    store
        .write(|tx| intend_doorbells(tx, &[action("p", "p:captain", 1, &["M-1"])], T0 + 1))
        .unwrap();
    store
        .write(|tx| {
            for _ in 0..6 {
                insert_op(tx, "p", "p:captain", T0 + 1);
            }
            Ok(())
        })
        .unwrap();
    store
        .write(|tx| {
            walk_to(tx, "p", "O-1", OpState::Executing, T0 + 2);
            walk_to(tx, "p", "O-2", OpState::Intended, T0 + 2);
            walk_to(tx, "p", "O-3", OpState::Submitted, T0 + 2);
            walk_to(tx, "p", "O-4", OpState::Confirmed, T0 + 2);
            walk_to(tx, "p", "O-5", OpState::Failed, T0 + 2);
            walk_to(tx, "p", "O-6", OpState::Unknown, T0 + 2);
            walk_to(tx, "p", "O-7", OpState::Void, T0 + 2);
            Ok(())
        })
        .unwrap();

    let recovered = recover_on_start(&mut store, T0 + 3).unwrap();
    assert_eq!(
        recovered,
        vec![("p".to_string(), "O-1".to_string())],
        "only the Executing op is recovered"
    );

    let states = store
        .read(|tx| {
            Ok((
                tx.get_op("p", "O-1")?.unwrap().state,
                tx.get_op("p", "O-2")?.unwrap().state,
                tx.get_op("p", "O-3")?.unwrap().state,
                tx.get_op("p", "O-4")?.unwrap().state,
                tx.get_op("p", "O-5")?.unwrap().state,
                tx.get_op("p", "O-6")?.unwrap().state,
                tx.get_op("p", "O-7")?.unwrap().state,
            ))
        })
        .unwrap();
    assert_eq!(
        states,
        (
            OpState::Unknown,
            OpState::Intended,
            OpState::Submitted,
            OpState::Confirmed,
            OpState::Failed,
            OpState::Unknown,
            OpState::Void,
        )
    );
}
