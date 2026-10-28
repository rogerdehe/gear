//! T-130: a message for a lane reaches it while it works.
//!
//! The engine used to ring a lane only when it is idle, so a lane busy for long
//! stretches never saw the captain's instructions (the trial's deck sat on five
//! for an hour). An urgent message, and every captain instruction to a lane
//! whose agent queues input typed while it works (pi, claude), is typed at once;
//! a normal message to an agent that does not queue (opencode, codex) still
//! waits for idle. One doorbell per message, never repeated.

use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::{FakeHerdr, Inject};
use gear_herdr::AgentStatus;
use gear_model::{MsgKind, MsgState, OpKind, OpState, SeatRole};
use gear_store::{testing, NewMessage, Store};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const PI_PANE: &str = "w1:p2";
const LANE_PANE: &str = "w1:p3";

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

/// A project with an idle codex captain and two lanes — `pi` (queues input
/// while it works) and `other` (codex, does not) — in the status the caller
/// asks for.
fn world(pi: AgentStatus, other: AgentStatus) -> World {
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
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PI_PANE, "pi", T0)?;
            testing::seed_active_seat(tx, "p", "wire", SeatRole::Lane, LANE_PANE, "codex", T0)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(PI_PANE, "pi", pi);
    herdr.add_agent(LANE_PANE, "codex", other);
    let cfg = EngineCfg {
        tick_s: 4,
        cooldown_s: 30,
        doorbell_check_budget: 3,
        profiles: gear_herdr::builtins(),
        ..Default::default()
    };
    World { store, herdr, cfg }
}

impl World {
    /// A message from the captain to `to`, open (an action item).
    fn captain_message(&mut self, to: &str, kind: MsgKind, urgent: bool, at: i64) -> String {
        self.store
            .write(|tx| {
                Ok(tx
                    .insert_message(&NewMessage {
                        meta: None,
                        project: "p".into(),
                        thread: format!("keel:{at}"),
                        task: None,
                        attempt: None,
                        sender: "p:captain".into(),
                        sender_gen: 1,
                        recipient: to.into(),
                        kind,
                        urgent,
                        body: "read the plan".into(),
                        reply_to: None,
                        needs_ack: false,
                        state: MsgState::Open,
                        created: at,
                    })?
                    .id)
            })
            .unwrap()
    }

    fn doorbell_ops(&mut self) -> Vec<gear_store::OpRow> {
        self.store
            .read(|tx| tx.ops_of_seat_kind("p:heat", OpKind::Doorbell))
            .unwrap()
    }
}

#[test]
fn a_working_pi_lane_gets_the_captains_instruction_in_the_same_tick() {
    let mut w = world(AgentStatus::Working, AgentStatus::Idle);
    let id = w.captain_message("p:heat", MsgKind::Instruction, false, T0 + 10);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();

    let said = w.herdr.prompts_to(PI_PANE);
    assert_eq!(said.len(), 1, "typed while the agent works: {said:?}");
    assert!(
        said[0].contains(&id),
        "the doorbell names the message: {said:?}"
    );
    // T-150 (design §10.4): the lane has no open attempt, so the text says so
    // and tells it to stop working on earlier things.
    assert!(said[0].contains("You have no task now"), "{said:?}");
    let ops = w.doorbell_ops();
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0].state, OpState::Submitted);
}

/// F2 of the T-130 review: a doorbell Herdr refused never reached the pane, so
/// it covers nothing — the same message is rung again on the next eligible tick
/// (an at-once message needs no cooldown).
#[test]
fn a_refused_doorbell_is_rung_again_on_the_next_tick() {
    let mut w = world(AgentStatus::Working, AgentStatus::Idle);
    // The captain is busy too, so the only prompt of this tick is the lane's
    // at-once doorbell — the one Herdr refuses here. Every agent already
    // carries its name (T-134), so no rename spends the injected refusal: the
    // next side-effecting call of the tick must be this test's doorbell.
    w.herdr.set_agent_name(CAP_PANE, Some("p-captain"));
    w.herdr.set_agent_name(PI_PANE, Some("p-heat"));
    w.herdr.set_agent_name(LANE_PANE, Some("p-wire"));
    w.herdr.set_status(CAP_PANE, AgentStatus::Working);
    let id = w.captain_message("p:heat", MsgKind::Instruction, false, T0 + 10);
    w.herdr.inject_next = Some(Inject::Refuse {
        code: "agent_not_ready".into(),
    });

    let first = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();
    let ops = w.doorbell_ops();
    assert_eq!(ops.len(), 1, "{ops:?}");
    // T-149 (design §10.1): the refusal is recovered — the agent of the seat
    // is there, so it is named again and the same op types the doorbell on the
    // next tick.
    assert_eq!(ops[0].state, OpState::Intended, "the op waits: {ops:?}");
    assert!(
        first.executed.iter().any(|(_, state)| state == "intended"),
        "{first:?}"
    );

    // Herdr is healthy again; the message was never typed, so it is rung now.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 21).unwrap();

    let said = w.herdr.prompts_to(PI_PANE);
    assert_eq!(
        said.len(),
        2,
        "the refused attempt, then the real doorbell: {said:?}"
    );
    assert!(said[1].contains(&id), "{said:?}");
    let ops = w.doorbell_ops();
    assert_eq!(ops.len(), 1, "the same op, typed again: {ops:?}");
    assert_eq!(ops[0].state, OpState::Submitted);
}

#[test]
fn a_working_codex_lane_waits_for_idle_with_a_normal_message() {
    let mut w = world(AgentStatus::Idle, AgentStatus::Working);
    w.captain_message("p:wire", MsgKind::Instruction, false, T0 + 10);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();

    assert!(
        w.herdr.prompts_to(LANE_PANE).is_empty(),
        "a normal message does not go into a running codex turn: {:?}",
        w.herdr.prompts_to(LANE_PANE)
    );
}

#[test]
fn a_working_codex_lane_gets_an_urgent_message_at_once() {
    let mut w = world(AgentStatus::Idle, AgentStatus::Working);
    let id = w.captain_message("p:wire", MsgKind::Instruction, true, T0 + 10);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();

    let said = w.herdr.prompts_to(LANE_PANE);
    assert_eq!(
        said.len(),
        1,
        "an urgent message reaches a running turn: {said:?}"
    );
    assert!(said[0].contains(&id), "{said:?}");
}

#[test]
fn the_same_message_is_never_rung_twice() {
    let mut w = world(AgentStatus::Working, AgentStatus::Idle);
    let id = w.captain_message("p:heat", MsgKind::Instruction, false, T0 + 10);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();
    // The lane is still working and never ran `gear`; the cooldown is long past.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 500).unwrap();

    let said = w.herdr.prompts_to(PI_PANE);
    assert_eq!(said.len(), 1, "one doorbell per message: {said:?}");
    assert!(said[0].contains(&id), "{said:?}");
    assert_eq!(w.doorbell_ops().len(), 1);
}
