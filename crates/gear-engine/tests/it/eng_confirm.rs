//! Tests for `gear_engine::ops::confirm_pending` (T-20, T-25).

use gear_engine::ops::confirm_pending;
use gear_model::{OpKind, OpState, SeatRole};
use gear_store::{testing, NewOp, Store, Tx, ViewRecord};

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
        .write(|tx| Ok(submitted_op_within(tx, covers, created)))
        .unwrap()
}

/// The same, inside an existing transaction (so a test can move it further).
fn submitted_op_within(tx: &Tx<'_>, covers: &[&str], created: i64) -> String {
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
    tx.set_op_state("p", &id, OpState::Executing, None, created)
        .unwrap();
    tx.set_op_state("p", &id, OpState::Submitted, None, created)
        .unwrap();
    id
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
    assert_eq!(
        op(&mut store, &id).check_budget,
        0,
        "a confirmed op needs no further checks"
    );
}

/// L-69: any view after the doorbell answers it, even one that did not list
/// every covered id (the inbox batch caps what one view shows).
#[test]
fn a_partial_view_confirms() {
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-1", "M-2"], T0 + 1);
    view(&mut store, 1, T0 + 5, &["M-1"]);

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 6).unwrap();

    assert_eq!(confirmed, vec![id.clone()]);
    assert_eq!(op(&mut store, &id).state, OpState::Confirmed);
}

#[test]
fn older_view_does_not_confirm() {
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-1"], T0 + 1);
    // The seat looked at its home page *before* the doorbell was intended.
    view(&mut store, 1, T0, &["M-1"]);

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 20).unwrap();

    assert!(
        confirmed.is_empty(),
        "a view from before the intent proves nothing"
    );
    assert_eq!(op(&mut store, &id).state, OpState::Submitted);
    assert_eq!(op(&mut store, &id).check_budget, 1);
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
fn view_at_creation_confirms() {
    // The boundary is inclusive: a view at exactly the op's created time counts.
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-1"], T0 + 1);
    view(&mut store, 1, T0 + 1, &["M-1"]);

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 20).unwrap();

    assert_eq!(confirmed, vec![id.clone()]);
    assert_eq!(op(&mut store, &id).state, OpState::Confirmed);
}

#[test]
fn unknown_op_confirms() {
    let mut store = store_with_project();
    let id = store
        .write(|tx| {
            // An op whose Herdr call was Uncertain waits in Unknown.
            let id = submitted_op_within(tx, &["M-1"], T0 + 1);
            tx.set_op_state("p", &id, OpState::Unknown, Some("deadline"), T0 + 2)
                .unwrap();
            Ok(id)
        })
        .unwrap();
    assert_eq!(op(&mut store, &id).state, OpState::Unknown);
    view(&mut store, 1, T0 + 5, &["M-1"]);

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 20).unwrap();

    assert_eq!(confirmed, vec![id.clone()]);
    assert_eq!(op(&mut store, &id).state, OpState::Confirmed);
}

#[test]
fn confirmation_records_evidence() {
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-1"], T0 + 1);
    view(&mut store, 1, T0 + 5, &["M-1"]);

    confirm_pending(&mut store, "p", 3, T0 + 20).unwrap();

    let stored = op(&mut store, &id);
    assert_eq!(stored.state, OpState::Confirmed);
    assert_eq!(
        stored.result.as_deref(),
        Some("view p:captain/1 @ 1800000005"),
        "the op keeps a pointer to the view that settled it"
    );
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
    assert!(
        flagged.is_empty(),
        "the seat is flagged once, not on every check"
    );
}

#[test]
fn no_resend_on_exhaustion() {
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-1"], T0 + 1);

    let (confirmed, flagged) = confirm_pending(&mut store, "p", 1, T0 + 6).unwrap();
    assert!(confirmed.is_empty());
    assert_eq!(
        flagged,
        vec![SEAT.to_string()],
        "the budget of one check is used up"
    );

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

/// A message to the captain in `state`; returns its id.
fn message(store: &mut Store, state: gear_model::MsgState) -> String {
    store
        .write(|tx| {
            Ok(tx
                .insert_message(&gear_store::NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "t".into(),
                    task: None,
                    attempt: None,
                    sender: "p:engine".into(),
                    sender_gen: 1,
                    recipient: SEAT.into(),
                    kind: gear_model::MsgKind::Question,
                    urgent: false,
                    body: "b".into(),
                    reply_to: None,
                    needs_ack: false,
                    state,
                    created: T0,
                })?
                .id)
        })
        .unwrap()
}

/// L-66: a doorbell whose items were all closed another way settles — else it
/// holds the captain's slot and no doorbell ever rings again.
#[test]
fn a_doorbell_whose_items_are_closed_settles() {
    let mut store = store_with_project();
    let closed = message(&mut store, gear_model::MsgState::Resolved);
    let id = submitted_op(&mut store, &[closed.as_str()], T0 + 1);

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 6).unwrap();

    assert_eq!(confirmed, vec![id.clone()]);
    assert_eq!(op(&mut store, &id).state, OpState::Confirmed);
}

#[test]
fn a_doorbell_with_an_open_item_waits_for_a_view() {
    let mut store = store_with_project();
    let closed = message(&mut store, gear_model::MsgState::Resolved);
    let open = message(&mut store, gear_model::MsgState::Open);
    let id = submitted_op(&mut store, &[closed.as_str(), open.as_str()], T0 + 1);

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 6).unwrap();

    assert!(confirmed.is_empty());
    assert_eq!(op(&mut store, &id).state, OpState::Submitted);
}

#[test]
fn a_view_of_the_still_open_items_confirms_a_doorbell() {
    let mut store = store_with_project();
    let closed = message(&mut store, gear_model::MsgState::Resolved);
    let open = message(&mut store, gear_model::MsgState::Open);
    let id = submitted_op(&mut store, &[closed.as_str(), open.as_str()], T0 + 1);
    // The closed item is no longer shown; the open one is.
    view(&mut store, 1, T0 + 5, &[open.as_str()]);

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 6).unwrap();

    assert_eq!(confirmed, vec![id]);
}

/// L-69 liveness: a doorbell nobody answered expires after
/// DOORBELL_EXPIRY_S, so it can never hold the captain's slot for good.
#[test]
fn an_unanswered_doorbell_expires() {
    let mut store = store_with_project();
    let id = submitted_op(&mut store, &["M-404"], T0);

    let (confirmed, _) = confirm_pending(&mut store, "p", 1000, T0 + 60).unwrap();
    assert!(confirmed.is_empty(), "still within its time");

    let (confirmed, _) = confirm_pending(
        &mut store,
        "p",
        1000,
        T0 + gear_engine::ops::DOORBELL_EXPIRY_S,
    )
    .unwrap();
    assert_eq!(confirmed, vec![id.clone()]);
    assert_eq!(op(&mut store, &id).state, OpState::Confirmed);
}
