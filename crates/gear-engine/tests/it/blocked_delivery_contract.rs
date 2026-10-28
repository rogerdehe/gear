//! T-68: a blocked seat is not re-delivered every tick (trial G09).
//!
//! While Herdr reports a seat blocked, the engine kept creating a delivery op
//! that the blocked agent refused, every ~4 s. Now a blocked seat gets no
//! delivery at all (design §10.2: only `idle` and `done` are ready, and a
//! blocked agent is never ready), the blocked-seat alert (T-57) carries it,
//! and when the seat works or is idle again it is delivered once.

use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{MsgState, OpKind, OpState, SeatRole, SeatState, TaskState};
use gear_store::{testing, ContractRow, OpRow, Store, TaskRow, ViewRecord};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const LANE: &str = "w1:p2";

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

/// A project with one lane and one ready task, its agent in `status`.
fn world(status: AgentStatus) -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "opencode", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T0)?;
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
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: None,
                issue: None,
                updated: T0,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-1".into(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP, "opencode", AgentStatus::Idle);
    herdr.add_agent(LANE, "opencode", status);
    let mut cfg = EngineCfg::default();
    cfg.profiles
        .insert("opencode".into(), { let mut p = gear_herdr::builtin("opencode").unwrap(); p.new_session.clear(); p });
    World { store, herdr, cfg }
}

/// Every delivery op of the lane, in any state.
fn deliveries(w: &mut World) -> Vec<OpRow> {
    w.store
        .read(|tx| {
            let mut out = Vec::new();
            for state in [
                OpState::Intended,
                OpState::Executing,
                OpState::Submitted,
                OpState::Unknown,
                OpState::Confirmed,
                OpState::Failed,
                OpState::Void,
            ] {
                for op in tx.ops_in_state("p", state)? {
                    if op.kind == OpKind::Deliver && op.seat == "p:heat" {
                        out.push(op);
                    }
                }
            }
            Ok(out)
        })
        .unwrap()
}

fn ticks(w: &mut World, from: i64, to: i64) {
    for n in from..to {
        tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + n * 4 + 1).unwrap();
    }
}

#[test]
fn a_blocked_seat_gets_no_delivery_and_one_after_it_clears() {
    let mut w = world(AgentStatus::Blocked);
    // Ten ticks while the agent sits on its dialog.
    ticks(&mut w, 0, 10);

    let ops = deliveries(&mut w);
    assert!(ops.is_empty(), "session setup waits until the dialog clears: {ops:?}");
    assert!(
        w.herdr.prompts_to(LANE).is_empty(),
        "nothing is typed into a blocked agent: {:?}",
        w.herdr.prompts_to(LANE)
    );

    // The dialog is dismissed: session preparation completes and delivery goes.
    w.herdr.add_agent(LANE, "opencode", AgentStatus::Idle);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 41).unwrap();
    // The lane reads it (T-97: a delivery nobody confirms fails after its
    // budget; a view is what confirms it).
    w.store
        .write(|tx| {
            tx.insert_view(&ViewRecord {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                ts: T0 + 42,
                shown_msgs: vec![],
                shown_ops: vec![],
                shown_task: Some("T-1".into()),
                shown_attempt: Some(1),
                shown_contract: Some(1),
            })
        })
        .unwrap();
    for n in 11..14 {
        tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + n * 4 + 1).unwrap();
    }

    let ops = deliveries(&mut w);
    assert_eq!(ops.len(), 1, "one delivery, never a second op: {ops:?}");
    assert_eq!(ops[0].state, OpState::Confirmed, "{ops:?}");
    assert_eq!(w.herdr.prompts_to(LANE).len(), 1, "delivered once");
    assert!(
        w.herdr.prompts_to(LANE)[0].contains("New task T-1"),
        "{:?}",
        w.herdr.prompts_to(LANE)
    );
}

/// R1 of the T-68 review: a lane retired while it sits on a dialog is still
/// alerted (its pane is a dialog nobody may ignore), and session preparation
/// waits; when the dialog clears the assignment is delivered once.
#[test]
fn a_retiring_blocked_lane_is_alerted_and_gets_no_delivery() {
    let mut w = world(AgentStatus::Blocked);
    // The session is not prepared while the lane is blocked.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    let ops = deliveries(&mut w);
    assert!(ops.is_empty(), "session setup waits on the dialog: {ops:?}");

    // The captain retires the lane while it is blocked.
    w.store
        .write(|tx| {
            tx.set_seat_state("p:heat", SeatState::Retiring, Some("retired by the plan"), T0 + 2)
        })
        .unwrap();

    // Ten blocked ticks, more than the alert's two minutes.
    for n in 0..10 {
        tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 15 * (n + 1)).unwrap();
    }
    let ops = deliveries(&mut w);
    assert!(ops.is_empty(), "no delivery before the dialog clears: {ops:?}");
    let alerts = w
        .store
        .read(|tx| tx.thread_messages("p", "blocked:p:heat"))
        .unwrap();
    assert_eq!(alerts.len(), 1, "the blocked alert: {alerts:?}");
    assert_eq!(alerts[0].state, MsgState::Open);

    // The dialog is dismissed: one delivery, once, and the lane reads it.
    w.herdr.add_agent(LANE, "opencode", AgentStatus::Idle);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 165).unwrap();
    w.store
        .write(|tx| {
            tx.insert_view(&ViewRecord {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                ts: T0 + 166,
                shown_msgs: vec![],
                shown_ops: vec![],
                shown_task: Some("T-1".into()),
                shown_attempt: Some(1),
                shown_contract: Some(1),
            })
        })
        .unwrap();
    // The dialog is dismissed, but the lane is retiring: the dispatch never
    // assigns to a retiring seat, so no delivery goes out (design/09).
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 180).unwrap();
    let ops = deliveries(&mut w);
    assert!(
        ops.is_empty(),
        "a retiring seat is never handed a task: {ops:?}"
    );
    assert!(
        w.herdr.prompts_to(LANE).is_empty(),
        "nothing was typed into it: {:?}",
        w.herdr.prompts_to(LANE)
    );
}
