//! T-294: the engine carries on after its Herdr server goes away and comes
//! back (T-176's rest).
//!
//! `gear_herdr::SocketHerdr` opens a connection per call, so it has no
//! connection to lose: a Herdr server restarted on the same socket path is
//! reached by the next call. The engine's half is that a failed observation
//! decides nothing — the tick reports the project in `observe_failed` and
//! writes no op — and the next tick, with the session readable again, delivers
//! the task that waited.

use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::{Call, FakeHerdr};
use gear_herdr::AgentStatus;
use gear_model::{OpState, SeatRole, TaskState};
use gear_store::{testing, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const TASK: &str = "T-1";

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

/// A project with an idle captain and one idle lane (`heat`), and a ready task
/// for the lane: a readable session delivers it in one tick.
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
                "codex",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: TASK.into(),
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
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    // Named agents: this test is about the outage, not about Herdr's restart
    // clearing names.
    herdr.add_agent_named(CAP_PANE, "codex", AgentStatus::Idle, "p-captain");
    herdr.add_agent_named(LANE_PANE, "opencode", AgentStatus::Idle, "p-heat");
    let mut cfg = EngineCfg {
        tick_s: 4,
        cooldown_s: 30,
        doorbell_check_budget: 3,
        profiles: gear_herdr::builtins(),
        ..Default::default()
    };
    cfg.profiles
        .get_mut("opencode")
        .unwrap()
        .new_session
        .clear();
    World { store, herdr, cfg }
}

impl World {
    /// Every op row written so far, whatever its state.
    fn ops(&mut self) -> Vec<String> {
        let states = [
            OpState::Intended,
            OpState::Executing,
            OpState::Submitted,
            OpState::Confirmed,
            OpState::Failed,
            OpState::Unknown,
            OpState::Void,
        ];
        let mut ids = Vec::new();
        for state in states {
            ids.extend(
                self.store
                    .read(|tx| tx.ops_in_state("p", state))
                    .unwrap()
                    .into_iter()
                    .map(|op| op.id),
            );
        }
        ids
    }

    /// The lane's stall thread: the items the captain would be rung with.
    fn stall_items(&mut self) -> Vec<gear_store::MessageRow> {
        self.store
            .read(|tx| tx.thread_messages("p", "stall:p:heat"))
            .unwrap()
    }
}

#[test]
fn a_failed_observation_decides_nothing_and_the_next_tick_acts_again() {
    let mut w = world();

    // The Herdr server goes away: the session cannot list its agents.
    w.herdr.agents_fail = true;
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    assert_eq!(report.observe_failed, vec!["p".to_string()], "{report:?}");
    let ops = w.ops();
    assert!(ops.is_empty(), "a failed observation writes no op: {ops:?}");
    let stall = w.stall_items();
    assert!(stall.is_empty(), "and no stall item: {stall:?}");
    assert!(
        w.herdr.calls.is_empty(),
        "and no side effect: {:?}",
        w.herdr.calls
    );

    // Two failed ticks in a row: still nothing decided, nothing flagged.
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();
    assert_eq!(report.observe_failed, vec!["p".to_string()], "{report:?}");
    assert!(report.flagged.is_empty(), "{report:?}");
    let ops = w.ops();
    assert!(
        ops.is_empty(),
        "the second failed tick writes no op: {ops:?}"
    );
    let stall = w.stall_items();
    assert!(stall.is_empty(), "{stall:?}");
    assert!(w.herdr.calls.is_empty(), "{:?}", w.herdr.calls);

    // The server is back on the same path: the next tick reaches it and acts.
    w.herdr.agents_fail = false;
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 30).unwrap();
    assert!(report.observe_failed.is_empty(), "{report:?}");
    assert!(
        w.herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::Prompt { pane, text, .. } if pane == LANE_PANE && text.contains(TASK))),
        "the ready task's packet is delivered: {:?}",
        w.herdr.calls
    );
    let said = w.herdr.prompts_to(LANE_PANE);
    assert_eq!(
        said.len(),
        1,
        "one packet, not one per failed tick: {:?}",
        w.herdr.calls
    );
    assert!(said[0].contains(TASK), "{said:?}");
}
