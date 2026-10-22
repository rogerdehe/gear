//! Tests for `gear_engine::ops::confirm_pending` (T-20).

use gear_engine::ops::confirm_pending;
use gear_model::{OpKind, OpState, SeatRole};
use gear_store::{testing, NewOp, Store, ViewRecord};

const T0: i64 = 1_800_000_000;
const SEAT: &str = "p:captain";

fn store_with_project() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess-p", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            Ok(())
        })
        .unwrap();
    store
}

/// A Submitted doorbell op covering `covers`, created at `created`.
fn submitted_op(store: &mut Store, covers: &[&str], created: i64) -> String {
    store
        .write(|tx| {
            let id = tx
                .insert_op(&NewOp {
                    project: "p".into(),
                    seat: SEAT.into(),
                    generation: 1,
                    kind: OpKind::Doorbell,
                    task: None,
                    attempt: None,
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({ "text": "t", "covers": covers }),
                    created,
                })
                .unwrap()
                .id;
            tx.set_op_state("p", &id, OpState::Executing, None, created).unwrap();
            tx.set_op_state("p", &id, OpState::Submitted, None, created).unwrap();
            Ok(id)
        })
        .unwrap()
}

/// A view record: what the seat actually displayed.
fn view(store: &mut Store, generation: i64, ts: i64, shown: &[&str]) {
    store
        .write(|tx| {
            tx.insert_view(&ViewRecord {
                project: "p".into(),
                seat: SEAT.into(),
                generation,
                ts,
                shown_msgs: shown.iter().map(|m| m.to_string()).collect(),
                shown_ops: vec![],
                shown_task: None,
                shown_attempt: None,
                shown_contract: None,
            })?;
            Ok(())
        })
        .unwrap();
}

fn op(store: &mut Store, id: &str) -> gear_store::OpRow {
    store.read(|tx| tx.get_op("p", id)).unwrap().unwrap()
}

#[test]
fn matching_view_confirms() {
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-1"], T0 + 1);
    view(&mut store, 1, T0 + 5, &["M-1"]);

    let (confirmed, flagged) = confirm_pending(&mut store, "p", 3, T0 + 6).unwrap();

    assert_eq!(confirmed, vec![id.clone()]);
    assert!(flagged.is_empty());
    assert_eq!(op(&mut store, &id).state, OpState::Confirmed);
    assert_eq!(op(&mut store, &id).check_budget, 0, "a confirmed op needs no further checks");
}

#[test]
fn partial_view_does_not_confirm() {
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-1", "M-2"], T0 + 1);
    // The seat showed one of the two ids: that is not the whole doorbell.
    view(&mut store, 1, T0 + 5, &["M-1"]);

    let (confirmed, flagged) = confirm_pending(&mut store, "p", 3, T0 + 6).unwrap();

    assert!(confirmed.is_empty());
    assert!(flagged.is_empty());
    assert_eq!(op(&mut store, &id).state, OpState::Submitted);
    assert_eq!(op(&mut store, &id).check_budget, 1, "an unconfirmed check costs budget");
}

#[test]
fn older_view_does_not_confirm() {
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-1"], T0 + 10);
    // The view is from before the doorbell went out.
    view(&mut store, 1, T0 + 5, &["M-1"]);

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 6).unwrap();

    assert!(confirmed.is_empty());
    assert_eq!(op(&mut store, &id).state, OpState::Submitted);
}

#[test]
fn other_generation_does_not_confirm() {
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-1"], T0 + 1);
    // The view belongs to another generation of the seat.
    view(&mut store, 2, T0 + 5, &["M-1"]);

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 6).unwrap();

    assert!(confirmed.is_empty());
    assert_eq!(op(&mut store, &id).state, OpState::Submitted);
}

#[test]
fn budget_exhaustion_flags_seat() {
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-1"], T0 + 1);

    let (confirmed, flagged) = confirm_pending(&mut store, "p", 2, T0 + 6).unwrap();
    assert!(confirmed.is_empty());
    assert!(flagged.is_empty(), "one check is not yet the budget");
    assert_eq!(op(&mut store, &id).check_budget, 1);

    let (_, flagged) = confirm_pending(&mut store, "p", 2, T0 + 7).unwrap();
    assert_eq!(flagged, vec![SEAT.to_string()], "the seat needs a check");

    let (_, flagged) = confirm_pending(&mut store, "p", 2, T0 + 8).unwrap();
    assert!(flagged.is_empty(), "the seat is flagged once, not on every check");
}

#[test]
fn no_resend_on_exhaustion() {
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-1"], T0 + 1);

    let (confirmed, flagged) = confirm_pending(&mut store, "p", 1, T0 + 6).unwrap();
    assert!(confirmed.is_empty());
    assert_eq!(flagged, vec![SEAT.to_string()], "the budget of one check is used up");

    // A later check flags nothing new and resends nothing.
    let (_, flagged) = confirm_pending(&mut store, "p", 1, T0 + 7).unwrap();
    assert!(flagged.is_empty());

    // Nothing new was written: the op stays where it is and no second op exists.
    assert_eq!(op(&mut store, &id).state, OpState::Submitted);
    let ops = store
        .read(|tx| {
            Ok((
                tx.ops_in_state("p", OpState::Intended)?.len(),
                tx.ops_in_state("p", OpState::Executing)?.len(),
                tx.ops_in_state("p", OpState::Submitted)?.len(),
                tx.ops_in_state("p", OpState::Unknown)?.len(),
            ))
        })
        .unwrap();
    assert_eq!(ops, (0, 0, 1, 0), "no new op was written");
}
