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
    // Three nudges, then the captain is told.
    for at in [130, 140, 150] {
        tick(&mut s, &mut h, &c, T + at).unwrap();
    }
    assert_eq!(nudges(&mut s).len(), 3, "{:?}", nudges(&mut s));
    assert!(stall_items(&mut s).is_empty(), "the captain hears nothing yet");

    tick(&mut s, &mut h, &c, T + 160).unwrap();
    assert_eq!(nudges(&mut s).len(), 3, "no fourth nudge: {:?}", nudges(&mut s));
    let items = stall_items(&mut s);
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].contains("heat"), "{items:?}");

    // And the item is not repeated.
    tick(&mut s, &mut h, &c, T + 170).unwrap();
    assert_eq!(stall_items(&mut s).len(), 1, "one item only");
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
