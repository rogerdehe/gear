//! T-190: `gear plan check` ends the parks whose time has come.
use std::path::PathBuf;
use std::process::Command;

use gear_model::{SeatRole, SeatState};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;

/// A scratch directory for the store and the home of the child.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gear-check-{}-{tag}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

#[test]
fn the_check_command_exists_and_is_honest_about_the_session() {
    let dir = scratch("check");
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
                Some("until=2027-01-15T07:00:00Z"),
                T0,
            )
        })
        .expect("the world");
    drop(store);

    let out = Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["plan", "check"])
        .env("HOME", &dir)
        .env("GEAR_DB", &db)
        .env("GEAR_CTX", "p:captain:1")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH")
        .output()
        .expect("run gear");

    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the command exists: {text} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Without the session's agents nothing is guessed: the park stays and the
    // command says so (fail-closed, T-190).
    assert!(text.contains("agents could not be read"), "{text}");
    let mut store = Store::open(&db).expect("the store");
    let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Parked, "the park stays");
}
