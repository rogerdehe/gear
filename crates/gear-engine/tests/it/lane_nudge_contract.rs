//! T-206: a lane that stopped on a server error goes on by itself.
use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{SeatRole, SeatState, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T: i64 = 1_800_000_000;
const LANE: &str = "w1:p2";

/// A project with a lane whose agent is idle and an attempt in flight.
fn world() -> (Store, FakeHerdr, EngineCfg) {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T)?;
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
            estimate_min: Some(30),
            issue: None,
            updated: T,
        })?;
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-1".into(),
            version: 1,
            goal: "the work".into(),
            accept: "done".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T,
            mechanical: None,
        })?;
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
        Ok(())
    })
    .unwrap();
    let mut h = FakeHerdr::new();
    h.add_agent("w1:p1", "codex", AgentStatus::Idle);
    h.add_agent(LANE, "opencode", AgentStatus::Idle);
    let mut cfg = EngineCfg::default();
    cfg.profiles = gear_herdr::builtins();
    cfg.cooldown_s = 0;
    // A long delivery grace: the delivery of the packet is not the story of
    // these tests, the idle lane is.
    cfg.delivery_grace_s = 3600;
    (s, h, cfg)
}

/// The nudges the lane got: the text of every delivery that carries one.
fn nudges(s: &mut Store) -> Vec<String> {
    s.read(|tx| tx.ops_of_seat_kind("p:heat", gear_model::OpKind::Deliver))
        .unwrap()
        .into_iter()
        .filter(|op| op.payload.get("nudge").is_some())
        .filter_map(|op| op.payload.get("text").and_then(|t| t.as_str()).map(str::to_string))
        .collect()
}

/// The captain's items on the lane's stall thread.
fn stall_items(s: &mut Store) -> Vec<String> {
    s.read(|tx| tx.thread_messages("p", "stall:p:heat"))
        .unwrap()
        .into_iter()
        .map(|message| message.body)
        .collect()
}

#[test]
fn an_idle_lane_is_nudged_after_two_minutes() {
    let (mut s, mut h, c) = world();

    // The first idle tick only marks it.
    tick(&mut s, &mut h, &c, T + 10).unwrap();
    assert!(nudges(&mut s).is_empty(), "no nudge before the two minutes");

    // Two minutes later the lane is told to go on.
    tick(&mut s, &mut h, &c, T + 130).unwrap();
    let nudges = nudges(&mut s);
    assert_eq!(nudges.len(), 1, "{nudges:?}");
    assert!(
        nudges[0].contains("Continue T-1: your last turn stopped"),
        "{:?}",
        nudges[0]
    );
    assert!(nudges[0].contains("model at capacity"), "{:?}", nudges[0]);
    assert!(stall_items(&mut s).is_empty(), "the captain hears nothing yet");
}

#[test]
fn a_working_lane_is_not_nudged() {
    let (mut s, mut h, c) = world();
    h.set_status(LANE, AgentStatus::Working);

    tick(&mut s, &mut h, &c, T + 10).unwrap();
    tick(&mut s, &mut h, &c, T + 300).unwrap();

    assert!(nudges(&mut s).is_empty(), "{:?}", nudges(&mut s));
}

#[test]
fn a_parked_lane_is_not_nudged() {
    let (mut s, mut h, c) = world();
    s.write(|tx| {
        tx.set_seat_state("p:heat", SeatState::Parked, Some("until=2027-01-15T09:00:00Z"), T)
    })
    .unwrap();

    tick(&mut s, &mut h, &c, T + 10).unwrap();
    tick(&mut s, &mut h, &c, T + 300).unwrap();

    assert!(nudges(&mut s).is_empty(), "{:?}", nudges(&mut s));
}

#[test]
fn a_lane_without_an_attempt_in_flight_is_not_nudged() {
    let (mut s, mut h, c) = world();
    // The attempt is over: the lane has nothing in flight.
    s.write(|tx| {
        let mut attempt = tx.get_attempt("p", "T-1", 1)?.expect("the attempt");
        attempt.state = gear_model::AttemptState::Accepted;
        tx.update_attempt(&attempt)
    })
    .unwrap();

    tick(&mut s, &mut h, &c, T + 10).unwrap();
    tick(&mut s, &mut h, &c, T + 300).unwrap();

    assert!(nudges(&mut s).is_empty(), "{:?}", nudges(&mut s));
}

#[test]
fn a_lane_that_keeps_stopping_ends_with_one_captain_item() {
    let (mut s, mut h, c) = world();

    tick(&mut s, &mut h, &c, T + 10).unwrap();
    // One nudge per idle episode: the next nudge waits a full interval
    // (F2), so the episodes are two minutes apart.
    for at in [130, 250, 370] {
        tick(&mut s, &mut h, &c, T + at).unwrap();
    }
    assert_eq!(nudges(&mut s).len(), 3, "{:?}", nudges(&mut s));
    assert!(stall_items(&mut s).is_empty(), "the captain hears nothing yet: {:?}", stall_items(&mut s));

    tick(&mut s, &mut h, &c, T + 490).unwrap();
    assert_eq!(nudges(&mut s).len(), 3, "no fourth nudge: {:?}", nudges(&mut s));
    let items = stall_items(&mut s);
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].contains("heat"), "{items:?}");

    // And the item is not repeated.
    tick(&mut s, &mut h, &c, T + 610).unwrap();
    assert_eq!(stall_items(&mut s).len(), 1, "one item only");
}

/// F2: a nudge waits a full interval: the tick right after it sends nothing.
#[test]
fn a_nudge_waits_a_full_interval_before_the_next_one() {
    let (mut s, mut h, c) = world();

    tick(&mut s, &mut h, &c, T + 10).unwrap();
    tick(&mut s, &mut h, &c, T + 130).unwrap();
    assert_eq!(nudges(&mut s).len(), 1, "{:?}", nudges(&mut s));

    // The tick right after the nudge: nothing goes out.
    tick(&mut s, &mut h, &c, T + 131).unwrap();
    assert_eq!(nudges(&mut s).len(), 1, "one nudge only: {:?}", nudges(&mut s));

    // A full interval later: the next nudge.
    tick(&mut s, &mut h, &c, T + 260).unwrap();
    assert_eq!(nudges(&mut s).len(), 2, "{:?}", nudges(&mut s));
}

/// F2: a delivery that is still in flight blocks the nudge: the lane gets
/// one packet, not two.
#[test]
fn a_delivery_in_flight_blocks_the_nudge() {
    let (mut s, mut h, c) = world();
    // The lane's packet is Unknown: it may yet get through, so no second
    // packet may go out.
    s.write(|tx| {
        let op = tx.insert_op(&gear_store::NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: gear_model::OpKind::Deliver,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({ "text": "the packet" }),
            created: T,
        })?;
        tx.set_op_state("p", &op.id, gear_model::OpState::Unknown, Some("the engine stopped"), T)
    })
    .unwrap();

    tick(&mut s, &mut h, &c, T + 10).unwrap();
    tick(&mut s, &mut h, &c, T + 130).unwrap();

    assert!(nudges(&mut s).is_empty(), "{:?}", nudges(&mut s));
}

#[test]
fn a_lane_on_its_quota_is_not_nudged() {
    let (mut s, mut h, c) = world();
    // The screen shows the provider's limit: a nudge cannot fix it.
    h.set_screen(LANE, "  Error: Token Plan ... (2056)\n  Session failed");

    tick(&mut s, &mut h, &c, T + 10).unwrap();
    tick(&mut s, &mut h, &c, T + 300).unwrap();

    assert!(nudges(&mut s).is_empty(), "{:?}", nudges(&mut s));
}

/// F3: the packet of a delivery that was never confirmed is delivered again,
/// once, the same way.
#[test]
fn the_first_failure_waits_for_the_retry() {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T)?;
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
            estimate_min: Some(30),
            issue: None,
            updated: T,
        })?;
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
        // The packet was sent and never confirmed.
        let op = tx.insert_op(&gear_store::NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: gear_model::OpKind::Deliver,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({ "text": "the packet" }),
            created: T,
        })?;
        tx.set_op_state("p", &op.id, gear_model::OpState::Submitted, None, T)
    })
    .unwrap();
    let mut h = FakeHerdr::new();
    h.add_agent("w1:p1", "codex", AgentStatus::Idle);
    h.add_agent(LANE, "opencode", AgentStatus::Idle);
    let mut cfg = EngineCfg::default();
    cfg.profiles = gear_herdr::builtins();
    cfg.cooldown_s = 0;
    // A short grace: the delivery fails at once.
    cfg.delivery_grace_s = 1;

    // The first failure.
    tick(&mut s, &mut h, &cfg, T + 10).unwrap();
    // T-218: the captain is not rung yet: the one automatic retry of the
    // lane's own path may still come.
    assert!(
        stall_items(&mut s).is_empty(),
        "the captain hears nothing before the retry: {:?}",
        stall_items(&mut s)
    );

    // Two minutes later the packet is delivered again.
    tick(&mut s, &mut h, &cfg, T + 130).unwrap();
    let redelivered = s
        .read(|tx| tx.ops_of_seat_kind("p:heat", gear_model::OpKind::Deliver))
        .unwrap()
        .into_iter()
        .filter(|op| op.payload.get("redeliver").is_some())
        .count();
    assert_eq!(redelivered, 1, "the packet goes out again once");
    // The retry is on its way: the captain still hears nothing.
    assert!(
        stall_items(&mut s).is_empty(),
        "the captain hears nothing while the retry is on its way: {:?}",
        stall_items(&mut s)
    );

    // The retry fails too: the captain hears about it, once.
    tick(&mut s, &mut h, &cfg, T + 140).unwrap();
    let items = stall_items(&mut s);
    assert_eq!(items.len(), 1, "{items:?}");

    // And the item is not repeated.
    tick(&mut s, &mut h, &cfg, T + 160).unwrap();
    assert_eq!(stall_items(&mut s).len(), 1, "one item only");
}

/// The world of the retry tests: a lane whose packet was sent and never
/// confirmed, so the delivery fails at once.
fn failed_packet_world() -> (Store, FakeHerdr, EngineCfg) {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T)?;
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
            estimate_min: Some(30),
            issue: None,
            updated: T,
        })?;
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
        // The packet was sent and never confirmed.
        let op = tx.insert_op(&gear_store::NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: gear_model::OpKind::Deliver,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({ "text": "the packet" }),
            created: T,
        })?;
        tx.set_op_state("p", &op.id, gear_model::OpState::Submitted, None, T)
    })
    .unwrap();
    let mut h = FakeHerdr::new();
    h.add_agent("w1:p1", "codex", AgentStatus::Idle);
    h.add_agent(LANE, "opencode", AgentStatus::Idle);
    let cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        cooldown_s: 0,
        // A short grace: the delivery fails at once.
        delivery_grace_s: 1,
        ..Default::default()
    };
    (s, h, cfg)
}

/// T-218 (F1): a retry that fails while it is sent — the agent of the pane
/// was gone between the observation and the execution — never reaches the
/// confirmation window. It spends its marker and opens the one stall item,
/// the same as a retry that times out.
#[test]
fn a_retry_that_fails_while_it_is_sent_opens_the_stall_item() {
    let (mut s, mut h, cfg) = failed_packet_world();

    // The first failure: the captain is not rung yet.
    tick(&mut s, &mut h, &cfg, T + 10).unwrap();
    assert!(
        stall_items(&mut s).is_empty(),
        "the captain hears nothing before the retry: {:?}",
        stall_items(&mut s)
    );

    // T+130: the retry is planned, but the agent of the pane exits before
    // the execution: the retry fails at once (agent-gone).
    let agents = gear_engine::observe::snapshot(&mut h).unwrap();
    let observed = s
        .read(|tx| gear_engine::observe::reconcile(tx, &mut h, "p", &agents, &cfg.profiles))
        .unwrap();
    gear_engine::lane_nudge::nudge_stopped_lanes(&mut s, "p", &observed, T + 130).unwrap();
    h.add_shell(LANE);
    gear_engine::ops::execute_intended(&mut s, &mut h, "p", T + 130).unwrap();
    // The replacement agent is there before the next observation.
    h.add_agent(LANE, "opencode", AgentStatus::Idle);

    // Exactly one redelivery, and it failed.
    let retries: Vec<_> = s
        .read(|tx| tx.ops_of_seat_kind("p:heat", gear_model::OpKind::Deliver))
        .unwrap()
        .into_iter()
        .filter(|op| op.payload.get("redeliver").is_some())
        .collect();
    assert_eq!(retries.len(), 1, "the packet goes out again once");
    assert_eq!(retries[0].state, gear_model::OpState::Failed, "{:?}", retries[0]);
    assert!(
        retries[0].result.as_deref().unwrap_or("").contains("agent-gone"),
        "{:?}",
        retries[0].result
    );

    // The captain hears about it: the marker is spent and the item is open.
    tick(&mut s, &mut h, &cfg, T + 140).unwrap();
    let items = stall_items(&mut s);
    assert_eq!(items.len(), 1, "{items:?}");
    let marker = s
        .read(|tx| tx.get_meta(&gear_engine::lane_nudge::retry_marker("p:heat", "T-1", 1)))
        .unwrap();
    assert_eq!(marker.as_deref(), Some(""), "the marker is spent: {marker:?}");

    // And the item is not repeated.
    tick(&mut s, &mut h, &cfg, T + 160).unwrap();
    tick(&mut s, &mut h, &cfg, T + 1000).unwrap();
    assert_eq!(stall_items(&mut s).len(), 1, "one item only");
}

/// A late confirmed check-in retires the failed retry without a stale alert.
#[test]
fn a_failed_retry_followed_by_a_check_in_does_not_open_a_stall() {
    let (mut s, mut h, cfg) = failed_packet_world();
    tick(&mut s, &mut h, &cfg, T + 10).unwrap();
    let agents = gear_engine::observe::snapshot(&mut h).unwrap();
    let observed = s.read(|tx| gear_engine::observe::reconcile(tx, &mut h, "p", &agents, &cfg.profiles)).unwrap();
    gear_engine::lane_nudge::nudge_stopped_lanes(&mut s, "p", &observed, T + 130).unwrap();
    h.add_shell(LANE);
    gear_engine::ops::execute_intended(&mut s, &mut h, "p", T + 130).unwrap();
    h.add_agent(LANE, "opencode", AgentStatus::Idle);
    s.write(|tx| tx.insert_view(&gear_store::ViewRecord {
        project: "p".into(), seat: "p:heat".into(), generation: 1, ts: T + 135,
        shown_msgs: vec![], shown_ops: vec![], shown_task: Some("T-1".into()),
        shown_attempt: Some(1), shown_contract: Some(1),
    })).unwrap();
    h.set_status(LANE, AgentStatus::Working);
    for time in [140, 160, 1000] {
        tick(&mut s, &mut h, &cfg, T + time).unwrap();
        assert!(stall_items(&mut s).is_empty(), "stale stall at T+{time}");
        assert_eq!(s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap().state, gear_model::AttemptState::Started);
    }
    assert_eq!(s.read(|tx| tx.get_meta(&gear_engine::lane_nudge::retry_marker("p:heat", "T-1", 1))).unwrap().as_deref(), Some(""));
    assert!(s.read(|tx| gear_engine::delivery::failed(tx, "p:heat")).unwrap().is_none());
}

/// T-218: the same ordering with the default confirmation window: the
/// captain is not rung before the retry, and the retry's own window opens the
/// one item.
#[test]
fn the_first_failure_waits_for_the_retry_with_the_default_window() {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T)?;
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
            estimate_min: Some(30),
            issue: None,
            updated: T,
        })?;
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
        let op = tx.insert_op(&gear_store::NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: gear_model::OpKind::Deliver,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({ "text": "the packet" }),
            created: T,
        })?;
        tx.set_op_state("p", &op.id, gear_model::OpState::Submitted, None, T)
    })
    .unwrap();
    let mut h = FakeHerdr::new();
    h.add_agent("w1:p1", "codex", AgentStatus::Idle);
    h.add_agent(LANE, "opencode", AgentStatus::Idle);
    let cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        cooldown_s: 0,
        delivery_grace_s: 180,
        ..Default::default()
    };
    // The default window: the delivery fails 180 s after it was sent.
    assert_eq!(cfg.delivery_grace_s, 180, "the default window");

    // The window is over: the delivery fails. The captain is not rung: the
    // one automatic retry may still come.
    tick(&mut s, &mut h, &cfg, T + 200).unwrap();
    assert!(
        stall_items(&mut s).is_empty(),
        "the captain hears nothing before the retry: {:?}",
        stall_items(&mut s)
    );

    // The retry is on its way: still nothing.
    tick(&mut s, &mut h, &cfg, T + 320).unwrap();
    assert!(
        stall_items(&mut s).is_empty(),
        "the captain hears nothing while the retry is on its way: {:?}",
        stall_items(&mut s)
    );

    // The retry's own window ends unconfirmed: the captain hears about it,
    // once.
    tick(&mut s, &mut h, &cfg, T + 520).unwrap();
    let items = stall_items(&mut s);
    assert_eq!(items.len(), 1, "{items:?}");
}

/// T-218: a delivery that cannot get the one automatic retry of the lane's own
/// path (the attempt already started: the lane read the packet) reaches the
/// captain at once — nothing holds the fact back.
#[test]
fn a_delivery_without_the_automatic_retry_is_flagged_at_once() {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T)?;
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
            estimate_min: Some(30),
            issue: None,
            updated: T,
        })?;
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
        // The attempt already started: the lane's own path has no retry for
        // it.
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
        let op = tx.insert_op(&gear_store::NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: gear_model::OpKind::Deliver,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({ "text": "the packet" }),
            created: T,
        })?;
        tx.set_op_state("p", &op.id, gear_model::OpState::Submitted, None, T)
    })
    .unwrap();
    let mut h = FakeHerdr::new();
    h.add_agent("w1:p1", "codex", AgentStatus::Idle);
    h.add_agent(LANE, "opencode", AgentStatus::Idle);
    let cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        cooldown_s: 0,
        delivery_grace_s: 1,
        ..Default::default()
    };

    tick(&mut s, &mut h, &cfg, T + 10).unwrap();

    assert_eq!(
        stall_items(&mut s).len(),
        1,
        "the captain hears about a delivery that cannot be retried: {:?}",
        stall_items(&mut s)
    );
}
