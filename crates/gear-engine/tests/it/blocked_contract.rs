//! T-57: a seat blocked by a dialog for minutes reaches the captain.
//!
//! On 2026-10-25 a Codex reviewer sat on its start-up "Update available"
//! dialog: Herdr reported the agent `blocked` and nobody was told (lesson
//! 7b). A lane or reviewer seat that stays blocked for more than two minutes
//! gets one item in its captain's inbox, with the last screen lines.

use gear_engine::{blocked, observe, tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{MsgKind, MsgState, SeatRole};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const LANE: &str = "w1:p2";
const TEMP: &str = "w1:p3";

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
            testing::seed_active_seat(tx, "p", "rev-1", SeatRole::Temp, TEMP, "codex", T0)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE, "codex", AgentStatus::Blocked);
    herdr.add_agent(TEMP, "codex", AgentStatus::Idle);
    herdr.set_screen(
        LANE,
        "Update available\ncodex 0.164.0 is available\nesc to dismiss",
    );
    let mut cfg = EngineCfg::default();
    cfg.profiles
        .insert("codex".into(), gear_herdr::builtin("codex").unwrap());
    World { store, herdr, cfg }
}

fn world() -> World {
    world_in(Store::open_in_memory().unwrap())
}

fn run(w: &mut World, seconds: i64) -> gear_engine::TickReport {
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + seconds).unwrap()
}

/// Observe and run only this watch, so the test sees the blocked seats alone.
fn watch(w: &mut World, seconds: i64) -> Vec<String> {
    let agents = observe::snapshot(&mut w.herdr).unwrap();
    let observed = w
        .store
        .read(|tx| observe::reconcile(tx, &mut w.herdr, "p", &agents, &w.cfg.profiles))
        .unwrap();
    w.store
        .write(|tx| blocked::check(tx, "p", &observed, T0 + seconds))
        .unwrap()
}

fn items(w: &mut World, seat: &str) -> Vec<gear_store::MessageRow> {
    w.store
        .read(|tx| tx.thread_messages("p", &format!("blocked:p:{seat}")))
        .unwrap()
}

#[test]
fn a_seat_blocked_for_more_than_two_minutes_rings_the_captain_once() {
    let mut w = world();
    assert!(watch(&mut w, 0).is_empty(), "the timer starts");
    assert!(watch(&mut w, 119).is_empty());
    assert!(
        watch(&mut w, 120).is_empty(),
        "strictly more than two minutes"
    );
    assert_eq!(watch(&mut w, 121), vec!["p:heat"]);

    let messages = items(&mut w, "heat");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].recipient, "p:captain");
    assert_eq!(messages[0].kind, MsgKind::System);
    assert_eq!(messages[0].state, MsgState::Open);
    assert!(messages[0].urgent);
    assert!(messages[0].body.contains("blocked"), "{}", messages[0].body);
    assert!(
        messages[0].body.contains("Update available"),
        "the last screen lines are in the item: {}",
        messages[0].body
    );

    // The same blocked spell does not ring again.
    assert!(watch(&mut w, 200).is_empty());
    assert!(watch(&mut w, 600).is_empty());
    assert_eq!(items(&mut w, "heat").len(), 1);
}

#[test]
fn the_engine_tick_rings_the_captain_for_a_blocked_seat() {
    let mut w = world();
    assert!(run(&mut w, 0).blocked.is_empty(), "the timer starts");
    assert!(
        run(&mut w, 120).blocked.is_empty(),
        "strictly more than two minutes"
    );
    let report = run(&mut w, 121);
    assert_eq!(report.blocked, vec!["p:heat"]);
    assert_eq!(items(&mut w, "heat").len(), 1);
    assert!(
        !w.herdr.prompts_to(CAP).is_empty(),
        "the captain is rung for the blocked seat"
    );
}

#[test]
fn a_reviewer_seat_blocked_for_minutes_rings_the_captain_too() {
    let mut w = world();
    w.herdr.set_status(LANE, AgentStatus::Idle);
    w.herdr.set_status(TEMP, AgentStatus::Blocked);
    assert!(watch(&mut w, 0).is_empty());
    assert_eq!(watch(&mut w, 121), vec!["p:rev-1"]);
    assert_eq!(items(&mut w, "rev-1").len(), 1);
    assert_eq!(
        items(&mut w, "heat").len(),
        0,
        "the other seat is not blocked"
    );
}

#[test]
fn a_seat_that_works_again_resolves_the_item_and_a_later_spell_rings_again() {
    let mut w = world();
    watch(&mut w, 0);
    assert_eq!(watch(&mut w, 121).len(), 1);

    w.herdr.set_status(LANE, AgentStatus::Idle);
    assert!(watch(&mut w, 122).is_empty());
    assert_eq!(items(&mut w, "heat")[0].state, MsgState::Resolved);

    w.herdr.set_status(LANE, AgentStatus::Blocked);
    assert!(watch(&mut w, 123).is_empty(), "the timer starts again");
    assert!(watch(&mut w, 243).is_empty());
    assert_eq!(watch(&mut w, 244).len(), 1);
    assert_eq!(items(&mut w, "heat").len(), 2, "a new spell is reported");
}

#[test]
fn the_timer_and_the_alert_survive_reopening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    let mut w = world_in(Store::open(&path).unwrap());
    run(&mut w, 0);
    w.store = Store::open(&path).unwrap();
    assert_eq!(run(&mut w, 121).blocked, vec!["p:heat"]);
    w.store = Store::open(&path).unwrap();
    assert!(
        run(&mut w, 300).blocked.is_empty(),
        "the same spell does not ring twice"
    );
    assert_eq!(items(&mut w, "heat").len(), 1);
}

#[test]
fn a_seat_that_is_never_blocked_never_rings_the_captain() {
    let mut w = world();
    w.herdr.set_status(LANE, AgentStatus::Idle);
    assert!(watch(&mut w, 0).is_empty());
    assert!(watch(&mut w, 600).is_empty());
    assert!(items(&mut w, "heat").is_empty());
}
