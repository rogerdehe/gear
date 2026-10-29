//! T-169 (design §6.4): the friction ring — one item on the thread
//! `friction`, at most once per 30 minutes, updated by a new report and
//! closed when every report it lists is closed.
//!
//! T-416: the ring no longer waits for an idle captain (a captain on a long
//! voyage always has an actionable item and the reports were never rung), the
//! item names each report with its task, role and the start of its body, and
//! an item resolved while its reports are still open comes back after the
//! cooldown.

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
    seed(&mut store);
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "claude", AgentStatus::Idle);
    let mut cfg = EngineCfg { tick_s: 4, cooldown_s: 30, doorbell_check_budget: 3, ..Default::default() };
    cfg.profiles.insert("claude".to_string(), gear_herdr::builtin("claude").unwrap());
    World { store, herdr, cfg }
}

/// The same project in a store of its own: the state of the ring is in the
/// store, so a restart reads it back.
fn world_at(path: &std::path::Path) -> World {
    let mut store = Store::open(path).unwrap();
    seed(&mut store);
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "claude", AgentStatus::Idle);
    let mut cfg = EngineCfg { tick_s: 4, cooldown_s: 30, doorbell_check_budget: 3, ..Default::default() };
    cfg.profiles.insert("claude".to_string(), gear_herdr::builtin("claude").unwrap());
    World { store, herdr, cfg }
}

/// The project of a world: a captain and nothing else to do.
fn seed(store: &mut Store) {
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "claude", T0)?;
            Ok(())
        })
        .unwrap();
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

/// The captain's items, oldest first.
fn items(store: &mut Store) -> Vec<gear_store::MessageRow> {
    store.read(|tx| tx.thread_messages("p", "friction")).unwrap()
}

/// The captain's doorbell ops: one per prompt.
fn doorbells(store: &mut Store) -> Vec<gear_store::OpRow> {
    store.read(|tx| tx.ops_of_seat_kind("p:captain", gear_model::OpKind::Doorbell)).unwrap()
}

/// The captain's view of a message: it has seen it.
fn seen(store: &mut Store, message: &str, at: i64) {
    store
        .write(|tx| {
            let seat = tx.get_seat("p:captain")?.unwrap();
            tx.insert_view(&gear_store::ViewRecord {
                project: "p".into(),
                seat: "p:captain".into(),
                generation: seat.generation,
                ts: at,
                shown_msgs: vec![message.to_string()],
                shown_ops: vec![],
                shown_task: None,
                shown_attempt: None,
                shown_contract: None,
            })
        })
        .unwrap();
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
        "3 friction reports:\n\
         F-3 T-3 dev: the contract was unclear\n\
         F-2 T-2 dev: no permission for the sandbox\n\
         F-1 T-1 dev: the build took forever\n\
         run gear friction",
        "{}",
        open[0].body
    );

    // A second tick adds no second item.
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert!(report.friction.is_empty(), "{report:?}");
    assert_eq!(items(&mut w.store).len(), 1, "one item per ring");
}

/// T-416: a captain on a long voyage always has an actionable item; the
/// reports must reach it all the same.
#[test]
fn a_busy_captain_still_gets_the_ring() {
    let mut w = world();
    report(&mut w.store, "T-1", "the build took forever");
    captain_item(&mut w.store, T0);

    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(report.friction, vec!["friction:opened".to_string()], "{report:?}");
    let open = items(&mut w.store);
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0].recipient, "p:captain");
    assert_eq!(
        open[0].body,
        "1 friction report:\nF-1 T-1 dev: the build took forever\nrun gear friction",
        "{}",
        open[0].body
    );

    // The captain's own work is untouched: it still waits.
    let work = w.store.read(|tx| tx.thread_messages("p", "T-1:1")).unwrap();
    assert_eq!(work.len(), 1, "{work:?}");
    assert_eq!(work[0].state, MsgState::Open);
}

#[test]
fn a_new_report_updates_the_item() {
    let mut w = world();
    report(&mut w.store, "T-1", "the build took forever");
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(
        items(&mut w.store)[0].body,
        "1 friction report:\nF-1 T-1 dev: the build took forever\nrun gear friction"
    );

    // A second report arrives while the item is open.
    report(&mut w.store, "T-2", "a missing tool");
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(report.friction, vec!["friction:updated".to_string()], "{report:?}");
    let all = items(&mut w.store);
    assert_eq!(all.len(), 1, "the same item, no second one: {all:?}");
    assert_eq!(all[0].state, MsgState::Open);
    assert_eq!(
        all[0].body,
        "2 friction reports:\n\
         F-2 T-2 dev: a missing tool\n\
         F-1 T-1 dev: the build took forever\n\
         run gear friction"
    );
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
    assert_eq!(
        all[0].body,
        "1 friction report:\nF-2 T-2 dev: a missing tool\nrun gear friction"
    );

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
    assert_eq!(
        all[1].body,
        "1 friction report:\nF-2 T-2 dev: a missing tool\nrun gear friction"
    );
}

/// F1 of the review: the friction item is not an ordinary stale item. The
/// generic stale reminder turned it urgent and rang the captain a second time
/// for the same reports, 15 minutes after the first ring (design §6.4: the
/// ring happens once, and only a new report rings again).
#[test]
fn the_friction_item_is_not_promoted_and_does_not_ring_the_captain_again() {
    let mut w = world();
    report(&mut w.store, "T-1", "the build took forever");

    // The first tick: the item and the captain's first prompt.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    let item = items(&mut w.store).remove(0);
    assert_eq!(item.state, MsgState::Open);
    assert!(!item.urgent, "an information item");
    let prompts = doorbells(&mut w.store).len();
    assert_eq!(prompts, 1, "the captain is rung once: {prompts}");

    // The captain reads it.
    seen(&mut w.store, &item.id, T0 + 2);
    // The captain is busy now, and a quarter of an hour has passed.
    w.herdr.add_agent(CAP_PANE, "claude", AgentStatus::Working);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 901).unwrap();

    // The item is still an information item, and the captain is not rung a
    // second time for it.
    let item = items(&mut w.store).remove(0);
    assert_eq!(item.state, MsgState::Open, "the item waits for its reports");
    assert!(!item.urgent, "a friction item is never promoted: {item:?}");
    assert_eq!(doorbells(&mut w.store).len(), prompts, "no second prompt");

    // A later tick with no new report rings nothing either.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1801).unwrap();
    assert_eq!(doorbells(&mut w.store).len(), prompts, "no second prompt");
    assert!(!items(&mut w.store)[0].urgent);
}

/// T-416: an item the captain resolves while its reports are still open
/// counts as rung, not as handled: the next ring after the cooldown names the
/// reports still open again. Resolving the item alone closes no report.
#[test]
fn a_resolved_item_comes_back_with_the_reports_still_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    let mut w = world_at(&path);
    report(&mut w.store, "T-1", "the build took forever");

    // The first ring: the item names F-1.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    let first = items(&mut w.store).remove(0);
    assert_eq!(
        first.body,
        "1 friction report:\nF-1 T-1 dev: the build took forever\nrun gear friction"
    );

    // The captain resolves it by hand (what `gear reply M-n --resolve` does):
    // the report stays open.
    w.store
        .write(|tx| {
            tx.update_message_state("p", &first.id, MsgState::Resolved, Some("seen"), None, None, T0 + 2)
        })
        .unwrap();

    // The cooldown is not over yet: no second item.
    let tick_report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 60).unwrap();
    assert!(tick_report.friction.is_empty(), "too soon: {tick_report:?}");
    assert_eq!(items(&mut w.store).len(), 1, "no second item yet");

    // A restart does not forget the last ring: the cooldown holds across a
    // reload of the store.
    drop(w.store);
    let store = Store::open(&path).unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "claude", AgentStatus::Idle);
    let mut cfg = EngineCfg { tick_s: 4, cooldown_s: 30, doorbell_check_budget: 3, ..Default::default() };
    cfg.profiles.insert("claude".to_string(), gear_herdr::builtin("claude").unwrap());
    let mut w = World { store, herdr, cfg };
    let tick_report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1000).unwrap();
    assert!(tick_report.friction.is_empty(), "the state survives the restart: {tick_report:?}");

    // Half an hour after the ring, the item comes back with F-1 still open.
    let tick_report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1801).unwrap();
    assert_eq!(tick_report.friction, vec!["friction:opened".to_string()], "{tick_report:?}");
    let all = items(&mut w.store);
    assert_eq!(all.len(), 2, "exactly one new item: {all:?}");
    assert_eq!(all[1].state, MsgState::Open);
    assert_eq!(
        all[1].body,
        "1 friction report:\nF-1 T-1 dev: the build took forever\nrun gear friction",
        "{}",
        all[1].body
    );

    // At most one ring per 30 minutes.
    let tick_report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1810).unwrap();
    assert!(tick_report.friction.is_empty(), "one ring per window: {tick_report:?}");
    assert_eq!(items(&mut w.store).len(), 2, "no third item");

    // Closing the report closes the item, as today.
    w.store
        .write(|tx| tx.close_friction("p", "F-1", "fixed the fixture", T0 + 1811))
        .unwrap();
    let tick_report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1812).unwrap();
    assert_eq!(tick_report.friction, vec!["friction:closed".to_string()], "{tick_report:?}");
    let all = items(&mut w.store);
    assert_eq!(all.len(), 2, "no third item: the second one closed");
    assert_eq!(all[1].state, MsgState::Resolved);
}

/// T-416: one item names at most ten reports, newest first, and says how many
/// it left out.
#[test]
fn the_item_names_at_most_ten_reports() {
    let mut w = world();
    for n in 1..=13 {
        report(&mut w.store, &format!("T-{n}"), &format!("slowdown number {n}"));
    }

    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(report.friction, vec!["friction:opened".to_string()], "{report:?}");
    let open = items(&mut w.store);
    assert_eq!(open.len(), 1, "{open:?}");
    let mut expected = String::from("13 friction reports:");
    for n in (4..=13).rev() {
        expected.push_str(&format!("\nF-{n} T-{n} dev: slowdown number {n}"));
    }
    expected.push_str("\nand 3 more: run gear friction");
    assert_eq!(open[0].body, expected, "{}", open[0].body);
}

/// T-416: the item quotes the first 160 characters of a report's body.
#[test]
fn the_item_quotes_the_start_of_a_body() {
    let mut w = world();
    let long = format!("{}the tail", "a".repeat(160));
    report(&mut w.store, "T-1", &long);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    let all = items(&mut w.store);
    let expected = format!("1 friction report:\nF-1 T-1 dev: {}\nrun gear friction", "a".repeat(160));
    assert_eq!(all[0].body, expected, "{}", all[0].body);
    assert!(!all[0].body.contains("the tail"), "{}", all[0].body);
}
