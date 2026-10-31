//! T-75: a provider back-off is not a stall (trial G11).
//!
//! When Codex retries a 429 it shows `Reconnecting... {n}/{max}` as its
//! status header (its own source: `codex-rs/core/src/responses_retry.rs`) and
//! keeps working with no output for minutes. GEAR raised stall alerts for
//! that. Now the engine records the wait, the stall check leaves the seat
//! alone and the home page shows "provider back-off".

use gear_engine::{observe, stall, tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::SeatRole;
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const LANE: &str = "w1:p2";

/// The screen Codex shows while it retries a rate-limited request: the
/// status header is `Reconnecting... {retry}/{max_retries}` and the status
/// details carry the provider's error.
const RETRYING: &str = "• Worked for 2m 03s\n\
• Working (2m 12s • esc to interrupt)\n\
• Reconnecting... 3/5 (2m 12s • esc to interrupt)\n\
  └ Rate limit exceeded: Rate limit reached for gpt-5.1 in organization org-test. Please try again in 11.054s.";

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

fn world_in(mut store: Store) -> World {
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "codex", T0)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE, "codex", AgentStatus::Working);
    herdr.set_screen(LANE, RETRYING);
    let mut cfg = EngineCfg::default();
    cfg.profiles
        .insert("codex".into(), gear_herdr::builtin("codex").unwrap());
    World { store, herdr, cfg }
}

fn world() -> World {
    world_in(Store::open_in_memory().unwrap())
}

/// The stall thread of the lane: the item the captain would be rung with.
fn stall_items(w: &mut World) -> Vec<gear_store::MessageRow> {
    w.store
        .read(|tx| tx.thread_messages("p", "stall:p:heat"))
        .unwrap()
}

#[test]
fn a_working_codex_seat_in_a_back_off_is_recorded_and_shown() {
    let mut w = world();

    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    // The wait is recorded, so the rest of GEAR can see it (the captain's
    // home page shows it — `backoff_home_contract` in gear-app).
    assert_eq!(report.backoff, vec!["p:heat".to_string()], "{report:?}");
    assert_eq!(
        w.store.read(|tx| tx.seats_in_backoff("p")).unwrap(),
        vec!["p:heat".to_string()]
    );

    // And a flagged seat that waits for its provider raises no stall item.
    let opened = w
        .store
        .write(|tx| stall::alert(tx, "p:heat", T0 + 12))
        .unwrap();
    assert!(!opened, "a back-off is not a stall");
    assert!(stall_items(&mut w).is_empty(), "{:?}", stall_items(&mut w));
}

#[test]
fn ordinary_output_is_not_a_back_off() {
    let mut w = world();
    w.herdr.set_screen(
        LANE,
        "• Working (12s • esc to interrupt)\n• Ran cargo test --workspace\n• All tests passed",
    );

    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    assert!(report.backoff.is_empty(), "{report:?}");
    assert!(w
        .store
        .read(|tx| tx.seats_in_backoff("p"))
        .unwrap()
        .is_empty());
    // A flagged seat that is not waiting rings the captain as before.
    assert!(w
        .store
        .write(|tx| stall::alert(tx, "p:heat", T0 + 12))
        .unwrap());
    assert_eq!(stall_items(&mut w).len(), 1);
}

#[test]
fn the_back_off_ends_when_the_screen_has_moved_on() {
    let mut w = world();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    assert_eq!(
        w.store.read(|tx| tx.seats_in_backoff("p")).unwrap(),
        vec!["p:heat".to_string()]
    );

    // The provider answered: the status header is back to the ordinary work
    // line, and the next tick clears the wait.
    w.herdr.set_screen(
        LANE,
        "• Working (3m 01s • esc to interrupt)\n• Ran cargo build",
    );
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();

    assert!(report.backoff.is_empty(), "{report:?}");
    assert!(w
        .store
        .read(|tx| tx.seats_in_backoff("p"))
        .unwrap()
        .is_empty());
}

#[test]
fn the_wait_survives_reopening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    let mut w = world_in(Store::open(&path).unwrap());
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    w.store = Store::open(&path).unwrap();

    assert_eq!(
        w.store.read(|tx| tx.seats_in_backoff("p")).unwrap(),
        vec!["p:heat".to_string()]
    );
    assert!(!w
        .store
        .write(|tx| stall::alert(tx, "p:heat", T0 + 20))
        .unwrap());
    assert!(stall_items(&mut w).is_empty());
}

#[test]
fn the_observation_carries_the_back_off_and_the_seat_is_found() {
    let mut w = world();
    let agents = observe::snapshot(&mut w.herdr).unwrap();
    let observed = w
        .store
        .read(|tx| observe::reconcile(tx, &mut w.herdr, "p", &agents, &w.cfg.profiles))
        .unwrap();

    let heat = observed.iter().find(|obs| obs.seat.id == "p:heat").unwrap();
    assert!(heat
        .screen
        .as_deref()
        .unwrap_or_default()
        .contains("Reconnecting..."));
    assert_eq!(
        stall::waiting_for_provider(&w.cfg.profiles, &observed),
        vec!["p:heat".to_string()]
    );
    // A working seat is never idle, back-off or not.
    assert!(!heat.idle);
}
