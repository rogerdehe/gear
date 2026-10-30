//! T-161: declared waits suppress stall work and wake after the merge.
use gear_app::{mail::{self, SendArgs}, identity::Fingerprint, Env};
use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{AttemptState, GearCtx, MsgKind, MsgState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, ContractRow, NewMessage, NewOp, NewRound, RoundKey, Store, TaskRow};
const T:i64=1_800_000_000;
const LANE:&str="w1:p2";
const SHA:&str="abc1234567890abc1234567890abc1234567890a";

fn world() -> (Store,FakeHerdr,EngineCfg) {
    let mut s=Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx,"p","sess",T)?;
        testing::seed_active_seat(tx,"p","captain",SeatRole::Captain,"w1:p1","codex",T)?;
        testing::seed_active_seat(tx,"p","heat",SeatRole::Lane,LANE,"opencode",T)?;
        for (id,rank) in [("T-1",1),("T-2",2)] {
            tx.insert_task(&TaskRow {repo:None,base:None,gate:None,rework_estimate_min:None,project:"p".into(),id:id.into(),rank,state:TaskState::Ready,priority:"must".into(),lane:Some("heat".into()),milestone:None,required:false,contract_version:1,deps:vec![],followup_of:None,wait_on:None,wait_cond:None,rework_note:None,estimate_min:Some(10),issue:None,updated:T})?;
            tx.insert_contract(&ContractRow {project:"p".into(),task:id.into(),version:1,goal:"goal".into(),accept:"done".into(),evidence_files:vec![],evidence_due_min:None,body:String::new(),created:T, mechanical: None,})?;
        }
        gear_app::tasks::start_attempt(tx,"p","T-1","p:heat",1,T)?;
        gear_app::tasks::on_delivery_confirmed(tx,"p","T-1",1,T)?;
        tx.insert_round(&NewRound {key:RoundKey{project:"p".into(),task:"T-1".into(),attempt:1,role:"dev".into()},seat:"p:heat".into(),generation:1,cli:"opencode".into(),model:None,start_at:T})?;
        Ok(())
    }).unwrap();
    let mut h=FakeHerdr::new(); h.add_agent_named("w1:p1","codex",AgentStatus::Idle,"p-captain"); h.add_agent_named(LANE,"opencode",AgentStatus::Idle,"p-heat"); h.set_agent_session(LANE,"existing-session");
    let mut cfg=EngineCfg::default(); cfg.profiles=gear_herdr::builtins(); cfg.cooldown_s=0;
    (s,h,cfg)
}
fn block(s:&mut Store) { block_at(s,10); }
fn block_at(s:&mut Store, at:i64) {
    let env=Env {ctx:Some(GearCtx{project:"p".into(),seat:"heat".into(),generation:1}),pane_id:Some(LANE.into()),admin:false};
    let fp=Fingerprint{pane_id:LANE.into(),kind:Some("opencode".into()),session_ref:None};
    let args=SendArgs {kind:MsgKind::Blocked,body:"waiting for API".into(),task:Some("T-1".into()),attempt:Some(1),contract:Some(1),candidate:None,verdict:None,blocking:None,base:None,about:None,to:None,reply_to:None,urgent:false,left:None,blocker:None,on:Some("T-2".into()),friction:None,request_id:"wait-one".into()};
    mail::send(s,&env,Some(&fp),&args,T+at).unwrap();
}
fn advance(s:&mut Store,h:&mut FakeHerdr,c:&EngineCfg,at:i64) {tick(s,h,c,T+at).unwrap();}
#[test]
fn wait_pauses_clock_suppresses_nudges_and_wakes_once_when_merged() {
    let (mut s,mut h,c)=world(); block(&mut s);
    s.write(|tx| {
        // Overlapping quota and declared waits count only once in the round.
        tx.open_pause("p","p:heat","quota","quota",T+20)?;
        tx.close_pause("p","p:heat","quota",T+50)?;
        tx.insert_message(&NewMessage {project:"p".into(),thread:"overrun-question:T-1:1:dev".into(),task:Some("T-1".into()),attempt:Some(1),sender:"engine".into(),sender_gen:0,recipient:"p:heat".into(),kind:MsgKind::Instruction,urgent:true,body:"old overrun question".into(),reply_to:None,needs_ack:false,state:MsgState::Open,created:T+11,meta:None})?;
        tx.insert_op(&NewOp {project:"p".into(),seat:"p:heat".into(),generation:1,kind:OpKind::Deliver,task:Some("T-1".into()),attempt:Some(1),ref_msg:None,plan_version:None,payload:serde_json::json!({"text":"old nudge"}),created:T+12})?;
        Ok(())
    }).unwrap();
    advance(&mut s,&mut h,&c,100);
    let r=s.read(|tx|tx.rounds_of_task("p","T-1")).unwrap().remove(0); assert_eq!(r.paused_s,90);
    let ops=s.read(|tx|tx.ops_of_seat_kind("p:heat",OpKind::Deliver)).unwrap(); assert_eq!(ops[0].state,OpState::Void);
    assert!(h.prompts_to(LANE).is_empty(),"{:?}",h.calls);
    let (mut missing_s,mut missing_h,missing_c)=world(); block(&mut missing_s);
    advance(&mut missing_s,&mut missing_h,&missing_c,100);
    missing_h.exit_agent(LANE);
    let report=tick(&mut missing_s,&mut missing_h,&missing_c,T+120).unwrap(); assert!(!report.flagged.contains(&"p:heat".into()));
    assert!(missing_s.read(|tx|tx.thread_messages("p","stall:p:heat")).unwrap().iter().all(|m|m.state!=MsgState::Open));
    s.write(|tx| {
        let mut task=tx.get_task("p","T-2")?.unwrap(); task.state=TaskState::Done; tx.update_task(&task)?;
        tx.insert_attempt(&AttemptRow {session_ref:None,session_cli:None,session_action:None,sent_at:None,project:"p".into(),task:"T-2".into(),n:1,seat:"p:other".into(),generation:1,contract_version:1,adopted_version:1,pending_update:None,state:AttemptState::Accepted,candidate:Some(SHA.into()),report:None,safe_point:None,started:T,updated:T+130})?;
        Ok(())
    }).unwrap();
    advance(&mut s,&mut h,&c,150);
    assert_eq!(s.read(|tx|tx.get_task("p","T-1")).unwrap().unwrap().wait_on.as_deref(),Some("T-2"),"Done alone is not a merge");
    s.write(|tx|tx.set_attempt_time("p","T-2",1,AttemptStep::Merged,T+200)).unwrap();
    advance(&mut s,&mut h,&c,199);
    assert_eq!(s.read(|tx|tx.get_task("p","T-1")).unwrap().unwrap().wait_on.as_deref(),Some("T-2"),"a future merge has not landed");
    advance(&mut s,&mut h,&c,230); advance(&mut s,&mut h,&c,240);
    let task=s.read(|tx|tx.get_task("p","T-1")).unwrap().unwrap(); assert!(task.wait_on.is_none());
    let pauses=s.read(|tx|tx.pauses_of_project("p")).unwrap(); let pause=pauses.iter().find(|p|p.kind=="wait" && p.seat=="p:heat").unwrap(); assert_eq!((pause.start_at,pause.end_at),(T+10,Some(T+200)));
    let r=s.read(|tx|tx.rounds_of_task("p","T-1")).unwrap().remove(0); assert_eq!(r.paused_s,190);
    let wake=s.read(|tx|tx.thread_messages("p","wait:T-1:1:T-2")).unwrap(); assert_eq!(wake.len(),1); assert!(wake[0].urgent); assert_eq!(wake[0].body,format!("GEAR: T-2 is merged ({SHA}). Continue T-1."));
    assert!(h.prompts_to(LANE).iter().any(|p|p.contains(&wake[0].id)),"{:?}",h.calls);
    assert_eq!(s.read(|tx|tx.get_binding("p:heat",1)).unwrap().unwrap().session_ref.as_deref(),Some("existing-session"));
}

#[test]
fn declared_wait_stops_the_live_overrun_clock() {
    let (mut s,mut h,c)=world(); block(&mut s);
    advance(&mut s,&mut h,&c,1000);
    let r=s.read(|tx|tx.rounds_of_task("p","T-1")).unwrap().remove(0);
    assert_eq!((r.active_s,r.paused_s),(10,990));
    advance(&mut s,&mut h,&c,2000);
    let r=s.read(|tx|tx.rounds_of_task("p","T-1")).unwrap().remove(0);
    assert_eq!((r.active_s,r.paused_s),(10,1990));
    assert!(s.read(|tx|tx.thread_messages("p","overrun-question:T-1:1:dev")).unwrap().is_empty());
    assert!(h.prompts_to(LANE).is_empty());
}

#[test]
fn pending_overrun_does_not_escalate_while_task_is_waiting() {
    let (mut s,mut h,c)=world();
    advance(&mut s,&mut h,&c,600);
    assert_eq!(s.read(|tx|tx.thread_messages("p","overrun-question:T-1:1:dev")).unwrap().len(),1);
    block_at(&mut s,1200);
    advance(&mut s,&mut h,&c,1200);
    assert!(s.read(|tx|tx.thread_messages("p","overrun:T-1:dev")).unwrap().is_empty());
    assert!(h.prompts_to(LANE).is_empty(),"{:?}",h.calls);
}

#[test]
fn merge_collected_after_entry_wait_pass_uses_its_actual_completion_time() {
    let (mut s,mut h,c)=world(); block(&mut s);
    s.write(|tx| tx.insert_attempt(&AttemptRow {session_ref:None,session_cli:None,session_action:None,sent_at:None,project:"p".into(),task:"T-2".into(),n:1,seat:"p:other".into(),generation:1,contract_version:1,adopted_version:1,pending_update:None,state:AttemptState::Accepted,candidate:Some(SHA.into()),report:None,safe_point:None,started:T,updated:T+130})).unwrap();
    gear_engine::waits::tick(&mut s,&mut h,&c,T+230).unwrap();
    s.write(|tx|tx.set_attempt_time("p","T-2",1,AttemptStep::Merged,T+200)).unwrap();
    gear_engine::waits::tick(&mut s,&mut h,&c,T+230).unwrap();
    gear_engine::overrun::tick(&mut s,&mut h,&c,T+230).unwrap();
    let r=s.read(|tx|tx.rounds_of_task("p","T-1")).unwrap().remove(0);
    assert_eq!((r.paused_s,r.active_s),(190,40));
}

#[test]
fn full_tick_uses_quota_resume_transition_instead_of_observation_time() {
    let (mut s,mut h,c)=world();
    s.write(|tx|tx.set_seat_state("p:heat",gear_model::SeatState::Parked,Some("quota"),T+700)).unwrap();
    advance(&mut s,&mut h,&c,2000);
    s.write(|tx|tx.set_seat_state("p:heat",gear_model::SeatState::Active,Some("quota reset"),T+2100)).unwrap();
    advance(&mut s,&mut h,&c,2200);
    let r=s.read(|tx|tx.rounds_of_task("p","T-1")).unwrap().remove(0);
    assert_eq!((r.paused_s,r.active_s),(1400,800));
}

// T-440: a wait on something outside GEAR lasts at most 120 minutes, and the
// lane's own next word on the task ends it early.

/// Declare an external wait for T-1 as the lane, with the reason in the body.
fn wait_outside(s: &mut Store, at: i64) {
    let env = Env { ctx: Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 1 }), pane_id: Some(LANE.into()), admin: false };
    let fp = Fingerprint { pane_id: LANE.into(), kind: Some("opencode".into()), session_ref: None };
    let args = SendArgs { kind: MsgKind::Blocked, body: "the CI run on heat/t1".into(), task: Some("T-1".into()), attempt: Some(1),
        contract: Some(1), candidate: None, verdict: None, blocking: None, base: None, about: None, to: None, reply_to: None,
        urgent: false, left: None, blocker: None, on: Some("external".into()), friction: None, request_id: "wait-outside".into() };
    mail::send(s, &env, Some(&fp), &args, T + at).unwrap();
}

fn external_wait(s: &mut Store) -> (Option<String>, Option<String>) {
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    (task.wait_on, task.wait_cond)
}

/// The lane's later message on the task ends the wait without an instruction:
/// it is back to work, and the pause is closed.
#[test]
fn a_later_message_ends_an_external_wait() {
    let (mut s, mut h, c) = world();
    wait_outside(&mut s, 10);
    assert_eq!(external_wait(&mut s), (Some("external".into()), Some("external".into())));
    advance(&mut s, &mut h, &c, 100);
    assert_eq!(external_wait(&mut s), (Some("external".into()), Some("external".into())), "still waiting");

    // A later blocked message does not end it; a progress one does.
    let late = |kind, at: i64, s: &mut Store| {
        s.write(|tx| tx.insert_message(&NewMessage { project: "p".into(), thread: "T-1:q".into(), task: Some("T-1".into()), attempt: Some(1),
            sender: "p:heat".into(), sender_gen: 1, recipient: "p:captain".into(), kind, urgent: false, body: "still waiting".into(),
            reply_to: None, needs_ack: false, state: MsgState::Info, created: T + at, meta: None }).map(|_| ())).unwrap()
    };
    late(MsgKind::Blocked, 150, &mut s);
    advance(&mut s, &mut h, &c, 160);
    assert_eq!(external_wait(&mut s), (Some("external".into()), Some("external".into())), "another blocked message is not a word to go on");

    late(MsgKind::Progress, 200, &mut s);
    advance(&mut s, &mut h, &c, 210);
    assert_eq!(external_wait(&mut s), (None, None));
    let pause = s.read(|tx| tx.pauses_of_project("p")).unwrap().into_iter().find(|p| p.kind == "wait" && p.seat == "p:heat").unwrap();
    assert_eq!((pause.start_at, pause.end_at), (T + 10, Some(T + 210)), "the wait pause is closed");
    assert_eq!(s.read(|tx| tx.get_meta(&gear_app::mail::external_wait_key("p", "T-1"))).unwrap().as_deref(), Some(""), "the clock is emptied");
    assert!(s.read(|tx| tx.thread_messages("p", "wait-external-end:T-1:1")).unwrap().is_empty(), "the lane ended it, so no time-out instruction");
}

/// 120 minutes with no word from the lane: the wait ends and the lane is told
/// to push what it has, or to declare the wait again.
#[test]
fn an_external_wait_times_out_with_an_instruction() {
    let (mut s, mut h, c) = world();
    wait_outside(&mut s, 0);
    advance(&mut s, &mut h, &c, 120 * 60 - 1);
    assert_eq!(external_wait(&mut s), (Some("external".into()), Some("external".into())), "not before 120 min");
    assert!(h.prompts_to(LANE).is_empty(), "{:?}", h.calls);
    advance(&mut s, &mut h, &c, 120 * 60);
    assert_eq!(external_wait(&mut s), (None, None));
    let told = s.read(|tx| tx.thread_messages("p", "wait-external-end:T-1:1")).unwrap();
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].recipient, "p:heat");
    assert_eq!(told[0].kind, MsgKind::Instruction);
    assert!(told[0].urgent);
    assert_eq!(told[0].body, "GEAR: your external wait on T-1 ended after 120 min. Push what you have, or declare the wait again with its reason.");
    assert_eq!(s.read(|tx| tx.get_meta(&gear_app::mail::external_wait_key("p", "T-1"))).unwrap().as_deref(), Some(""));
    assert!(h.prompts_to(LANE).iter().any(|p| p.contains(&told[0].id)), "{:?}", h.calls);
    advance(&mut s, &mut h, &c, 120 * 60 + 10);
    assert_eq!(s.read(|tx| tx.thread_messages("p", "wait-external-end:T-1:1")).unwrap().len(), 1, "told once");
}

/// A wait on another task still wakes on the merge, not on the clock.
#[test]
fn a_merged_wait_keeps_its_own_ending() {
    let (mut s, mut h, c) = world();
    block(&mut s);
    advance(&mut s, &mut h, &c, 200 * 60);
    assert_eq!(s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().wait_on.as_deref(), Some("T-2"), "no 120-minute clock on a merged wait");
    assert!(s.read(|tx| tx.thread_messages("p", "wait-external-end:T-1:1")).unwrap().is_empty());
}
