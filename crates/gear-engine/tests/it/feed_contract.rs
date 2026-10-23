//! Keep-lanes-fed contract (T-7): the engine tells the captain before a lane
//! runs dry — one item per lane, no duplicates, closed when the queue is
//! refilled, and nothing for a lane that is not supposed to work.

use gear_engine::{feed, tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{AttemptState, MsgKind, MsgState, SeatRole, SeatState, TaskState};
use gear_store::{testing, NewMessage, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

fn profile(kind: &str) -> gear_herdr::Profile {
    gear_herdr::Profile {
        kind: kind.into(),
        busy_markers: vec!["esc interrupt".into()],
        ratelimit_markers: vec![],
        quota_markers: vec![],
        auth_markers: vec![],
        tail_lines: 16,
        confirm_rounds: 2,
        unattended_args: vec!["--auto".into()],
        session_ref: false,
        resume_args: None,
        exit_keys: vec![],
        exit_command: None,
        verified: true,
    }
}

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

/// A project with a captain and one lane, `tasks` ready tasks on that lane.
/// The lane's agent is `lane` status: a busy lane is not assigned its ready
/// tasks by the same tick, which is what these tests want to observe.
fn world(lane: AgentStatus, tasks: usize) -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
            for n in 1..=tasks {
                task(tx, &format!("T-{n}"), n as i64)?;
            }
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", lane);
    let mut cfg = EngineCfg { tick_s: 4, cooldown_s: 30, doorbell_check_budget: 3, ..Default::default() };
    for kind in ["codex", "opencode"] {
        cfg.profiles.insert(kind.to_string(), profile(kind));
    }
    World { store, herdr, cfg }
}

fn task(tx: &gear_store::Tx<'_>, id: &str, rank: i64) -> gear_model::GearResult<()> {
    tx.insert_task(&TaskRow {
        project: "p".into(),
        id: id.into(),
        rank,
        state: TaskState::Ready,
        priority: "must".into(),
        lane: Some("heat".into()),
        milestone: None,
        required: false,
        contract_version: 1,
        deps: Vec::new(),
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        updated: T0,
    })
}

/// The lane's items, open and closed.
fn items(store: &mut Store, lane: &str) -> Vec<gear_store::MessageRow> {
    store
        .read(|tx| tx.thread_messages("p", &format!("lane:{lane}")))
        .unwrap()
}

#[test]
fn a_lane_running_dry_gets_one_item_and_loses_it_when_refilled() {
    let mut w = world(AgentStatus::Working, 1);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].state, MsgState::Open);
    assert_eq!(item[0].kind, MsgKind::System);
    assert_eq!(item[0].recipient, "p:captain");
    assert_eq!(item[0].body, "lane heat has 1 ready task left");

    // The next tick creates no duplicate.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(items(&mut w.store, "heat").len(), 1, "no duplicate");

    // The queue is refilled: the item closes and nothing new opens.
    w.store.write(|tx| task(tx, "T-2", 2)).unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].state, MsgState::Resolved);
}

#[test]
fn a_lane_with_nothing_at_all_is_idle() {
    let mut w = world(AgentStatus::Idle, 0);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();

    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].body, "lane heat is IDLE");
}

#[test]
fn a_parked_or_held_lane_is_not_warned_about() {
    let mut w = world(AgentStatus::Idle, 0);
    w.store
        .write(|tx| {
            tx.set_seat_state("p:heat", SeatState::Parked, Some("parked for the test"), T0)?;
            testing::seed_active_seat(tx, "p", "runtime", SeatRole::Lane, "w1:p3", "opencode", T0)?;
            tx.set_seat_state("p:runtime", SeatState::Held, Some("held for the test"), T0)?;
            Ok(())
        })
        .unwrap();

    // The observation is not what this test is about: the watch over the
    // store is.
    let changes = feed::keep_lanes_fed(&mut w.store, "p", T0 + 1).unwrap();

    assert!(changes.is_empty(), "{changes:?}");
    assert!(items(&mut w.store, "heat").is_empty());
    assert!(items(&mut w.store, "runtime").is_empty());
}

#[test]
fn an_item_from_a_previous_engine_is_not_duplicated() {
    let mut w = world(AgentStatus::Working, 1);
    // What a previous engine run left open.
    w.store
        .write(|tx| {
            tx.insert_message(&NewMessage {
                project: "p".into(),
                thread: "lane:heat".into(),
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: "p:captain".into(),
                kind: MsgKind::System,
                urgent: false,
                body: "lane heat has 1 ready task left".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T0,
            })?;
            Ok(())
        })
        .unwrap();

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 30).unwrap();

    assert_eq!(items(&mut w.store, "heat").len(), 1, "no duplicate after a restart");
}

#[test]
fn a_task_taken_by_a_delivery_is_not_ready_work() {
    let mut w = world(AgentStatus::Working, 2);
    // The first task's delivery was intended: a Delivered attempt while the
    // task is still Ready (the delivery is confirmed on a later tick).
    w.store
        .write(|tx| {
            tx.insert_attempt(&gear_store::AttemptRow {
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Delivered,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
            Ok(())
        })
        .unwrap();

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();

    // One task is taken, one left: the lane is in the danger zone (R4).
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].body, "lane heat has 1 ready task left");
}

#[test]
fn a_parked_lane_closes_its_open_warning() {
    let mut w = world(AgentStatus::Working, 1);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(items(&mut w.store, "heat")[0].state, MsgState::Open);

    // Parked while the warning is open: the warning goes (R3).
    w.store
        .write(|tx| tx.set_seat_state("p:heat", SeatState::Parked, Some("parked"), T0 + 2))
        .unwrap();
    let changes = feed::keep_lanes_fed(&mut w.store, "p", T0 + 3).unwrap();

    assert_eq!(changes.len(), 1, "{changes:?}");
    let item = items(&mut w.store, "heat");
    assert_eq!(item[0].state, MsgState::Resolved);
}

#[test]
fn a_new_shortage_after_recovery_is_reported_at_once() {
    let mut w = world(AgentStatus::Working, 1);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(items(&mut w.store, "heat").len(), 1, "the first shortage opens an item");

    // Refilled: the item closes.
    w.store.write(|tx| task(tx, "T-2", 2)).unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(items(&mut w.store, "heat")[0].state, MsgState::Resolved);

    // The new work is finished, the lane is dry again: the alert must be
    // there on that same tick — no cooldown hides a new shortage (R1).
    w.store
        .write(|tx| {
            let mut done = tx.get_task("p", "T-2").unwrap().unwrap();
            done.state = TaskState::Done;
            tx.update_task(&done)?;
            Ok(())
        })
        .unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 9).unwrap();

    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 2, "{item:?}");
    assert_eq!(item[1].state, MsgState::Open);
    assert_eq!(item[1].body, "lane heat has 1 ready task left");
}

#[test]
fn a_deferred_alert_is_the_same_item_and_resolves_on_refill() {
    let mut w = world(AgentStatus::Working, 1);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    // The captain defers the item: Waiting, due later (what
    // mail::reply(Wait) writes).
    w.store
        .write(|tx| {
            tx.update_message_state(
                "p",
                "M-1",
                MsgState::Waiting,
                None,
                Some("p:heat"),
                Some(T0 + 600),
                T0 + 2,
            )?;
            Ok(())
        })
        .unwrap();

    // The condition persists: the deferred item is the lane's item, so no
    // second System message appears (R2).
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 3).unwrap();
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].state, MsgState::Waiting);

    // Refilled: the deferred item resolves and never becomes due later.
    w.store.write(|tx| task(tx, "T-2", 2)).unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].state, MsgState::Resolved);
}
