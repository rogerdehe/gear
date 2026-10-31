//! Contract for gear_app::seats onboarding transitions (T-59).

use gear_app::seats::{begin_onboard, finish_onboard};
use gear_model::{ErrorCode, SeatRole, SeatState};
use gear_store::{testing, BindingStatus, SeatRow, Store};

const T0: i64 = 1_800_000_000;

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        tx.insert_seat(&SeatRow {
            id: "p:heat".into(),
            project: "p".into(),
            role: SeatRole::Lane,
            name: "heat".into(),
            profile: "opencode".into(),
            generation: 0,
            state: SeatState::Queued,
            state_since: T0,
            state_reason: Some("registered".into()),
            task_id: None,
        })
    })
    .unwrap();
    s
}

#[test]
fn onboard_from_queued_to_active() {
    let mut s = world();
    let gen = s
        .write(|tx| begin_onboard(tx, "p:heat", "w3:p1", "opencode", T0 + 1))
        .unwrap();
    assert_eq!(gen, 1);
    let seat = s.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    assert_eq!((seat.state, seat.generation), (SeatState::Starting, 1));
    let b = s.read(|tx| tx.get_binding("p:heat", 1)).unwrap().unwrap();
    assert_eq!(
        (
            b.status,
            b.pane_id.as_str(),
            b.kind.as_str(),
            b.session.as_str()
        ),
        (BindingStatus::Pending, "w3:p1", "opencode", "sess")
    );
    s.write(|tx| finish_onboard(tx, "p:heat", 1, Some("ses_x"), T0 + 2))
        .unwrap();
    assert_eq!(
        s.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap().state,
        SeatState::Active
    );
    let b = s.read(|tx| tx.get_binding("p:heat", 1)).unwrap().unwrap();
    assert_eq!(
        (b.status, b.session_ref.as_deref()),
        (BindingStatus::Active, Some("ses_x"))
    );
}

#[test]
fn restart_voids_the_old_binding_and_stale_finish_is_refused() {
    let mut s = world();
    s.write(|tx| {
        begin_onboard(tx, "p:heat", "w3:p1", "opencode", T0)?;
        finish_onboard(tx, "p:heat", 1, None, T0)
    })
    .unwrap();
    s.write(|tx| tx.set_seat_state("p:heat", SeatState::Missing, Some("pane gone"), T0 + 5))
        .unwrap();
    let gen = s
        .write(|tx| begin_onboard(tx, "p:heat", "w3:p2", "opencode", T0 + 6))
        .unwrap();
    assert_eq!(gen, 2);
    assert_eq!(
        s.read(|tx| tx.get_binding("p:heat", 1))
            .unwrap()
            .unwrap()
            .status,
        BindingStatus::Void
    );
    assert_eq!(
        s.write(|tx| finish_onboard(tx, "p:heat", 1, None, T0 + 7))
            .unwrap_err()
            .code,
        ErrorCode::StaleGeneration
    );
    s.write(|tx| finish_onboard(tx, "p:heat", 2, None, T0 + 8))
        .unwrap();
    assert_eq!(
        s.write(|tx| finish_onboard(tx, "p:heat", 2, None, T0 + 9))
            .unwrap_err()
            .code,
        ErrorCode::BadTransition,
        "already active"
    );
    assert_eq!(
        s.write(|tx| begin_onboard(tx, "p:nope", "w", "opencode", T0))
            .unwrap_err()
            .code,
        ErrorCode::NoTarget
    );
}
