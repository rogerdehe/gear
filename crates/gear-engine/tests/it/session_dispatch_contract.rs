//! T-153: dispatch waits for session preparation, and recovers open rounds.
use gear_engine::{tick, EngineCfg};
use gear_herdr::{
    fake::{Call, FakeHerdr},
    AgentStatus,
};
use gear_model::{SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T: i64 = 1_800_000_000;
const PANE: &str = "w1:p2";

fn world(followup: bool) -> (Store, FakeHerdr, EngineCfg) {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "opencode", T)?;
        tx.set_binding_session_ref("p:heat", 1, "old")?;
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
            followup_of: followup.then(|| "T-7".into()),
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
            goal: "goal".into(),
            accept: "accept".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T,
            mechanical: None,
        })?;
        Ok(())
    })
    .unwrap();
    let mut h = FakeHerdr::new();
    h.add_agent_named("w1:p1", "codex", AgentStatus::Idle, "p-captain");
    h.add_agent_named(PANE, "opencode", AgentStatus::Idle, "p-heat");
    h.set_agent_session(PANE, "old");
    let mut cfg = EngineCfg::default();
    cfg.cooldown_s = 0;
    cfg.profiles = gear_herdr::builtins();
    let profile = cfg.profiles.get_mut("opencode").unwrap();
    profile.new_session = "/new".into();
    profile.resume_args = Some(vec!["--session".into(), "{session}".into()]);
    (s, h, cfg)
}
fn advance(s: &mut Store, h: &mut FakeHerdr, c: &EngineCfg, t: i64) {
    tick(s, h, c, T + t).unwrap();
}

/// A project with one lane of its own name (`probe`), so a test that reads the
/// engine log (T-407) never mixes in another test's lines.
fn probe_world() -> (Store, FakeHerdr, EngineCfg) {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
        testing::seed_active_seat(tx, "p", "probe", SeatRole::Lane, "w1:p9", "opencode", T)?;
        tx.set_binding_session_ref("p:probe", 1, "old")?;
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
            lane: Some("probe".into()),
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
            goal: "goal".into(),
            accept: "accept".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T,
            mechanical: None,
        })?;
        Ok(())
    })
    .unwrap();
    let mut h = FakeHerdr::new();
    h.add_agent_named("w1:p1", "codex", AgentStatus::Idle, "p-captain");
    h.add_agent_named("w1:p9", "opencode", AgentStatus::Idle, "p-probe");
    h.set_agent_session("w1:p9", "old");
    let mut cfg = EngineCfg::default();
    cfg.cooldown_s = 0;
    cfg.profiles = gear_herdr::builtins();
    let profile = cfg.profiles.get_mut("opencode").unwrap();
    profile.new_session = "/new".into();
    (s, h, cfg)
}

/// T-407: a session switch that reaches `done` writes one probe line per step
/// and one total, in milliseconds, from the durable step times.
#[test]
fn a_session_switch_to_done_logs_one_probe_line_per_step_and_one_total() {
    let (mut s, mut h, c) = probe_world();
    advance(&mut s, &mut h, &c, 0);
    assert!(
        h.calls
            .iter()
            .any(|x| matches!(x, Call::PaneType { text, .. } if text == "/new")),
        "the switch went: {:?}",
        h.calls
    );
    advance(&mut s, &mut h, &c, 1);
    h.set_agent_session("w1:p9", "new");
    advance(&mut s, &mut h, &c, 7);
    let log: Vec<String> = gear_engine::ops::probe_log()
        .into_iter()
        .filter(|line| line.starts_with("probe new p:probe "))
        .collect();
    let steps: Vec<&str> = log
        .iter()
        .map(|line| line.split_whitespace().nth(3).unwrap())
        .collect();
    // T-434: the typed command and its Enter leave in one step, so the switch
    // goes from `initial` straight to `wait_action`.
    assert_eq!(steps, ["initial", "wait_action", "done"], "{log:?}");
    // Each step carries its own time and the switch's time so far: the typed
    // `/new` with its Enter (tick 0), then the wait to tick 7.
    assert_eq!(log[0], "probe new p:probe initial 0 total=0");
    assert_eq!(log[1], "probe new p:probe wait_action 7000 total=7000");
    assert_eq!(
        log[2], "probe new p:probe done 7000 total=7000",
        "one total: the whole switch"
    );
}

#[test]
fn waits_then_sends_once_records_sent_and_names_followup() {
    let (mut s, mut h, c) = world(true);
    advance(&mut s, &mut h, &c, 0);
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert!(a.sent_at.is_none());
    assert!(h.prompts_to(PANE).is_empty());
    assert!(h
        .calls
        .iter()
        .any(|x| matches!(x,Call::PaneType { text,.. } if text=="/new")));
    advance(&mut s, &mut h, &c, 1);
    assert!(h.prompts_to(PANE).is_empty());
    h.set_agent_session(PANE, "new");
    advance(&mut s, &mut h, &c, 7);
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(a.sent_at, Some(T + 7));
    let prompts = h.prompts_to(PANE);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("This task follows T-7; read its report: gear --show T-7"));
}

#[test]
fn open_round_with_dead_agent_starts_with_resume_arguments() {
    let (mut s, mut h, c) = world(false);
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
        let key = gear_store::RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
        };
        tx.insert_round(&gear_store::NewRound {
            key: key.clone(),
            seat: "p:heat".into(),
            generation: 1,
            cli: "opencode".into(),
            model: None,
            start_at: T,
        })?;
        tx.add_round_session(&key, "old")?;
        Ok(())
    })
    .unwrap();
    h.add_shell(PANE);
    advance(&mut s, &mut h, &c, 1);
    advance(&mut s, &mut h, &c, 2);
    assert!(h.calls.iter().any(|x| matches!(x,Call::AgentStart { pane,args,.. } if pane==PANE && args.windows(2).any(|w| w==["--session","old"]))),"{:?}",h.calls);
    assert_eq!(s.read(|tx| tx.rounds_of_task("p", "T-1")).unwrap().len(), 1);
}

#[test]
fn refused_packet_does_not_record_sent_at() {
    let (mut s, mut h, c) = world(false);
    advance(&mut s, &mut h, &c, 0);
    advance(&mut s, &mut h, &c, 1);
    h.set_agent_session(PANE, "new");
    h.inject_next = Some(gear_herdr::fake::Inject::Refuse {
        code: "refused".into(),
    });
    advance(&mut s, &mut h, &c, 7);
    assert!(s
        .read(|tx| tx.get_attempt("p", "T-1", 1))
        .unwrap()
        .unwrap()
        .sent_at
        .is_none());
}

#[test]
fn freshly_onboarded_generation_uses_its_first_session() {
    let (mut s, mut h, c) = world(false);
    s.write(|tx| tx.set_meta("engine.session_fresh.p:heat", "1"))
        .unwrap();
    advance(&mut s, &mut h, &c, 0);
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(a.session_action.as_deref(), Some("fresh"));
    assert_eq!(a.sent_at, Some(T));
    assert!(!h
        .calls
        .iter()
        .any(|x| matches!(x,Call::PaneType {text,..} if text=="/new")));
    assert_eq!(
        s.read(|tx| tx.get_meta("engine.session_fresh.p:heat"))
            .unwrap()
            .as_deref(),
        Some("")
    );
}

#[test]
fn freshly_onboarded_followup_still_resumes_its_earlier_session() {
    let (mut s, mut h, c) = world(true);
    s.write(|tx| {
        tx.set_meta("engine.session_fresh.p:heat", "1")?;
        tx.insert_attempt(&gear_store::AttemptRow {
            session_ref: Some("previous".into()),
            session_cli: Some("opencode".into()),
            session_action: None,
            sent_at: Some(T - 100),
            project: "p".into(),
            task: "T-7".into(),
            n: 1,
            seat: "p:heat".into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state: gear_model::AttemptState::Accepted,
            candidate: None,
            report: None,
            safe_point: None,
            started: T - 100,
            updated: T - 50,
        })
    })
    .unwrap();
    advance(&mut s, &mut h, &c, 0);
    let state = s
        .read(|tx| tx.get_meta("engine.session_step.p:heat"))
        .unwrap()
        .unwrap();
    let state: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["action"], "resume");
    assert_eq!(state["target"], "previous");
    assert!(h.prompts_to(PANE).is_empty());
}

#[test]
fn urgent_doorbell_waits_for_session_preparation_and_delivery() {
    use gear_model::{MsgKind, MsgState};
    use gear_store::{NewMessage, ViewRecord};
    let (mut s, mut h, c) = world(false);
    s.write(|tx| {
        tx.insert_message(&NewMessage {
            project: "p".into(),
            thread: "instruction".into(),
            task: None,
            attempt: None,
            sender: "p:captain".into(),
            sender_gen: 1,
            recipient: "p:heat".into(),
            kind: MsgKind::Instruction,
            urgent: true,
            body: "please read".into(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: T,
            meta: None,
        })
        .map(|_| ())
    })
    .unwrap();
    advance(&mut s, &mut h, &c, 0);
    advance(&mut s, &mut h, &c, 1);
    assert!(h.prompts_to(PANE).is_empty(), "{:?}", h.calls);
    h.set_agent_session(PANE, "new");
    advance(&mut s, &mut h, &c, 7);
    assert_eq!(h.prompts_to(PANE).len(), 1);
    assert!(h.prompts_to(PANE)[0].contains("New task T-1"));
    s.write(|tx| {
        tx.insert_view(&ViewRecord {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            ts: T + 8,
            shown_msgs: vec![],
            shown_ops: vec![],
            shown_task: Some("T-1".into()),
            shown_attempt: Some(1),
            shown_contract: Some(1),
        })
    })
    .unwrap();
    advance(&mut s, &mut h, &c, 9);
    assert!(
        h.prompts_to(PANE)
            .iter()
            .any(|p| p.contains("item(s) waiting")),
        "{:?}",
        h.calls
    );
}

#[test]
fn recovering_existing_round_consumes_fresh_marker_before_unrelated_task() {
    let (mut s, mut h, c) = world(false);
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
        let key = gear_store::RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
        };
        tx.insert_round(&gear_store::NewRound {
            key: key.clone(),
            seat: "p:heat".into(),
            generation: 1,
            cli: "opencode".into(),
            model: None,
            start_at: T,
        })?;
        tx.add_round_session(&key, "old")?;
        let mut binding = tx.get_binding("p:heat", 1)?.unwrap();
        binding.generation = 2;
        binding.session_ref = Some("restart".into());
        tx.set_seat_generation("p:heat", 2)?;
        tx.insert_binding(&binding)?;
        tx.set_meta("engine.session_fresh.p:heat", "2")?;
        Ok(())
    })
    .unwrap();
    h.set_agent_session(PANE, "restart");
    advance(&mut s, &mut h, &c, 1);
    assert_eq!(
        s.read(|tx| tx.get_meta("engine.session_fresh.p:heat"))
            .unwrap()
            .as_deref(),
        Some("")
    );
    h.exit_agent(PANE);
    advance(&mut s, &mut h, &c, 2);
    advance(&mut s, &mut h, &c, 3);
    assert!(h.calls.iter().any(|x|matches!(x,Call::AgentStart {pane,args,..} if pane==PANE && args.windows(2).any(|w|w==["--session","old"]))));
    h.set_agent_session(PANE, "old");
    advance(&mut s, &mut h, &c, 4);
    s.write(|tx| {
        let mut a = tx.get_attempt("p", "T-1", 1)?.unwrap();
        a.state = gear_model::AttemptState::Accepted;
        tx.update_attempt(&a)?;
        let mut t = tx.get_task("p", "T-1")?.unwrap();
        t.state = TaskState::Done;
        tx.update_task(&t)?;
        t.id = "T-2".into();
        t.state = TaskState::Ready;
        t.rank = 2;
        tx.insert_task(&t)?;
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-2".into(),
            version: 1,
            goal: "new".into(),
            accept: "done".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T + 5,
            mechanical: None,
        })?;
        Ok(())
    })
    .unwrap();
    advance(&mut s, &mut h, &c, 5);
    let a = s.read(|tx| tx.get_attempt("p", "T-2", 1)).unwrap().unwrap();
    assert_eq!(a.session_action.as_deref(), Some("new"));
    assert!(
        h.calls
            .iter()
            .any(|x| matches!(x,Call::PaneType {pane,text} if pane==PANE && text=="/new")),
        "{:?}",
        h.calls
    );
}

#[test]
fn retiring_lane_recovers_its_open_round_without_receiving_new_work() {
    let (mut s, mut h, c) = world(false);
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
        let key = gear_store::RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
        };
        tx.insert_round(&gear_store::NewRound {
            key: key.clone(),
            seat: "p:heat".into(),
            generation: 1,
            cli: "opencode".into(),
            model: None,
            start_at: T,
        })?;
        tx.add_round_session(&key, "old")?;
        tx.set_seat_state(
            "p:heat",
            gear_model::SeatState::Retiring,
            Some("finish current work"),
            T + 1,
        )?;
        let mut task = tx.get_task("p", "T-1")?.unwrap();
        task.id = "T-2".into();
        task.state = TaskState::Ready;
        task.rank = 2;
        tx.insert_task(&task)?;
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-2".into(),
            version: 1,
            goal: "new".into(),
            accept: "done".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T,
            mechanical: None,
        })?;
        Ok(())
    })
    .unwrap();
    h.add_shell(PANE);
    advance(&mut s, &mut h, &c, 1);
    advance(&mut s, &mut h, &c, 2);
    assert!(h.calls.iter().any(|x|matches!(x,Call::AgentStart {pane,args,..} if pane==PANE && args.windows(2).any(|w|w==["--session","old"]))),"{:?}",h.calls);
    h.set_agent_session(PANE, "old");
    advance(&mut s, &mut h, &c, 3);
    s.write(|tx| {
        let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
        attempt.state = gear_model::AttemptState::Accepted;
        tx.update_attempt(&attempt)?;
        let mut task = tx.get_task("p", "T-1")?.unwrap();
        task.state = TaskState::Done;
        tx.update_task(&task)
    })
    .unwrap();
    advance(&mut s, &mut h, &c, 4);
    assert!(s
        .read(|tx| tx.get_attempt("p", "T-2", 1))
        .unwrap()
        .is_none());
    assert!(h
        .prompts_to(PANE)
        .iter()
        .all(|p| !p.contains("New task T-2")));
}

#[test]
fn doorbell_waits_when_recovery_cannot_initialize_its_session_step() {
    use gear_model::{MsgKind, MsgState};
    use gear_store::NewMessage;
    let (mut s, mut h, mut c) = world(false);
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
        let key = gear_store::RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
        };
        tx.insert_round(&gear_store::NewRound {
            key: key.clone(),
            seat: "p:heat".into(),
            generation: 1,
            cli: "opencode".into(),
            model: None,
            start_at: T,
        })?;
        tx.add_round_session(&key, "old")?;
        tx.insert_message(&NewMessage {
            project: "p".into(),
            thread: "instruction".into(),
            task: None,
            attempt: None,
            sender: "p:captain".into(),
            sender_gen: 1,
            recipient: "p:heat".into(),
            kind: MsgKind::Instruction,
            urgent: true,
            body: "please read".into(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: T,
            meta: None,
        })?;
        Ok(())
    })
    .unwrap();
    c.profiles.remove("opencode");
    h.set_agent_session(PANE, "restart");
    advance(&mut s, &mut h, &c, 1);
    assert!(s
        .read(|tx| tx.get_meta("engine.session_step.p:heat"))
        .unwrap()
        .is_none());
    assert!(h.prompts_to(PANE).is_empty(), "{:?}", h.calls);
}
