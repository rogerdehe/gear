//! T-117 (trial B02-08): when every lane is in the tail while required work
//! remains, the captain hears it once — one item naming the tailed lanes and
//! the required tasks, no second one on the next tick, closed when the tail is
//! cleared.

use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{MsgState, SeatRole, TaskState};
use gear_store::{testing, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

/// A project with a captain and two busy lanes; the required task T-1 is
/// ready and no lane takes it in the same tick (both are working), so the
/// state lasts.
fn world() -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "claude",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            testing::seed_active_seat(tx, "p", "wire", SeatRole::Lane, "w1:p3", "opencode", T0)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: Some("heat".into()),
                milestone: None,
                required: true,
                contract_version: 1,
                deps: Vec::new(),
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: None,
                issue: None,
                updated: T0,
            })
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "claude", AgentStatus::Idle);
    herdr.add_agent("w1:p2", "opencode", AgentStatus::Working);
    herdr.add_agent("w1:p3", "opencode", AgentStatus::Working);
    let mut cfg = EngineCfg {
        tick_s: 4,
        cooldown_s: 30,
        doorbell_check_budget: 3,
        ..Default::default()
    };
    for kind in ["claude", "opencode"] {
        cfg.profiles
            .insert(kind.to_string(), gear_herdr::builtin(kind).unwrap());
    }
    World { store, herdr, cfg }
}

/// The captain's all-tail items, open and closed.
fn items(store: &mut Store) -> Vec<gear_store::MessageRow> {
    store
        .read(|tx| tx.thread_messages("p", "all-tail"))
        .unwrap()
}

fn tail(store: &mut Store, lane: &str, on: bool) {
    store
        .write(|tx| tx.set_meta(&format!("lane-tail:p:{lane}"), if on { "1" } else { "0" }))
        .unwrap();
}

#[test]
fn every_lane_tailed_with_a_required_task_is_one_item_and_clearing_the_tail_closes_it() {
    let mut w = world();
    tail(&mut w.store, "heat", true);
    tail(&mut w.store, "wire", true);

    // One tick: one item, naming the lanes and the required task.
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(
        report.tail,
        vec!["all-tail:opened".to_string()],
        "{report:?}"
    );
    let open = items(&mut w.store);
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0].state, MsgState::Open);
    assert_eq!(open[0].recipient, "p:captain");
    assert!(!open[0].urgent, "an information item");
    assert!(open[0].body.contains("heat, wire"), "{}", open[0].body);
    assert!(open[0].body.contains("T-1 (ready)"), "{}", open[0].body);

    // A second tick adds none.
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert!(report.tail.is_empty(), "{report:?}");
    assert_eq!(items(&mut w.store).len(), 1, "one item per state");

    // One lane out of the tail: the state ended, the same item is resolved.
    tail(&mut w.store, "wire", false);
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    assert_eq!(
        report.tail,
        vec!["all-tail:closed".to_string()],
        "{report:?}"
    );
    let all = items(&mut w.store);
    assert_eq!(all.len(), 1, "no new item: the same one closed");
    assert_eq!(all[0].state, MsgState::Resolved);
    assert!(
        all[0]
            .result
            .as_deref()
            .unwrap_or_default()
            .contains("tail is cleared"),
        "{:?}",
        all[0].result
    );

    // Every lane tailed again while the work is there: a fresh item.
    tail(&mut w.store, "wire", true);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 15).unwrap();
    let all = items(&mut w.store);
    assert_eq!(all.len(), 2, "the state began again");
    assert_eq!(all[1].state, MsgState::Open);
}

#[test]
fn a_required_task_ending_closes_the_item() {
    let mut w = world();
    tail(&mut w.store, "heat", true);
    tail(&mut w.store, "wire", true);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(items(&mut w.store).len(), 1);

    // The required task is cancelled: the work the tail hid is gone.
    w.store
        .write(|tx| {
            let mut task = tx.get_task("p", "T-1")?.unwrap();
            task.state = TaskState::Cancelled;
            tx.update_task(&task)
        })
        .unwrap();
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(
        report.tail,
        vec!["all-tail:closed".to_string()],
        "{report:?}"
    );
    let all = items(&mut w.store);
    assert_eq!(all[0].state, MsgState::Resolved);
    assert!(
        all[0]
            .result
            .as_deref()
            .unwrap_or_default()
            .contains("no required task"),
        "{:?}",
        all[0].result
    );
}

#[test]
fn one_lane_out_of_the_tail_opens_nothing() {
    let mut w = world();
    tail(&mut w.store, "heat", true);
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert!(report.tail.is_empty(), "{report:?}");
    assert!(items(&mut w.store).is_empty());
}

/// F1 of the review: the captain resolving the item is an acknowledgement, not
/// the end of the state — an unchanged tick adds no second item. Clearing the
/// tail ends the state; tailing the lanes again opens exactly one new item.
#[test]
fn a_resolved_item_is_not_reopened_while_every_lane_stays_tailed() {
    let mut w = world();
    tail(&mut w.store, "heat", true);
    tail(&mut w.store, "wire", true);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    let item = items(&mut w.store).remove(0);
    assert_eq!(item.state, MsgState::Open);

    // The captain resolves it (what `gear reply M-n --resolve` does).
    w.store
        .write(|tx| {
            tx.update_message_state(
                "p",
                &item.id,
                MsgState::Resolved,
                Some("seen"),
                None,
                None,
                T0 + 2,
            )
        })
        .unwrap();

    // The state is unchanged: no new item, nothing reported.
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert!(report.tail.is_empty(), "{report:?}");
    let all = items(&mut w.store);
    assert_eq!(all.len(), 1, "no second item: {all:?}");
    assert_eq!(all[0].state, MsgState::Resolved);

    // Clearing the tail ends the state (nothing open to close); tailing the
    // lanes again is a new state — exactly one new item.
    tail(&mut w.store, "wire", false);
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    assert!(report.tail.is_empty(), "{report:?}");
    tail(&mut w.store, "wire", true);
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 15).unwrap();
    assert_eq!(
        report.tail,
        vec!["all-tail:opened".to_string()],
        "{report:?}"
    );
    let all = items(&mut w.store);
    assert_eq!(all.len(), 2, "the new state is one new item: {all:?}");
    assert_eq!(all[1].state, MsgState::Open);
}
