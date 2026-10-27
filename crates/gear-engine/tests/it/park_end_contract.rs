//! T-190: a plan park ends by itself: the tick ends the parks whose time has
//! come, so a lane the captain parked until a time that has passed works
//! again.
use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{SeatRole, SeatState};
use gear_store::{testing, Store};

const T: i64 = 1_800_000_000;
const LANE: &str = "w1:p2";

/// A project with one lane whose agent is in its pane.
fn world() -> (Store, FakeHerdr, EngineCfg) {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T)?;
        Ok(())
    })
    .unwrap();
    let mut h = FakeHerdr::new();
    h.add_agent("w1:p1", "codex", AgentStatus::Idle);
    h.add_agent(LANE, "opencode", AgentStatus::Idle);
    let mut cfg = EngineCfg::default();
    cfg.profiles = gear_herdr::builtins();
    cfg.cooldown_s = 0;
    (s, h, cfg)
}

/// Park the lane with the reason the plan writes.
fn park(s: &mut Store, reason: &str) {
    s.write(|tx| tx.set_seat_state("p:heat", SeatState::Parked, Some(reason), T))
        .unwrap();
}

/// The state of the lane.
fn seat(s: &mut Store) -> (SeatState, Option<String>) {
    let row = s.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    (row.state, row.state_reason)
}

#[test]
fn the_tick_ends_a_park_whose_time_has_come() {
    let (mut s, mut h, c) = world();
    // T0 is 2027-01-15T08:00:00Z: the park ended an hour ago.
    park(&mut s, "until=2027-01-15T07:00:00Z");

    let report = tick(&mut s, &mut h, &c, T + 10).unwrap();

    assert_eq!(seat(&mut s).0, SeatState::Active, "the lane works again");
    assert_eq!(seat(&mut s).1, Some("park ended".to_string()));
    assert!(
        report.park_ended.iter().any(|line| line.contains("heat")),
        "{:?}",
        report.park_ended
    );
}

#[test]
fn the_tick_leaves_a_park_whose_time_has_not_come() {
    let (mut s, mut h, c) = world();
    park(&mut s, "until=2027-01-15T09:00:00Z");

    tick(&mut s, &mut h, &c, T + 10).unwrap();

    assert_eq!(seat(&mut s).0, SeatState::Parked, "the park lasts");
}

#[test]
fn the_tick_leaves_a_quota_park_to_the_quota_path() {
    let (mut s, mut h, c) = world();
    park(&mut s, "quota: minimax-cn 5h; resume at 1800003600");
    // The engine's quota park carries the resume deadline.
    s.write(|tx| tx.set_meta("resume:p:heat", "1800003600"))
        .unwrap();

    tick(&mut s, &mut h, &c, T + 10).unwrap();

    assert_eq!(seat(&mut s).0, SeatState::Parked, "the quota path owns it");
}
