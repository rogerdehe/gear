//! T-169 (design §6.4): the friction ring. The captain hears about the open
//! friction reports when it has nothing else to do — one item on the thread
//! `friction`, at most once per 30 minutes, updated by a new report and
//! closed when every report it lists is closed.

use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{MsgKind, MsgState, SeatRole};
use gear_store::{testing, NewFriction, NewMessage, Store};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

/// A project with a captain and nothing else to do: the captain's inbox is
/// empty, so it is idle (design §6.4).
fn world() -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "claude", T0)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "claude", AgentStatus::Idle);
    let mut cfg = EngineCfg { tick_s: 4, cooldown_s: 30, doorbell_check_budget: 3, ..Default::default() };
    cfg.profiles.insert("claude".to_string(), gear_herdr::builtin("claude").unwrap());
    World { store, herdr, cfg }
}

/// A friction report of a lane (design §6.1).
fn report(store: &mut Store, task: &str, body: &str) -> String {
    store
        .write(|tx| {
            tx.insert_friction(&NewFriction {
                project: "p".into(),
                task: task.into(),
                attempt: 1,
                role: "dev".into(),
                seat: "p:heat".into(),
                body: body.into(),
                required: false,
            })
        })
        .unwrap()
}

/// The captain's friction items, oldest first.
fn items(store: &mut Store) -> Vec<gear_store::MessageRow> {
    store.read(|tx| tx.thread_messages("p", "friction")).unwrap()
}

/// An open item in the captain's inbox: the captain has something to do.
fn captain_item(store: &mut Store, now: i64) {
    store
        .write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: "T-1:1".into(),
                task: Some("T-1".into()),
                attempt: Some(1),
                sender: "p:heat".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Result,
                urgent: false,
                body: "a result waits".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: now,
            })
        })
        .unwrap();
}

#[test]
fn an_idle_captain_gets_one_item_naming_the_open_reports() {
    let mut w = world();
    report(&mut w.store, "T-1", "the build took forever");
    report(&mut w.store, "T-2", "no permission for the sandbox");
    report(&mut w.store, "T-3", "the contract was unclear");

    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(report.friction, vec!["friction:opened".to_string()], "{report:?}");
    let open = items(&mut w.store);
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0].state, MsgState::Open);
    assert_eq!(open[0].recipient, "p:captain");
    assert_eq!(open[0].kind, MsgKind::System);
    assert!(!open[0].urgent, "an information item");
    assert_eq!(
        open[0].body,
        "3 friction reports: F-1 F-2 F-3. Run gear friction.",
        "{}",
        open[0].body
    );

    // A second tick adds no second item.
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert!(report.friction.is_empty(), "{report:?}");
    assert_eq!(items(&mut w.store).len(), 1, "one item per ring");
}

#[test]
fn a_captain_with_an_open_item_gets_no_ring() {
    let mut w = world();
    report(&mut w.store, "T-1", "the build took forever");
    captain_item(&mut w.store, T0);

    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert!(report.friction.is_empty(), "the captain has work of its own: {report:?}");
    assert!(items(&mut w.store).is_empty());

    // The captain is idle again: the ring comes.
    w.store
        .write(|tx| {
            let item = tx
                .thread_messages("p", "T-1:1")?
                .into_iter()
                .find(|message| message.recipient == "p:captain")
                .unwrap();
            tx.update_message_state("p", &item.id, MsgState::Resolved, Some("seen"), None, None, T0 + 2)
        })
        .unwrap();
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 3).unwrap();
    assert_eq!(report.friction, vec!["friction:opened".to_string()], "{report:?}");
    assert_eq!(items(&mut w.store).len(), 1);
}

#[test]
fn a_new_report_updates_the_item() {
    let mut w = world();
    report(&mut w.store, "T-1", "the build took forever");
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(items(&mut w.store)[0].body, "1 friction report: F-1. Run gear friction.");

    // A second report arrives while the item is open.
    report(&mut w.store, "T-2", "a missing tool");
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(report.friction, vec!["friction:updated".to_string()], "{report:?}");
    let all = items(&mut w.store);
    assert_eq!(all.len(), 1, "the same item, no second one: {all:?}");
    assert_eq!(all[0].state, MsgState::Open);
    assert_eq!(all[0].body, "2 friction reports: F-1 F-2. Run gear friction.");
}

#[test]
fn closing_every_report_closes_the_item() {
    let mut w = world();
    report(&mut w.store, "T-1", "the build took forever");
    report(&mut w.store, "T-2", "a missing tool");
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(items(&mut w.store).len(), 1);

    // One report is closed: the item stays, it still names an open report.
    w.store
        .write(|tx| tx.close_friction("p", "F-1", "fixed the fixture", T0 + 2))
        .unwrap();
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 3).unwrap();
    assert_eq!(report.friction, vec!["friction:updated".to_string()], "{report:?}");
    let all = items(&mut w.store);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].state, MsgState::Open, "one report is still open");
    assert_eq!(all[0].body, "1 friction report: F-2. Run gear friction.");

    // Every report is closed: the item closes itself.
    w.store
        .write(|tx| tx.close_friction("p", "F-2", "lesson: read the docs", T0 + 4))
        .unwrap();
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(report.friction, vec!["friction:closed".to_string()], "{report:?}");
    let all = items(&mut w.store);
    assert_eq!(all.len(), 1, "no new item: the same one closed");
    assert_eq!(all[0].state, MsgState::Resolved);
    assert!(
        all[0].result.as_deref().unwrap_or_default().contains("closed"),
        "{:?}",
        all[0].result
    );
}

/// The ring happens at most once per 30 minutes (design §6.4): a report that
/// arrives right after a ring waits for the next window.
#[test]
fn the_ring_waits_thirty_minutes_before_it_rings_again() {
    let mut w = world();
    report(&mut w.store, "T-1", "the build took forever");
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(items(&mut w.store).len(), 1);

    // Every report is closed: the item closes itself.
    w.store
        .write(|tx| tx.close_friction("p", "F-1", "fixed the fixture", T0 + 2))
        .unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 3).unwrap();
    assert_eq!(items(&mut w.store)[0].state, MsgState::Resolved);

    // A new report a minute later: the ring waits for the window.
    report(&mut w.store, "T-2", "a missing tool");
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 60).unwrap();
    assert!(report.friction.is_empty(), "too soon: {report:?}");
    assert_eq!(items(&mut w.store).len(), 1, "no new item yet");

    // Half an hour later the captain is told.
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1801).unwrap();
    assert_eq!(report.friction, vec!["friction:opened".to_string()], "{report:?}");
    let all = items(&mut w.store);
    assert_eq!(all.len(), 2, "a new item after the window: {all:?}");
    assert_eq!(all[1].state, MsgState::Open);
    assert_eq!(all[1].body, "1 friction report: F-2. Run gear friction.");
}
