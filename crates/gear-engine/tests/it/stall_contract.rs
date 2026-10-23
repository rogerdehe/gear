//! Contract (L-52): a flagged lane reaches the captain's inbox, once.

use gear_engine::stall::alert;
use gear_model::{MsgState, SeatRole};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "claude", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        Ok(())
    })
    .unwrap();
    s
}

fn captain_items(s: &mut Store) -> Vec<(String, MsgState)> {
    s.read(|tx| tx.thread_messages("p", "stall:p:heat"))
        .unwrap()
        .into_iter()
        .map(|m| (m.recipient, m.state))
        .collect()
}

#[test]
fn a_flagged_lane_opens_one_captain_item() {
    let mut s = world();
    assert!(s.write(|tx| alert(tx, "p:heat", T0 + 10)).unwrap());
    // Flagged again while the item is open: nothing new.
    assert!(!s.write(|tx| alert(tx, "p:heat", T0 + 20)).unwrap());
    assert_eq!(captain_items(&mut s), vec![("p:captain".to_string(), MsgState::Open)]);
}

#[test]
fn the_captain_itself_is_not_alerted() {
    let mut s = world();
    assert!(!s.write(|tx| alert(tx, "p:captain", T0)).unwrap());
}
