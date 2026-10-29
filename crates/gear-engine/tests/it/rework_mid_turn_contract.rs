//! T-290: a rework packet reaches a lane that queues input while it still
//! works (pi, claude). The fail verdict leaves the attempt Rejected and the
//! task Ready with a rework note; the lane's agent is still working on the
//! previous step, and a pi or claude agent reads text typed mid-turn after
//! that step. A first packet, a lane with other work in flight and an
//! opencode or codex lane wait for idle as before.

use gear_app::tasks::{settle, start_attempt, submit_result, Verdict};
use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::{Call, FakeHerdr};
use gear_herdr::AgentStatus;
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const PANE: &str = "w1:p2";

fn task(id: &str) -> TaskRow {
    TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
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
    }
}

fn contract(id: &str) -> ContractRow {
    ContractRow {
        project: "p".into(),
        task: id.into(),
        version: 1,
        goal: "g".into(),
        accept: "a".into(),
        evidence_files: vec![],
        evidence_due_min: None,
        body: String::new(),
        created: T0,
        mechanical: None,
    }
}

/// The fail verdict the captain gave on T-1 attempt 1: the attempt is
/// Rejected, the task Ready with the review's findings as its rework note.
fn fail_rework(tx: &gear_store::Tx<'_>) -> gear_model::GearResult<()> {
    settle(
        tx,
        &Verdict {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            contract_version: 1,
            candidate: "abc1234".into(),
            pass: false,
            evidence: None,
            rework: Some("the check is missing".into()),
            integrated_by: None,
            by_seat: "p:captain".into(),
        },
        T0 + 2,
    )
}

/// A project with a captain and the lane `heat` of `kind`; T-1 ran here, was
/// handed in and failed through `settle` — its rework is waiting.
fn world(kind: &str) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, kind, T0)?;
            tx.insert_task(&task("T-1"))?;
            tx.insert_contract(&contract("T-1"))?;
            start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
            // The attempt ran in the session the seat still runs on (L2): the
            // rework continues it instead of opening a new conversation, so
            // the packet is ready the same tick.
            tx.set_binding_session_ref("p:heat", 1, "sess-a")?;
            tx.set_attempt_session("p", "T-1", 1, "sess-a", kind)?;
            submit_result(tx, "p", "T-1", 1, 1, "abc1234", "Report: r", T0 + 1)?;
            fail_rework(tx)?;
            Ok(())
        })
        .unwrap();
    store
}

fn cfg() -> EngineCfg {
    EngineCfg {
        tick_s: 4,
        cooldown_s: 30,
        doorbell_check_budget: 3,
        profiles: gear_herdr::builtins(),
        ..Default::default()
    }
}

/// Herdr with the lane's agent present in `status`.
fn herdr(kind: &str, status: AgentStatus) -> FakeHerdr {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p1", "codex", AgentStatus::Idle);
    herdr.add_agent(PANE, kind, status);
    herdr
}

#[test]
fn a_working_pi_lane_gets_its_rework_packet_in_the_next_tick() {
    let mut store = world("pi");
    let mut herdr = herdr("pi", AgentStatus::Working);

    tick(&mut store, &mut herdr, &cfg(), T0 + 10).unwrap();

    let attempt = store
        .read(|tx| tx.get_attempt("p", "T-1", 2))
        .unwrap()
        .expect("the rework's attempt 2 is opened");
    assert_eq!(attempt.state, AttemptState::Delivered);
    assert!(attempt.sent_at.is_some(), "the packet went: {attempt:?}");
    let said = herdr.prompts_to(PANE);
    assert_eq!(said.len(), 1, "one packet, typed mid-turn: {said:?}");
    assert!(said[0].contains("T-1"), "{said:?}");
    assert!(
        !herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::SendKeys { keys, .. } if keys.iter().any(|key| key == "esc"))),
        "the delivery does not interrupt: {:?}",
        herdr.calls
    );
}

#[test]
fn a_working_opencode_lane_waits_for_idle_with_its_rework() {
    let mut store = world("opencode");
    let mut herdr = herdr("opencode", AgentStatus::Working);

    tick(&mut store, &mut herdr, &cfg(), T0 + 10).unwrap();

    assert!(
        store.read(|tx| tx.get_attempt("p", "T-1", 2)).unwrap().is_none(),
        "an opencode turn takes the text as keystrokes: the rework waits"
    );
    assert!(herdr.prompts_to(PANE).is_empty(), "{:?}", herdr.prompts_to(PANE));
}

#[test]
fn a_working_pi_lane_waits_for_a_new_task() {
    // No rework: the ready task never ran here, so it is a first packet.
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T0)?;
            tx.insert_task(&task("T-9"))?;
            tx.insert_contract(&contract("T-9"))?;
            Ok(())
        })
        .unwrap();
    let mut herdr = herdr("pi", AgentStatus::Working);

    tick(&mut store, &mut herdr, &cfg(), T0 + 10).unwrap();

    assert!(
        store.read(|tx| tx.get_attempt("p", "T-9", 1)).unwrap().is_none(),
        "a first packet waits for idle"
    );
    assert!(herdr.prompts_to(PANE).is_empty(), "{:?}", herdr.prompts_to(PANE));
}

#[test]
fn an_idle_pi_lane_gets_its_rework_packet_as_before() {
    let mut store = world("pi");
    let mut herdr = herdr("pi", AgentStatus::Idle);

    tick(&mut store, &mut herdr, &cfg(), T0 + 10).unwrap();

    assert_eq!(herdr.prompts_to(PANE).len(), 1, "{:?}", herdr.prompts_to(PANE));
    assert!(store.read(|tx| tx.get_attempt("p", "T-1", 2)).unwrap().is_some());
}
