//! T-68: a blocked seat is not re-delivered every tick (trial G09).
//!
//! While Herdr reports a seat blocked, the engine kept creating a delivery op
//! that the blocked agent refused, every ~4 s. Now a blocked seat gets no new
//! delivery: at most one stays pending, the blocked-seat alert (T-57) carries
//! it, and when the seat works or is idle again it is delivered once.

use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, OpRow, Store, TaskRow};

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
            })?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP, "opencode", AgentStatus::Idle);
    herdr.add_agent(LANE, "opencode", status);
    let mut cfg = EngineCfg::default();
    cfg.profiles
        .insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
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
fn a_blocked_seat_keeps_one_pending_delivery_and_gets_one_after_it_clears() {
    let mut w = world(AgentStatus::Blocked);
    // Ten ticks while the agent sits on its dialog.
    ticks(&mut w, 0, 10);

    let ops = deliveries(&mut w);
    assert_eq!(ops.len(), 1, "one op, not one per tick: {ops:?}");
    assert_eq!(ops[0].state, OpState::Intended, "it stays pending: {ops:?}");
    assert!(
        w.herdr.prompts_to(LANE).is_empty(),
        "nothing is typed into a blocked agent: {:?}",
        w.herdr.prompts_to(LANE)
    );

    // The dialog is dismissed: the lane is idle, and the pending delivery goes.
    w.herdr.add_agent(LANE, "opencode", AgentStatus::Idle);
    ticks(&mut w, 10, 14);

    let ops = deliveries(&mut w);
    assert_eq!(ops.len(), 1, "one delivery, never a second op: {ops:?}");
    assert_eq!(ops[0].state, OpState::Submitted, "{ops:?}");
    assert_eq!(w.herdr.prompts_to(LANE).len(), 1, "delivered once");
    assert!(
        w.herdr.prompts_to(LANE)[0].contains("New task T-1"),
        "{:?}",
        w.herdr.prompts_to(LANE)
    );
}
