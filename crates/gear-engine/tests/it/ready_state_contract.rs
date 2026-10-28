//! T-152: Herdr's state decides busy and ready (design §10.2).
//!
//! Whether an agent is busy is Herdr's state, never the text on its screen:
//! only `idle` and `done` are ready, and `working`, `blocked` and `unknown`
//! are never ready for a delivery, a typed command or a session action. A pi
//! agent whose last line says "Working tree clean" is not "working" because
//! of the word — and an agent Herdr cannot read is never handed a task.

use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{MsgKind, MsgState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, NewMessage, OpRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

/// A project with one lane (`heat`, pi) and one ready task; the lane's agent
/// is in the status the caller asks for.
fn world(status: AgentStatus) -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "pi", T0)?;
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
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "pi", status);
    let cfg = EngineCfg {
        tick_s: 4,
        cooldown_s: 30,
        doorbell_check_budget: 3,
        profiles: gear_herdr::builtins(),
        ..Default::default()
    };
    World { store, herdr, cfg }
}

/// Every delivery op of the lane, in any state.
fn deliveries(w: &mut World) -> Vec<OpRow> {
    let mut ops = w
        .store
        .read(|tx| {
            let mut out = Vec::new();
            for op in tx.ops_of_seat_kind("p:heat", OpKind::Deliver)? {
                out.push(op);
            }
            Ok(out)
        })
        .unwrap();
    ops.sort_by_key(|op| op.created);
    ops
}

/// A message from the captain to the lane, open (an action item).
fn captain_message(w: &mut World, kind: MsgKind, urgent: bool, at: i64) -> String {
    w.store
        .write(|tx| {
            Ok(tx
                .insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: format!("captain:{at}"),
                    task: None,
                    attempt: None,
                    sender: "p:captain".into(),
                    sender_gen: 1,
                    recipient: "p:heat".into(),
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

/// T-152: a **done** pi whose last line says "Working tree clean" gets its
/// task: the screen text never makes an agent busy.
#[test]
fn a_done_pi_gets_its_task_whatever_its_screen_says() {
    let mut w = world(AgentStatus::Done);
    w.herdr.set_screen(LANE_PANE, "Working tree clean");
    // This freshly onboarded session needs no /new, but still goes through preparation.
    w.store.write(|tx| tx.set_meta("engine.session_fresh.p:heat", "1")).unwrap();

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    let ops = deliveries(&mut w);
    assert_eq!(ops.len(), 1, "the task was handed over: {ops:?}");
    assert_eq!(ops[0].state, OpState::Submitted, "{ops:?}");
    assert!(
        w.herdr.prompts_to(LANE_PANE)[0].contains("New task T-1"),
        "{:?}",
        w.herdr.prompts_to(LANE_PANE)
    );
}

/// T-152: an agent Herdr cannot read (`unknown`) is never ready for a
/// delivery.
#[test]
fn an_unknown_agent_never_gets_a_task() {
    let mut w = world(AgentStatus::Unknown);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();

    let ops = deliveries(&mut w);
    assert!(ops.is_empty(), "no delivery for an unknown agent: {ops:?}");
    assert!(
        w.herdr.prompts_to(LANE_PANE).is_empty(),
        "nothing was typed: {:?}",
        w.herdr.prompts_to(LANE_PANE)
    );
}

/// T-152: a working agent gets only the at-once messages, as before: a normal
/// message waits for idle, an urgent one is typed at once.
#[test]
fn a_working_agent_gets_only_at_once_messages() {
    let mut w = world(AgentStatus::Working);
    let normal = captain_message(&mut w, MsgKind::Question, false, T0 + 1);
    let urgent = captain_message(&mut w, MsgKind::Instruction, true, T0 + 2);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    let said = w.herdr.prompts_to(LANE_PANE);
    assert_eq!(said.len(), 1, "only the at-once message: {said:?}");
    assert!(said[0].contains(&urgent), "the urgent one is typed: {said:?}");
    assert!(!said[0].contains(&normal), "the normal one waits: {said:?}");
}

#[test]
fn tick_doorbells_wait_for_blocked_and_allow_only_urgent_for_unknown() {
    for status in [AgentStatus::Blocked, AgentStatus::Unknown] {
        let mut w = world(status);
        w.herdr.add_agent_named(LANE_PANE, "pi", status, "p-heat");
        let normal = captain_message(&mut w, MsgKind::Instruction, false, T0 + 1);
        let urgent = captain_message(&mut w, MsgKind::Instruction, true, T0 + 2);
        tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
        let prompts = w.herdr.prompts_to(LANE_PANE);
        if status == AgentStatus::Blocked { assert!(prompts.is_empty(), "{prompts:?}"); }
        else { assert_eq!(prompts.len(),1); assert!(prompts[0].contains(&urgent)); assert!(!prompts[0].contains(&normal)); }
    }
}

#[test]
fn sender_doorbells_check_blocked_and_unknown_live() {
    for (status, urgent, should_prompt) in [
        (AgentStatus::Blocked,true,false), (AgentStatus::Blocked,false,false),
        (AgentStatus::Unknown,true,true), (AgentStatus::Unknown,false,false),
    ] {
        let mut w=world(status);
        w.herdr.add_agent_named(LANE_PANE,"pi",status,"p-heat");
        let message=captain_message(&mut w,MsgKind::Instruction,urgent,T0+1);
        let result=gear_engine::ops::deliver_doorbell_now(&mut w.store,&mut w.herdr,"p","p:heat",1,&message,urgent,T0+2).unwrap();
        assert_eq!(result.is_some(),should_prompt,"{status:?} urgent={urgent}");
        assert_eq!(w.herdr.prompts_to(LANE_PANE).len(),usize::from(should_prompt));
    }
}

#[test]
fn intended_delivery_waits_for_live_ready_state_and_is_not_duplicated() {
    let mut w=world(AgentStatus::Idle);
    w.herdr.add_agent_named(LANE_PANE,"pi",AgentStatus::Idle,"p-heat");
    let seat=w.store.read(|tx|tx.get_seat("p:heat")).unwrap().unwrap();
    let op=w.store.write(|tx| gear_engine::dispatch::intend_dispatch(tx,"p",&[(seat,gear_engine::dispatch::Action::Assign{task:"T-1".into()})],T0+1)).unwrap().remove(0);
    for (at,status) in [(10,AgentStatus::Working),(20,AgentStatus::Unknown),(30,AgentStatus::Blocked)] {
        w.herdr.set_status(LANE_PANE,status);
        tick(&mut w.store,&mut w.herdr,&w.cfg,T0+at).unwrap();
        assert!(w.herdr.prompts_to(LANE_PANE).is_empty(),"{status:?}");
        let ops=deliveries(&mut w); assert_eq!(ops.len(),1); assert_eq!(ops[0].id,op.id); assert_eq!(ops[0].state,OpState::Intended);
    }
    w.herdr.set_status(LANE_PANE,AgentStatus::Done);
    tick(&mut w.store,&mut w.herdr,&w.cfg,T0+40).unwrap();
    assert_eq!(w.herdr.prompts_to(LANE_PANE).len(),1);
    assert!(w.herdr.prompts_to(LANE_PANE)[0].contains("New task T-1"));
    let ops=deliveries(&mut w); assert_eq!(ops.len(),1); assert_eq!(ops[0].id,op.id); assert_eq!(ops[0].state,OpState::Submitted);
}

#[test]
fn intended_doorbell_rechecks_eligibility_when_status_changes() {
    for (status,urgent,allowed) in [(AgentStatus::Blocked,true,false),(AgentStatus::Unknown,true,true),(AgentStatus::Unknown,false,false),(AgentStatus::Working,false,true)] {
        let mut w=world(AgentStatus::Idle);
        w.herdr.add_agent_named(LANE_PANE,"pi",AgentStatus::Idle,"p-heat");
        let message=captain_message(&mut w,MsgKind::Instruction,urgent,T0+1);
        let action=gear_engine::doorbell::DoorbellAction { project:"p".into(),seat:"p:heat".into(),generation:1,pane_id:LANE_PANE.into(),text:format!("GEAR: {message}"),covers:vec![message] };
        let op=w.store.write(|tx|gear_engine::ops::intend_doorbells(tx,&[action],T0+2)).unwrap().remove(0);
        w.herdr.set_status(LANE_PANE,status);
        gear_engine::ops::execute_intended(&mut w.store,&mut w.herdr,"p",T0+3).unwrap();
        assert_eq!(w.herdr.prompts_to(LANE_PANE).len(),usize::from(allowed),"{status:?} urgent={urgent}");
        let ops=w.store.read(|tx|tx.ops_of_seat_kind("p:heat",OpKind::Doorbell)).unwrap();
        assert_eq!(ops[0].id,op.id); assert_eq!(ops[0].state,if allowed {OpState::Submitted} else {OpState::Failed});
    }
}


#[test]
fn refused_nonurgent_sender_doorbell_cannot_starve_a_later_urgent_one() {
    let mut w=world(AgentStatus::Unknown);
    w.herdr.add_agent_named(LANE_PANE,"pi",AgentStatus::Unknown,"p-heat");
    w.store.write(|tx| { let mut task=tx.get_task("p","T-1")?.unwrap(); task.state=TaskState::Draft; tx.update_task(&task) }).unwrap();
    let normal=captain_message(&mut w,MsgKind::Instruction,false,T0+1);
    assert!(gear_engine::ops::deliver_doorbell_now(&mut w.store,&mut w.herdr,"p","p:heat",1,&normal,false,T0+2).unwrap().is_none());
    let old=w.store.read(|tx|tx.ops_of_seat_kind("p:heat",OpKind::Doorbell)).unwrap();
    assert_eq!(old.len(),1); assert_eq!(old[0].state,OpState::Failed);
    assert!(w.store.read(|tx|tx.slot_ops("p:heat")).unwrap().is_empty());
    let urgent=captain_message(&mut w,MsgKind::Instruction,true,T0+3);
    tick(&mut w.store,&mut w.herdr,&w.cfg,T0+10).unwrap();
    tick(&mut w.store,&mut w.herdr,&w.cfg,T0+20).unwrap();
    let prompts=w.herdr.prompts_to(LANE_PANE);
    assert_eq!(prompts.len(),1,"{prompts:?}"); assert!(prompts[0].contains(&urgent)); assert!(!prompts[0].contains(&normal));
    w.store.write(|tx|tx.insert_view(&gear_store::ViewRecord {project:"p".into(),seat:"p:heat".into(),generation:1,ts:T0+21,shown_msgs:vec![urgent.clone()],shown_ops:vec![],shown_task:None,shown_attempt:None,shown_contract:None})).unwrap();
    w.herdr.set_status(LANE_PANE,AgentStatus::Done);
    tick(&mut w.store,&mut w.herdr,&w.cfg,T0+30).unwrap();
    let prompts=w.herdr.prompts_to(LANE_PANE);
    assert_eq!(prompts.len(),2,"{prompts:?}"); assert!(prompts[1].contains(&normal));
}


#[test]
fn naming_and_observation_failures_leave_the_doorbell_for_retry() {
    for unreadable in [false,true] {
        let mut w=world(AgentStatus::Idle);
        let message=captain_message(&mut w,MsgKind::Instruction,true,T0+1);
        if unreadable { w.herdr.observe_fails=true; }
        else { w.herdr.inject_next=Some(gear_herdr::fake::Inject::Refuse {code:"rename-failed".into()}); }
        assert!(gear_engine::ops::deliver_doorbell_now(&mut w.store,&mut w.herdr,"p","p:heat",1,&message,true,T0+2).unwrap().is_none());
        let ops=w.store.read(|tx|tx.ops_of_seat_kind("p:heat",OpKind::Doorbell)).unwrap();
        assert_eq!(ops.len(),1); assert_eq!(ops[0].state,OpState::Intended,"unreadable={unreadable}");
        assert!(w.herdr.prompts_to(LANE_PANE).is_empty());
    }
}
