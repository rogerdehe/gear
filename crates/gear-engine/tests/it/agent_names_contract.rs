//! T-134: every seat's agent keeps its Herdr name.
//!
//! Herdr clears an agent's name when it re-detects the occupant (a server
//! restart, a CLI that changes its session), and `agent.prompt` refuses a pane
//! whose agent is unnamed: after the M3.1 restart the captain and both lanes
//! showed as unnamed and their first deliveries were lost. Every tick gives
//! each seat with an active binding its name back; a name another pane holds is
//! reported to the captain once and never stolen; and a delivery to a pane whose
//! agent lost its name renames it and delivers in the same tick.

use gear_engine::ops::execute_intended;
use gear_engine::{names, tick, EngineCfg};
use gear_herdr::fake::{Call, FakeHerdr};
use gear_herdr::AgentStatus;
use gear_model::{MsgState, OpKind, SeatRole, SeatState, TaskState};
use gear_store::{testing, NewOp, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const REV_PANE: &str = "w1:p3";
/// A pane with an agent that is nobody's seat.
const OTHER_PANE: &str = "w1:p9";

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

/// A project with an idle codex captain and one lane (`heat`, opencode), every
/// agent in the state the caller leaves it in.
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
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
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
    /// A reviewer seat `review-1` on task T-7 (the seat is warm and its agent
    /// carries the seat's name, `<project>-review-1`; the task is in its
    /// title), with the packet delivery still to do.
    fn reviewer(&mut self) -> gear_store::SeatRow {
        let seat = self
            .store
            .write(|tx| {
                let seat =
                    testing::seed_temp_seat(tx, "p", "review-1", "T-7", REV_PANE, "codex", T0)?;
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-7".into(),
                    rank: 1,
                    state: TaskState::Verifying,
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
                Ok(seat)
            })
            .unwrap();
        self.herdr.add_agent(REV_PANE, "codex", AgentStatus::Idle);
        seat
    }

    /// An Intended delivery of the review packet to `seat`.
    fn packet(&mut self, seat: &gear_store::SeatRow) -> String {
        self.store
            .write(|tx| {
                Ok(tx
                    .insert_op(&NewOp {
                        project: "p".into(),
                        seat: seat.id.clone(),
                        generation: seat.generation,
                        kind: OpKind::Deliver,
                        task: Some("T-7".into()),
                        attempt: Some(1),
                        ref_msg: None,
                        plan_version: None,
                        payload: serde_json::json!({ "text": "Review T-7 attempt 1: packet at p/T-7.md" }),
                        created: T0 + 5,
                    })?
                    .id)
            })
            .unwrap()
    }

    fn name(&self, pane: &str) -> Option<&str> {
        self.herdr.agent_name(pane)
    }

    /// A ready task for the lane: the tick delivers it.
    fn ready_task(&mut self) -> String {
        self.store
            .write(|tx| {
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
                Ok("T-1".to_string())
            })
            .unwrap()
    }

    /// The captain's items about a name another pane holds.
    fn name_items(&mut self) -> Vec<gear_store::MessageRow> {
        self.store
            .read(|tx| tx.thread_messages("p", "name:p:heat"))
            .unwrap()
    }
}

#[test]
fn an_unnamed_lane_agent_is_renamed_in_one_tick() {
    let mut w = world();
    assert_eq!(
        w.name(LANE_PANE),
        None,
        "the state Herdr leaves after a restart"
    );

    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    assert_eq!(w.name(LANE_PANE), Some("p-heat"));
    assert_eq!(
        w.herdr.renames_to("p-heat"),
        vec![LANE_PANE],
        "one rename, to the seat's name"
    );
    assert!(report.renamed.contains(&"p:heat".to_string()), "{report:?}");
    // The captain's agent is a seat too, and got its name back in the same tick.
    assert_eq!(w.name(CAP_PANE), Some("p-captain"));

    // Named now: no second call.
    let before = w.herdr.renames_to("p-heat").len();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();
    assert_eq!(
        w.herdr.renames_to("p-heat").len(),
        before,
        "a named agent gets no call"
    );
}

#[test]
fn a_correctly_named_agent_gets_no_rename() {
    let mut w = world();
    w.herdr.set_agent_name(LANE_PANE, Some("p-heat"));
    w.herdr.set_agent_name(CAP_PANE, Some("p-captain"));

    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    assert!(
        w.herdr
            .calls
            .iter()
            .all(|call| !matches!(call, Call::Rename { .. })),
        "{:?}",
        w.herdr.calls
    );
    assert!(report.renamed.is_empty(), "{report:?}");
}

/// F1 of the T-134 review, T-482: a reviewer's agent is named after its seat
/// (`<project>-review-<k>`, the name its review started with), not after the
/// task — so a correctly named reviewer is left alone.
#[test]
fn a_correctly_named_reviewer_gets_no_rename() {
    let mut w = world();
    let seat = w.reviewer();
    w.herdr.set_agent_name(REV_PANE, Some("p-review-1"));

    // The tick's own step (restore), with the names Herdr reports now.
    let names = names::read(&mut w.herdr).unwrap();
    let restored = names::restore(&mut w.store, &mut w.herdr, "p", &names, T0 + 10).unwrap();

    assert!(!restored.renamed.contains(&seat.id), "{restored:?}");
    assert_eq!(
        w.name(REV_PANE),
        Some("p-review-1"),
        "the reviewer keeps the name its review started with"
    );
    assert!(
        !w.herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::Rename { pane, .. } if pane == REV_PANE)),
        "a correctly named reviewer is not renamed: {:?}",
        w.herdr.calls
    );
}

/// F1, T-482: a cleared reviewer is restored to its **seat's** name before the
/// delivery, and the packet still goes out.
#[test]
fn a_cleared_reviewer_is_restored_to_its_seat_name_before_delivery() {
    let mut w = world();
    let seat = w.reviewer();
    w.packet(&seat);
    assert_eq!(w.name(REV_PANE), None, "Herdr cleared the name");
    assert_eq!(
        names::desired(&seat),
        "p-review-1",
        "the seat's agent is named after its seat"
    );

    // The delivery path makes sure of the name before it types. T-149: the
    // agent lost its name, so it is renamed now and the packet waits for the
    // next tick — the name is in place before any text is typed.
    execute_intended(&mut w.store, &mut w.herdr, "p", T0 + 10).unwrap();

    assert_eq!(
        w.name(REV_PANE),
        Some("p-review-1"),
        "the agent carries its seat's name"
    );
    assert!(
        w.herdr.prompts_to(REV_PANE).is_empty(),
        "nothing is typed while the agent has no name: {:?}",
        w.herdr.prompts_to(REV_PANE)
    );
    assert!(
        !w.herdr.renames_to("p-review-1").is_empty(),
        "the agent is renamed to its seat's name: {:?}",
        w.herdr.calls
    );

    // The next tick: the packet goes out.
    execute_intended(&mut w.store, &mut w.herdr, "p", T0 + 20).unwrap();

    let renamed_at = w
        .herdr
        .calls
        .iter()
        .position(|call| matches!(call, Call::Rename { name, .. } if name == "p-review-1"))
        .unwrap_or_else(|| panic!("no seat-based rename: {:?}", w.herdr.calls));
    let prompt_at = w
        .herdr
        .calls
        .iter()
        .position(|call| matches!(call, Call::Prompt { pane, .. } if pane == REV_PANE))
        .unwrap_or_else(|| panic!("no packet: {:?}", w.herdr.calls));
    assert!(renamed_at < prompt_at, "named first: {:?}", w.herdr.calls);
    assert_eq!(w.name(REV_PANE), Some("p-review-1"));
    assert!(
        w.herdr.prompts_to(REV_PANE)[0].contains("Review T-7"),
        "{:?}",
        w.herdr.prompts_to(REV_PANE)
    );
    assert!(
        !w.herdr.renames_to("p-review-1").is_empty(),
        "the agent is renamed to its seat's name: {:?}",
        w.herdr.calls
    );
}

#[test]
fn a_name_held_by_another_pane_is_reported_once_and_never_stolen() {
    let mut w = world();
    w.herdr
        .add_agent_named(OTHER_PANE, "opencode", AgentStatus::Idle, "p-heat");

    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    assert_eq!(w.name(LANE_PANE), None, "the seat's agent is not named");
    assert_eq!(
        w.name(OTHER_PANE),
        Some("p-heat"),
        "and the holder keeps its name"
    );
    assert!(
        w.herdr.renames_to("p-heat").is_empty(),
        "nothing was stolen"
    );
    assert!(
        report.name_taken.contains(&"p:heat".to_string()),
        "{report:?}"
    );

    // One item for the captain, in its own thread, naming the holder.
    let items = w.name_items();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].recipient, "p:captain");
    assert_eq!(items[0].state, MsgState::Open);
    assert!(items[0].body.contains("p-heat"), "{}", items[0].body);
    assert!(items[0].body.contains(OTHER_PANE), "{}", items[0].body);

    // Every tick while the name is held: still one item, still no rename.
    for now in [T0 + 20, T0 + 30, T0 + 40] {
        tick(&mut w.store, &mut w.herdr, &w.cfg, now).unwrap();
    }
    assert_eq!(w.name_items().len(), 1, "reported once, not every tick");
    assert!(w.herdr.renames_to("p-heat").is_empty());

    // The holder lets the name go: the next tick names the seat and resolves
    // its item.
    w.herdr.set_agent_name(OTHER_PANE, None);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 50).unwrap();
    assert_eq!(w.name(LANE_PANE), Some("p-heat"));
    let items = w.name_items();
    assert_eq!(items[0].state, MsgState::Resolved, "{items:?}");
}

#[test]
fn a_captain_whose_name_another_pane_holds_gets_one_item_and_its_name_back() {
    let mut w = world();
    // The captain's agent lost its name, and another pane carries `p-captain`.
    w.herdr
        .add_agent_named(OTHER_PANE, "codex", AgentStatus::Idle, "p-captain");

    for now in [T0 + 10, T0 + 20] {
        let names = names::read(&mut w.herdr).unwrap();
        let restored = names::restore(&mut w.store, &mut w.herdr, "p", &names, now).unwrap();
        assert!(
            restored.taken.contains(&"p:captain".to_string()),
            "{restored:?}"
        );
    }

    assert_eq!(w.name(CAP_PANE), None, "the seat's agent is not named");
    assert_eq!(
        w.name(OTHER_PANE),
        Some("p-captain"),
        "the holder keeps its name"
    );
    assert!(
        w.herdr.renames_to("p-captain").is_empty(),
        "nothing was stolen"
    );

    // One item for the captain, in the captain's own thread — not none.
    let items = w
        .store
        .read(|tx| tx.thread_messages("p", "name:p:captain"))
        .unwrap();
    assert_eq!(
        items.len(),
        1,
        "one item over two passes, not every tick: {items:?}"
    );
    assert_eq!(items[0].recipient, "p:captain");
    assert_eq!(items[0].state, MsgState::Open);
    assert!(items[0].body.contains("p-captain"), "{}", items[0].body);
    assert!(items[0].body.contains(OTHER_PANE), "{}", items[0].body);

    // The holder lets the name go: the captain carries it and its item is
    // resolved.
    w.herdr.set_agent_name(OTHER_PANE, None);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 30).unwrap();
    assert_eq!(w.name(CAP_PANE), Some("p-captain"));
    let items = w
        .store
        .read(|tx| tx.thread_messages("p", "name:p:captain"))
        .unwrap();
    assert_eq!(items[0].state, MsgState::Resolved, "{items:?}");
}

#[test]
fn a_delivery_to_a_pane_whose_agent_lost_its_name_names_it_and_delivers_in_the_same_tick() {
    let mut w = world();
    let task = w.ready_task();

    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    // The rename comes before the delivery, and the delivery went out — one
    // prompt, which the unnamed agent would have refused.
    let renamed_at = w
        .herdr
        .calls
        .iter()
        .position(|call| matches!(call, Call::Rename { name, .. } if name == "p-heat"))
        .unwrap_or_else(|| panic!("no rename: {:?}", w.herdr.calls));
    let prompt_at = w
        .herdr
        .calls
        .iter()
        .position(|call| matches!(call, Call::Prompt { pane, .. } if pane == LANE_PANE))
        .unwrap_or_else(|| panic!("no delivery: {:?}", w.herdr.calls));
    assert!(renamed_at < prompt_at, "named first: {:?}", w.herdr.calls);
    assert_eq!(
        w.herdr
            .calls
            .iter()
            .filter(|call| matches!(call, Call::Prompt { pane, .. } if pane == LANE_PANE))
            .count(),
        1,
        "exactly one prompt: {:?}",
        w.herdr.calls
    );
    assert_eq!(w.name(LANE_PANE), Some("p-heat"));

    let said = w.herdr.prompts_to(LANE_PANE);
    assert!(
        said[0].contains(&task),
        "the task is delivered, not lost to an unnamed agent: {said:?}"
    );
    assert!(
        report
            .executed
            .iter()
            .any(|(_, state)| state == "submitted"),
        "{report:?}"
    );
}

/// T-482: a finished reviewer (Standby) and a seat starting a new review in
/// the same tick each keep their own seat's name; the old task-based name a
/// finished reviewer still carried is given back and nobody's name is taken.
#[test]
fn each_reviewer_keeps_its_seat_name_across_tasks() {
    const REV9_PANE: &str = "w1:p8";
    const REV11_PANE: &str = "w1:p7";
    let mut w = world();
    w.ready_task();
    // review-9's verdict is in: the seat is Standby with no task, its agent
    // still named after the task it reviewed.
    let review9 = w
        .store
        .write(|tx| {
            let seat = testing::seed_active_seat(
                tx,
                "p",
                "review-9",
                SeatRole::Temp,
                REV9_PANE,
                "codex",
                T0,
            )?;
            tx.set_seat_state(&seat.id, SeatState::Standby, Some("verdict in"), T0)?;
            Ok(seat)
        })
        .unwrap();
    // review-11 starts the review of T-1: its agent's name is its seat's,
    // p-review-11.
    w.store
        .write(|tx| testing::seed_temp_seat(tx, "p", "review-11", "T-1", REV11_PANE, "codex", T0))
        .unwrap();
    // review-9 still carries the old task-based name p-review-t1; restore
    // gives it its seat name back. review-11 asks for p-review-11, which
    // nobody holds.
    w.herdr
        .add_agent_named(REV9_PANE, "codex", AgentStatus::Idle, "p-review-t1");
    w.herdr.add_agent(REV11_PANE, "codex", AgentStatus::Idle);

    let names = names::read(&mut w.herdr).unwrap();
    let restored = names::restore(&mut w.store, &mut w.herdr, "p", &names, T0 + 10).unwrap();

    assert!(restored.renamed.contains(&review9.id), "{restored:?}");
    assert!(
        restored.renamed.contains(&"p:review-11".to_string()),
        "{restored:?}"
    );
    assert!(restored.taken.is_empty(), "no name was taken: {restored:?}");
    assert_eq!(
        w.name(REV9_PANE),
        Some("p-review-9"),
        "the finished reviewer's seat name is back"
    );
    assert_eq!(
        w.name(REV11_PANE),
        Some("p-review-11"),
        "the new reviewer got the name it asks for"
    );
    assert!(
        w.herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::Rename { pane, .. } if pane == REV9_PANE)),
        "the finished reviewer is renamed: {:?}",
        w.herdr.calls
    );
    assert!(
        w.herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::Rename { pane, .. } if pane == REV11_PANE)),
        "the new reviewer is named: {:?}",
        w.herdr.calls
    );
    // No item for the captain: there is nothing the next tick would resolve.
    let items = w
        .store
        .read(|tx| tx.thread_messages("p", "name:p:review-11"))
        .unwrap();
    assert!(items.is_empty(), "{items:?}");
}

/// The two passes never hide a real holder: a name a pane of no seat holds is
/// still reported to the captain, once.
#[test]
fn a_name_held_by_a_pane_of_no_seat_is_still_reported_once() {
    const REV11_PANE: &str = "w1:p7";
    let mut w = world();
    w.ready_task();
    w.store
        .write(|tx| testing::seed_temp_seat(tx, "p", "review-11", "T-1", REV11_PANE, "codex", T0))
        .unwrap();
    w.herdr.add_agent(REV11_PANE, "codex", AgentStatus::Idle);
    // The name is held by a pane that is nobody's seat.
    w.herdr
        .add_agent_named(OTHER_PANE, "opencode", AgentStatus::Idle, "p-review-11");

    for now in [T0 + 10, T0 + 20] {
        let names = names::read(&mut w.herdr).unwrap();
        let restored = names::restore(&mut w.store, &mut w.herdr, "p", &names, now).unwrap();
        assert!(
            restored.taken.contains(&"p:review-11".to_string()),
            "{restored:?}"
        );
    }

    assert_eq!(w.name(REV11_PANE), None, "nothing was stolen");
    assert_eq!(w.name(OTHER_PANE), Some("p-review-11"));
    let items = w
        .store
        .read(|tx| tx.thread_messages("p", "name:p:review-11"))
        .unwrap();
    assert_eq!(items.len(), 1, "reported once, not every tick: {items:?}");
    assert_eq!(items[0].state, MsgState::Open);
}
