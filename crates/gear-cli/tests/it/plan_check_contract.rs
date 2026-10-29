//! T-190, T-216: `gear plan check` ends the parks whose time has come, and
//! only for a caller it can prove: the captain, at the generation the store
//! holds (T-216, F2).
use std::path::PathBuf;
use std::process::Command;

use gear_model::{SeatRole, SeatState};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;

/// A scratch directory for the store and the home of the child.
fn scratch(tag: &str) -> PathBuf {
    crate::scratch::temp(&format!("gear-check-{tag}"))
}

/// The store of a project whose captain sits in `w1:p1` and whose lane `heat`
/// is parked until a time that has passed.
fn world(tag: &str) -> (PathBuf, PathBuf) {
    let dir = scratch(tag);
    let db = dir.join("gear.db");
    let mut store = Store::open(&db).expect("the store");
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            // A lane parked until a time that has passed.
            tx.set_seat_state(
                "p:heat",
                SeatState::Parked,
                Some("until=2020-01-01T00:00:00Z"),
                T0,
            )
        })
        .expect("the world");
    drop(store);
    (dir, db)
}

#[test]
fn the_check_command_refuses_a_caller_it_cannot_prove_and_writes_nothing() {
    let (dir, db) = world("unproved");

    let out = Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["plan", "check"])
        .env("HOME", &dir)
        .env("GEAR_DB", &db)
        .env("GEAR_CTX", "p:captain:1")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH")
        .output()
        .expect("run gear");

    // The captain's identity cannot be proved (no session, no pane), so the
    // command is refused: it ends no park (T-216, F2).
    assert!(
        !out.status.success(),
        "the command is refused: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let mut store = Store::open(&db).expect("the store");
    let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Parked, "the park stays");
}

#[test]
fn the_check_command_refuses_an_unknown_captain_and_writes_nothing() {
    let (dir, db) = world("unknown");

    let out = Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["plan", "check"])
        .env("HOME", &dir)
        .env("GEAR_DB", &db)
        // A seat the store does not hold.
        .env("GEAR_CTX", "p:ghost:1")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH")
        .output()
        .expect("run gear");

    assert!(
        !out.status.success(),
        "the command is refused: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let mut store = Store::open(&db).expect("the store");
    let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Parked, "the park stays");
}
