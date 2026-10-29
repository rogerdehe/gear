//! T-233: "deliveries not confirmed" counts only the live deliveries.
//!
//! A Deliver op that is not settled counts only while the seat it names is
//! still the live incumbent: an op of a retired seat, or of an older
//! generation of a live one, is history and must not look like a lost
//! message.

use gear_board::{build, Options};
use gear_model::{OpKind, OpState, SeatRole};
use gear_store::{testing, NewOp, OpRow, Store};

const T0: i64 = 1_800_000_000;

/// A project with a captain, a live lane `data` (generation 1) and a lane
/// `old` that is retired.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                "w1:p1",
                "claude",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            testing::seed_active_seat(tx, "p", "old", SeatRole::Lane, "w1:p3", "opencode", T0)?;
            tx.set_seat_state("p:old", gear_model::SeatState::Retired, None, T0)?;
            Ok(())
        })
        .unwrap();
    store
}

/// One Deliver op of `seat` at `generation`, left in `state`.
fn seed_delivery(store: &mut Store, seat: &str, generation: i64, state: OpState) -> OpRow {
    store
        .write(|tx| {
            let op = tx.insert_op(&NewOp {
                project: "p".into(),
                seat: seat.to_string(),
                generation,
                kind: OpKind::Deliver,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({}),
                created: T0 - 60,
            })?;
            tx.set_op_state("p", &op.id, state, None, T0 - 30)?;
            tx.get_op("p", &op.id)?
                .ok_or_else(|| gear_model::GearError::new(gear_model::ErrorCode::NoTarget, "op"))
        })
        .unwrap()
}

fn options() -> Options {
    Options {
        project: Some("p".into()),
        ..Options::default()
    }
}

#[test]
fn only_a_live_delivery_is_unconfirmed() {
    let mut store = world();
    // The retired seat's op, though unsettled, is history.
    seed_delivery(&mut store, "p:old", 1, OpState::Unknown);
    // The lane restarted: generation 2 is live, so its old op is history too.
    store
        .write(|tx| tx.set_seat_generation("p:data", 2))
        .unwrap();
    seed_delivery(&mut store, "p:data", 1, OpState::Unknown);
    // The live generation's op counts.
    seed_delivery(&mut store, "p:data", 2, OpState::Unknown);

    let board = build(&mut store, &options(), T0).unwrap();
    assert_eq!(board.projects[0].unconfirmed_deliveries, 1);
}

#[test]
fn a_settled_or_non_deliver_op_does_not_count() {
    let mut store = world();
    // Confirmed: settled, so it is not in flight.
    seed_delivery(&mut store, "p:data", 1, OpState::Confirmed);
    // Another kind of op in flight is not a delivery.
    store
        .write(|tx| {
            let op = tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:data".into(),
                generation: 1,
                kind: OpKind::Doorbell,
                task: None,
                attempt: None,
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({}),
                created: T0 - 60,
            })?;
            tx.set_op_state("p", &op.id, OpState::Unknown, None, T0 - 30)?;
            Ok(())
        })
        .unwrap();

    let board = build(&mut store, &options(), T0).unwrap();
    assert_eq!(board.projects[0].unconfirmed_deliveries, 0);
}

/// One Deliver op of `seat` at `generation`, left in `state`, created at
/// `created`.
fn seed_delivery_at(
    store: &mut Store,
    seat: &str,
    generation: i64,
    state: OpState,
    created: i64,
) -> OpRow {
    store
        .write(|tx| {
            let op = tx.insert_op(&NewOp {
                project: "p".into(),
                seat: seat.to_string(),
                generation,
                kind: OpKind::Deliver,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({}),
                created,
            })?;
            tx.set_op_state("p", &op.id, state, None, created + 1)?;
            tx.get_op("p", &op.id)?
                .ok_or_else(|| gear_model::GearError::new(gear_model::ErrorCode::NoTarget, "op"))
        })
        .unwrap()
}

/// T-271: the board names the deliveries it counts — the live ones only, the
/// oldest first, with their ages and the seat's name.
#[test]
fn the_unconfirmed_deliveries_are_named_oldest_first() {
    let mut store = world();
    // A retired seat's delivery is history, never named.
    let retired = seed_delivery_at(&mut store, "p:old", 1, OpState::Submitted, T0 - 900);
    // Two live deliveries: the older one comes first.
    let older = seed_delivery_at(&mut store, "p:data", 1, OpState::Submitted, T0 - 300);
    let newer = seed_delivery_at(&mut store, "p:data", 1, OpState::Submitted, T0 - 120);

    let board = build(&mut store, &options(), T0).unwrap();
    let project = &board.projects[0];
    assert_eq!(
        project.unconfirmed_deliveries, 2,
        "the count stays as it is"
    );
    assert_eq!(
        project.unconfirmed.len(),
        2,
        "only the live deliveries are named"
    );
    assert_eq!(project.unconfirmed[0].op, older.id);
    assert_eq!(
        project.unconfirmed[0].seat, "data",
        "the seat's name, not its id"
    );
    assert_eq!(project.unconfirmed[0].task.as_deref(), Some("T-1"));
    assert_eq!(project.unconfirmed[0].age_s, 300);
    assert_eq!(project.unconfirmed[1].op, newer.id);
    assert_eq!(project.unconfirmed[1].age_s, 120);
    assert!(
        !project.unconfirmed.iter().any(|item| item.op == retired.id),
        "a retired seat's delivery is history"
    );
}
