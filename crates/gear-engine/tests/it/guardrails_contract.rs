//! T-113: a lane refused by its own guardrails reaches the captain.
//!
//! Pi runs `@aliou/pi-guardrails`: the `permission-gate` extension refuses a
//! dangerous command (`Command auto-denied: …`) and the `path-access`
//! extension refuses a write outside the workspace (`Access to <path> is
//! blocked (outside working directory).`). The agent goes on with the
//! refusal as a tool result, so nothing about the seat's state says it was
//! told to do something it may not do. The profile carries the two deny
//! lines as markers; the engine reads them off the screen and tells the
//! captain once per spell.

use gear_engine::{guardrails, observe, tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{MsgKind, MsgState, SeatRole};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const LANE: &str = "w1:p2";
const TEMP: &str = "w1:p3";

/// The screen pi shows after its guardrails refused a command (the live text,
/// pi 1.4.0 on this machine).
const REFUSED: &str = "\
 $ sudo -n systemctl status (timeout 15s)

 Blocked before execution:

   Command auto-denied: No privileged commands in an unattended lane.

 Took 0.0s

 LLM Inactive";

/// The screen of a lane that was not refused.
const CLEAN: &str = "\
 $ gear --version (timeout 30s)

 gear 0.3.1

 Took 0.0s

 LLM Inactive";

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
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "pi", T0)?;
            testing::seed_active_seat(tx, "p", "rev-1", SeatRole::Temp, TEMP, "pi", T0)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE, "pi", AgentStatus::Idle);
    herdr.add_agent(TEMP, "pi", AgentStatus::Idle);
    herdr.set_screen(LANE, REFUSED);
    herdr.set_screen(TEMP, CLEAN);
    let mut cfg = EngineCfg::default();
    cfg.profiles
        .insert("pi".into(), gear_herdr::builtin("pi").unwrap());
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

/// Observe and run only this watch, so the test sees the refusals alone.
fn watch(w: &mut World, seconds: i64) -> Vec<String> {
    let agents = observe::snapshot(&mut w.herdr).unwrap();
    let observed = w
        .store
        .read(|tx| observe::reconcile(tx, &mut w.herdr, "p", &agents, &w.cfg.profiles))
        .unwrap();
    w.store
        .write(|tx| guardrails::check(tx, "p", &observed, T0 + seconds))
        .unwrap()
}

fn items(w: &mut World, seat: &str) -> Vec<gear_store::MessageRow> {
    w.store
        .read(|tx| tx.thread_messages("p", &format!("guardrails:p:{seat}")))
        .unwrap()
}

#[test]
fn a_lane_refused_by_its_guardrails_rings_the_captain_once() {
    let mut w = world();
    assert_eq!(watch(&mut w, 0), vec!["p:heat"]);

    let messages = items(&mut w, "heat");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].recipient, "p:captain");
    assert_eq!(messages[0].kind, MsgKind::System);
    assert_eq!(messages[0].state, MsgState::Open);
    assert!(
        messages[0].body.contains("guardrails"),
        "{}",
        messages[0].body
    );
    assert!(
        messages[0].body.contains("Command auto-denied"),
        "the last screen lines are in the item: {}",
        messages[0].body
    );

    // The same spell does not ring again.
    assert!(watch(&mut w, 200).is_empty());
    assert_eq!(items(&mut w, "heat").len(), 1);
    // The reviewer seat was not refused: no item for it.
    assert!(items(&mut w, "rev-1").is_empty());
}

#[test]
fn the_engine_tick_rings_the_captain_for_a_refused_lane() {
    let mut w = world();
    let report = run(&mut w, 0);
    assert_eq!(report.guardrails, vec!["p:heat"]);
    assert_eq!(items(&mut w, "heat").len(), 1);
    // The doorbell that carries the item to the captain takes a few ticks.
    run(&mut w, 1);
    run(&mut w, 2);
    assert!(
        !w.herdr.prompts_to(CAP).is_empty(),
        "the captain is rung for the refused lane"
    );
}

#[test]
fn a_lane_that_is_not_refused_never_rings_the_captain() {
    let mut w = world();
    w.herdr.set_screen(LANE, CLEAN);
    assert!(watch(&mut w, 0).is_empty());
    assert!(watch(&mut w, 600).is_empty());
    assert!(items(&mut w, "heat").is_empty());
}

/// The refusal is read off the screen of an idle agent. A working agent's
/// screen is not read (only a provider back-off is), so a fact that was not
/// looked at is not a fact that went away.
#[test]
fn a_screen_that_was_not_read_does_not_resolve_the_item() {
    let mut w = world();
    assert_eq!(watch(&mut w, 0), vec!["p:heat"]);
    // The agent works: its screen is not read, so the item stays open.
    w.herdr.set_status(LANE, AgentStatus::Working);
    assert!(watch(&mut w, 1).is_empty());
    assert_eq!(items(&mut w, "heat")[0].state, MsgState::Open);
}

#[test]
fn a_lane_that_is_refused_no_more_resolves_the_item_and_a_later_refusal_rings_again() {
    let mut w = world();
    assert_eq!(watch(&mut w, 0), vec!["p:heat"]);

    w.herdr.set_screen(LANE, CLEAN);
    assert!(watch(&mut w, 1).is_empty());
    assert_eq!(items(&mut w, "heat")[0].state, MsgState::Resolved);

    w.herdr.set_screen(LANE, REFUSED);
    assert_eq!(watch(&mut w, 2), vec!["p:heat"]);
    assert_eq!(items(&mut w, "heat").len(), 2, "a new refusal is reported");
}
