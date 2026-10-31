//! T-130: a message for a lane reaches it while it works.
//!
//! The engine used to ring a lane only when it is idle, so a lane busy for long
//! stretches never saw the captain's instructions (the trial's deck sat on five
//! for an hour). An urgent message, and every captain instruction to a lane
//! whose agent queues input typed while it works (pi, claude), is typed at once;
//! a normal message to an agent that does not queue (opencode, codex) still
//! waits for idle. One doorbell per message, never repeated.
//!
//! T-454: a notice from the engine waits for the seat's next idle moment and
//! never cuts its turn; one notice is the exception (T-444), the one that
//! closes a direction check under its reviewer, which ends that turn at once.

use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::{Call, FakeHerdr, Inject};
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

    /// A notice from the engine to `to`, open — as the quiet, think and
    /// overrun notices are written (T-454).
    fn notice(&mut self, to: &str, urgent: bool, at: i64) -> String {
        self.store
            .write(|tx| {
                Ok(tx
                    .insert_message(&NewMessage {
                        meta: None,
                        project: "p".into(),
                        thread: format!("quiet-lane:{to}"),
                        task: None,
                        attempt: None,
                        sender: "engine".into(),
                        sender_gen: 0,
                        recipient: to.into(),
                        kind: MsgKind::Instruction,
                        urgent,
                        body: "No push to heat/t1 for 20 min.".into(),
                        reply_to: None,
                        needs_ack: false,
                        state: MsgState::Open,
                        created: at,
                    })?
                    .id)
            })
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

/// T-454: a notice to a working seat is held and reaches it at its next idle
/// moment; it never cuts the running command (no escape), even when it is
/// urgent as the quiet, think and overrun notices are.
#[test]
fn a_notice_to_a_working_lane_waits_for_its_idle_moment() {
    let mut w = world(AgentStatus::Working, AgentStatus::Idle);
    let id = w.notice("p:heat", true, T0 + 10);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();

    assert!(
        w.herdr.prompts_to(PI_PANE).is_empty(),
        "a notice waits while the lane works: {:?}",
        w.herdr.prompts_to(PI_PANE)
    );
    assert!(
        !w.herdr
            .calls
            .iter()
            .any(|c| matches!(c, Call::SendKeys { pane, .. } if pane == PI_PANE)),
        "a notice does not cut the running turn: {:?}",
        w.herdr.calls
    );
    assert!(
        w.doorbell_ops().is_empty(),
        "no doorbell op while the lane works: {:?}",
        w.doorbell_ops()
    );

    // The command is over and the lane is idle: the notice reaches it now.
    w.herdr.set_status(PI_PANE, AgentStatus::Idle);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 40).unwrap();

    let said = w.herdr.prompts_to(PI_PANE);
    assert_eq!(said.len(), 1, "delivered at the next idle moment: {said:?}");
    assert!(
        said[0].contains(&id),
        "the doorbell names the notice: {said:?}"
    );
    let ops = w.doorbell_ops();
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0].state, OpState::Submitted);
}

/// T-454: an urgent captain instruction keeps its path into a working lane
/// (typed in the same tick, its turn cut first), while a notice to another
/// working lane waits for idle.
#[test]
fn an_urgent_captain_instruction_is_not_held() {
    let mut w = world(AgentStatus::Working, AgentStatus::Working);
    let captain_id = w.captain_message("p:wire", MsgKind::Instruction, true, T0 + 10);
    let notice_id = w.notice("p:heat", true, T0 + 10);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();

    let said = w.herdr.prompts_to(LANE_PANE);
    assert_eq!(
        said.len(),
        1,
        "an urgent captain instruction is typed while the lane works: {said:?}"
    );
    assert!(said[0].contains(&captain_id), "{said:?}");
    assert!(
        w.herdr.prompts_to(PI_PANE).is_empty(),
        "the engine's notice to the other working lane is held: {:?}",
        w.herdr.prompts_to(PI_PANE)
    );

    w.herdr.set_status(PI_PANE, AgentStatus::Idle);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 40).unwrap();
    let said = w.herdr.prompts_to(PI_PANE);
    assert_eq!(said.len(), 1, "the held notice arrives at idle: {said:?}");
    assert!(said[0].contains(&notice_id), "{said:?}");
}

// ---- T-454 v2 (F1 of the T-454 review): the direction-closed notice --------

const REVIEW_PANE: &str = "w1:p4";
const WIRE_PANE: &str = "w1:p5";

/// A project with `[temps]`: a reviewer seat holding the direction check of
/// T-1 (its agent working), a second lane whose attempt is Started so the
/// reviewer is not released, and the lane's own seat.
const DIRECTION_CONFIG: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 2
temps = 1

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "test"
owns = []
kind = "codex"
profile = "codex"

[[lanes]]
name = "wire"
duty = "test"
owns = []
kind = "codex"
profile = "codex"

[temps]
kind = "codex"
profile = "codex"

[models]
lanes = [{ model = "test-model", context_window_k = 100 }]
review = [{ model = "review-model", cli = "codex" }]
"#;

/// The direction-closed scenario: T-1's check runs on `review-1`; the lane's
/// attempt 1 ends (rejected, no result), so the check closes under the working
/// reviewer and its seat is freed. T-2 on `wire` stays Started so the freed
/// reviewer is not released in the same tick.
fn direction_world() -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            let mut project = tx.get_project("p")?.unwrap();
            project.config_toml = DIRECTION_CONFIG.into();
            tx.update_project(&project)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PI_PANE, "codex", T0)?;
            testing::seed_active_seat(tx, "p", "wire", SeatRole::Lane, WIRE_PANE, "codex", T0)?;
            testing::seed_temp_seat(tx, "p", "review-1", "T-1", REVIEW_PANE, "codex", T0)?;
            for (id, lane, state) in [
                ("T-1", "heat", gear_model::TaskState::Ready),
                ("T-2", "wire", gear_model::TaskState::Running),
            ] {
                tx.insert_task(&gear_store::TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: id.into(),
                    rank: if id == "T-1" { 1 } else { 2 },
                    state,
                    priority: "must".into(),
                    lane: Some(lane.into()),
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
                    updated: T0,
                })?;
                tx.insert_contract(&gear_store::ContractRow {
                    project: "p".into(),
                    task: id.into(),
                    version: 1,
                    goal: "the goal".into(),
                    accept: "the accept".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                    mechanical: None,
                })?;
            }
            // T-1's attempt 1 ran, its direction check is still running, and
            // the attempt ended without a result (a rejected hand-in).
            tx.insert_attempt(&gear_store::AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: gear_model::AttemptState::Rejected,
                candidate: Some("abc1234".into()),
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
            tx.set_meta(
                "engine.direction.p.T-1.1",
                &serde_json::json!({
                    "state": "running",
                    "seat": "p:review-1",
                    "sha": "abc1234",
                    "generation": 1
                })
                .to_string(),
            )?;
            // T-2 keeps a lane Started, so the freed reviewer stays in Standby.
            tx.insert_attempt(&gear_store::AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-2".into(),
                n: 1,
                seat: "p:wire".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: gear_model::AttemptState::Started,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(PI_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(WIRE_PANE, "codex", AgentStatus::Working);
    herdr.add_agent(REVIEW_PANE, "codex", AgentStatus::Working);
    let cfg = EngineCfg {
        tick_s: 4,
        cooldown_s: 30,
        doorbell_check_budget: 3,
        profiles: gear_herdr::builtins(),
        ..Default::default()
    };
    World { store, herdr, cfg }
}

/// T-454 v2 (T-444, F1 of the T-454 review): a full tick closes a direction
/// check under a working reviewer, frees its seat, and reaches it with the
/// notice — the escape goes before the notice text, so the reviewer ends its
/// turn at once.
#[test]
fn the_direction_closed_notice_reaches_a_working_reviewer_in_the_tick() {
    let mut w = direction_world();

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    let seat = w
        .store
        .read(|tx| tx.get_seat("p:review-1"))
        .unwrap()
        .unwrap();
    assert_eq!(seat.state, gear_model::SeatState::Standby, "{seat:?}");
    assert_eq!(seat.task_id, None, "the check's job is cleared: {seat:?}");
    let notice = w
        .store
        .read(|tx| tx.thread_messages("p", "direction:T-1:1:abc1234"))
        .unwrap()
        .into_iter()
        .find(|m| {
            m.sender == "engine"
                && m.recipient == "p:review-1"
                && gear_store::is_direction_closed_notice(m)
        })
        .expect("the closed check tells its seat");

    let esc = w.herdr.calls.iter().position(|c| {
        matches!(c, Call::SendKeys { pane, keys } if pane == REVIEW_PANE && keys == &["esc".to_string()])
    });
    let prompt = w
        .herdr
        .calls
        .iter()
        .position(|c| matches!(c, Call::Prompt { pane, .. } if pane == REVIEW_PANE));
    let (Some(esc), Some(prompt)) = (esc, prompt) else {
        panic!("the reviewer is not reached: {:?}", w.herdr.calls);
    };
    assert!(esc < prompt, "the escape goes before the notice");
    assert!(
        w.herdr.prompts_to(REVIEW_PANE)[0].contains(&notice.id),
        "the doorbell names the notice: {:?}",
        w.herdr.prompts_to(REVIEW_PANE)
    );
}
