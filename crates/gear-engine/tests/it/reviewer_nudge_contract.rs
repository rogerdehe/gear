//! Contract (L-53): a reviewer that is idle before its verdict is told to go
//! on, a few times, then flagged for the captain. The signal is Herdr's state
//! (idle while the job is not done), not screen text.

use std::collections::BTreeSet;

use gear_app::tasks::start_attempt;
use gear_engine::observe::{Presence, SeatObs};
use gear_engine::review::nudge_stopped_reviewers;
use gear_model::{AttemptState, OpKind, OpState, TaskState};
use gear_store::{testing, ContractRow, NewOp, ReviewVerdictRow, SeatRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const COOLDOWN: i64 = 90;

fn world() -> (Store, SeatRow) {
    let mut s = Store::open_in_memory().unwrap();
    let seat = s
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", gear_model::SeatRole::Lane, "w1:p2", "opencode", T0)?;
            tx.insert_task(&TaskRow { repo: None, base: None, gate: None, rework_estimate_min: None, project: "p".into(), id: "T-7".into(), rank: 1, state: TaskState::Ready,
                priority: "must".into(), lane: Some("heat".into()), milestone: None, required: false, contract_version: 1,
                deps: vec![], followup_of: None, wait_on: None, wait_cond: None, rework_note: None, estimate_min: None,  issue: None,updated: T0 })?;
            tx.insert_contract(&ContractRow { project: "p".into(), task: "T-7".into(), version: 1, goal: "g".into(),
                accept: "a".into(), evidence_files: vec![], evidence_due_min: None, body: String::new(), created: T0, mechanical: None, })?;
            start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?;
            let mut a = tx.attempts_of_task("p", "T-7")?.pop().unwrap();
            a.state = AttemptState::Submitted;
            a.candidate = Some("abc1234".into());
            tx.update_attempt(&a)?;
            let mut task = tx.get_task("p", "T-7")?.unwrap();
            task.state = TaskState::Verifying;
            tx.update_task(&task)?;
            let seat = testing::seed_temp_seat(tx, "p", "r-1", "T-7", "w9:p1", "codex", T0)?;
            // The review packet, delivered and confirmed.
            let op = tx.insert_op(&NewOp { project: "p".into(), seat: seat.id.clone(), generation: 1, kind: OpKind::Deliver,
                task: Some("T-7".into()), attempt: Some(1), ref_msg: None, plan_version: None,
                payload: serde_json::json!({"text": "Review T-7"}), created: T0 })?;
            tx.set_op_state("p", &op.id, OpState::Confirmed, Some("x"), T0 + 1)?;
            Ok(seat)
        })
        .unwrap();
    (s, seat)
}

fn idle(seat: &SeatRow) -> Vec<SeatObs> {
    vec![SeatObs { seat: seat.clone(), presence: Presence::Present, status: None, flags: BTreeSet::new(), idle: true, screen: None, session: None }]
}

/// Settle the last nudge so the next one may go out.
fn settle_last(s: &mut Store, seat: &str, at: i64) {
    s.write(|tx| {
        let op = tx.last_op(seat, OpKind::Deliver)?.unwrap();
        tx.set_op_state("p", &op.id, OpState::Confirmed, Some("x"), at)
    })
    .unwrap();
}

#[test]
fn an_idle_reviewer_without_a_verdict_is_nudged_then_flagged() {
    let (mut s, seat) = world();
    // Inside the cooldown: nothing.
    assert_eq!(nudge_stopped_reviewers(&mut s, "p", &idle(&seat), COOLDOWN, T0 + 10).unwrap(), (vec![], vec![]));
    let mut t = T0;
    for _ in 0..3 {
        t += COOLDOWN + 1;
        let (nudged, flagged) = nudge_stopped_reviewers(&mut s, "p", &idle(&seat), COOLDOWN, t).unwrap();
        assert_eq!((nudged, flagged), (vec![seat.id.clone()], vec![]));
        settle_last(&mut s, &seat.id, t + 1);
    }
    t += COOLDOWN + 1;
    assert_eq!(nudge_stopped_reviewers(&mut s, "p", &idle(&seat), COOLDOWN, t).unwrap(), (vec![], vec![seat.id.clone()]));
}

#[test]
fn a_reviewer_that_handed_in_or_is_working_is_left_alone() {
    let (mut s, seat) = world();
    let mut working = idle(&seat);
    working[0].idle = false;
    assert_eq!(nudge_stopped_reviewers(&mut s, "p", &working, COOLDOWN, T0 + 500).unwrap(), (vec![], vec![]));
    s.write(|tx| tx.insert_review_verdict(&ReviewVerdictRow { project: "p".into(), task: "T-7".into(), attempt: 1,
        passed: true, findings: "ok".into(), by_seat: seat.id.clone(), created: T0 + 5 })).unwrap();
    assert_eq!(nudge_stopped_reviewers(&mut s, "p", &idle(&seat), COOLDOWN, T0 + 500).unwrap(), (vec![], vec![]));
}

/// L-76: the packet's delivery stays submitted (it is confirmed only by the
/// verdict) — when the packet or its Enter was lost, the reviewer sits idle
/// and the nudge must still go out, carrying the packet again.
#[test]
fn a_lost_packet_is_sent_again_by_the_nudge() {
    let (mut s, seat) = world();
    s.write(|tx| {
        // A packet that was submitted and never taken (no verdict confirms it).
        let op = tx.insert_op(&NewOp { project: "p".into(), seat: seat.id.clone(), generation: 1, kind: OpKind::Deliver,
            task: Some("T-7".into()), attempt: Some(1), ref_msg: None, plan_version: None,
            payload: serde_json::json!({"text": "Review T-7 (resent packet)"}), created: T0 })?;
        tx.set_op_state("p", &op.id, OpState::Executing, None, T0)?;
        tx.set_op_state("p", &op.id, OpState::Submitted, None, T0)
    })
    .unwrap();
    let t = T0 + COOLDOWN + 1;
    let (nudged, _) = nudge_stopped_reviewers(&mut s, "p", &idle(&seat), COOLDOWN, t).unwrap();
    assert_eq!(nudged, vec![seat.id.clone()]);
    let nudge = s.read(|tx| tx.last_op(&seat.id, OpKind::Deliver)).unwrap().unwrap();
    let text = nudge.payload["text"].as_str().unwrap();
    assert!(text.contains("Review T-7"), "the packet rides along: {text}");
}

#[test]
fn overrun_answer_resets_reviewer_stall_and_starts_a_new_nudge_budget() {
    use gear_herdr::{fake::FakeHerdr, AgentStatus};
    use gear_model::{MsgKind, MsgState, SeatRole};
    use gear_store::{NewMessage, NewRound, RoundKey};
    let (mut s, seat) = world();
    s.write(|tx| {
        testing::seed_active_seat(tx,"p","captain",SeatRole::Captain,"w1:p1","codex",T0)?;
        tx.insert_round(&NewRound { key:RoundKey {project:"p".into(),task:"T-7".into(),attempt:1,role:"review".into()},seat:seat.id.clone(),generation:1,cli:"codex".into(),model:None,start_at:T0 })?;
        let op=tx.insert_op(&NewOp {project:"p".into(),seat:seat.id.clone(),generation:1,kind:OpKind::Deliver,task:Some("T-7".into()),attempt:Some(1),ref_msg:None,plan_version:None,payload:serde_json::json!({"text":"continue review", "nudge":3}),created:T0+1700})?;
        tx.set_op_state("p",&op.id,OpState::Confirmed,None,T0+1701)?;
        Ok(())
    }).unwrap();
    let mut h=FakeHerdr::new();
    h.add_agent_named("w1:p1","codex",AgentStatus::Idle,"p-captain");
    h.add_agent_named("w1:p2","opencode",AgentStatus::Working,"p-heat");
    h.add_agent_named("w9:p1","codex",AgentStatus::Idle,"p-review-t7");
    let mut cfg=gear_engine::EngineCfg::default(); cfg.cooldown_s=COOLDOWN; cfg.profiles=gear_herdr::builtins();
    gear_engine::overrun::tick(&mut s,&mut h,&cfg,T0+1800).unwrap();
    s.write(|tx| {
        tx.insert_message(&NewMessage {project:"p".into(),thread:"T-7:1".into(),task:Some("T-7".into()),attempt:Some(1),sender:seat.id.clone(),sender_gen:1,recipient:"p:captain".into(),kind:MsgKind::Progress,urgent:false,body:"review in progress".into(),reply_to:None,needs_ack:false,state:MsgState::Info,created:T0+1801,meta:Some(serde_json::json!({"left":15,"blocker":"none"}).to_string())})?;
        Ok(())
    }).unwrap();
    let report=gear_engine::tick(&mut s,&mut h,&cfg,T0+1802).unwrap();
    assert!(!report.flagged.contains(&seat.id),"{report:?}");
    let stalls=s.read(|tx|tx.thread_messages("p","stall:p:r-1")).unwrap();
    assert!(stalls.iter().all(|m|m.state!=MsgState::Open),"{stalls:?}");
    let items=s.read(|tx|tx.thread_messages("p","overrun:T-7:review")).unwrap();
    assert_eq!(items.len(),1); assert!(items[0].body.contains("review in progress"));
    let (nudged, flagged)=nudge_stopped_reviewers(&mut s,"p",&idle(&seat),COOLDOWN,T0+1892).unwrap();
    assert_eq!((nudged,flagged),(vec![seat.id.clone()],vec![]));
    let op=s.read(|tx|tx.last_op(&seat.id,OpKind::Deliver)).unwrap().unwrap();
    assert_eq!(op.payload["nudge"],1);

    // An unrelated missing-agent flag still reaches the captain, even though
    // the reviewer recently answered the overrun question.
    h.exit_agent("w9:p1");
    let report=gear_engine::tick(&mut s,&mut h,&cfg,T0+1893).unwrap();
    assert!(report.flagged.contains(&seat.id),"{report:?}");
    let stalls=s.read(|tx|tx.thread_messages("p","stall:p:r-1")).unwrap();
    assert!(stalls.iter().any(|m|m.state==MsgState::Open),"{stalls:?}");
}

#[test]
fn progress_from_an_earlier_generation_of_the_same_review_still_counts() {
    let (mut s,mut seat)=world();
    s.write(|tx| tx.set_seat_generation(&seat.id,2)).unwrap();
    seat.generation=2;
    s.write(|tx| {
        let op=tx.insert_op(&NewOp {project:"p".into(),seat:seat.id.clone(),generation:1,kind:OpKind::Deliver,task:Some("T-7".into()),attempt:Some(1),ref_msg:None,plan_version:None,payload:serde_json::json!({"nudge":3}),created:T0+100})?;
        tx.set_op_state("p",&op.id,OpState::Confirmed,None,T0+101)?;
        tx.insert_message(&gear_store::NewMessage {project:"p".into(),thread:"T-7:1".into(),task:Some("T-7".into()),attempt:Some(1),sender:seat.id.clone(),sender_gen:1,recipient:"p:captain".into(),kind:gear_model::MsgKind::Progress,urgent:false,body:"progress".into(),reply_to:None,needs_ack:false,state:gear_model::MsgState::Info,created:T0+200,meta:Some(serde_json::json!({"left":15,"blocker":"none"}).to_string())})?;
        Ok(())
    }).unwrap();
    let (nudged,flagged)=nudge_stopped_reviewers(&mut s,"p",&idle(&seat),COOLDOWN,T0+291).unwrap();
    assert_eq!((nudged,flagged),(vec![seat.id.clone()],vec![]));
    let op=s.read(|tx|tx.last_op(&seat.id,OpKind::Deliver)).unwrap().unwrap();
    assert_eq!(op.payload["nudge"],1);
}

// ---------- T-284: an uncertain packet op or an older generation never holds
// the nudge for good.

/// The at-capacity screen a stopped reviewer shows (measured on review-7 of
/// T-242): it is no quota and no login, so the screen flags do not stop the
/// nudge — this test reads the flags the way the engine does.
fn at_capacity_flags() -> BTreeSet<gear_herdr::profile::ScreenFlag> {
    gear_herdr::classify_screen(
        &gear_herdr::builtin("codex").unwrap(),
        "■ Selected model is at capacity. Please try a different model.",
    )
}

/// An idle reviewer whose packet op is `Unknown` and older than the cooldown is
/// nudged: an uncertain delivery never settles, so it could not block for good.
#[test]
fn an_unknown_packet_op_older_than_the_cooldown_does_not_block_the_nudge() {
    let (mut s, seat) = world();
    s.write(|tx| {
        let op = tx.insert_op(&NewOp { project: "p".into(), seat: seat.id.clone(), generation: 1, kind: OpKind::Deliver,
            task: Some("T-7".into()), attempt: Some(1), ref_msg: None, plan_version: None,
            payload: serde_json::json!({"text": "Review T-7 (uncertain send)"}), created: T0 })?;
        tx.set_op_state("p", &op.id, OpState::Executing, None, T0)?;
        tx.set_op_state("p", &op.id, OpState::Unknown, None, T0)
    })
    .unwrap();
    let flags = at_capacity_flags();
    let mut seen = idle(&seat);
    seen[0].flags = flags;
    let t = T0 + COOLDOWN + 1;
    assert_eq!(
        nudge_stopped_reviewers(&mut s, "p", &seen, COOLDOWN, t).unwrap(),
        (vec![seat.id.clone()], vec![])
    );
    let nudge = s.read(|tx| tx.last_op(&seat.id, OpKind::Deliver)).unwrap().unwrap();
    let text = nudge.payload["text"].as_str().unwrap();
    assert!(text.contains("Review T-7"), "the packet rides along: {text}");
}

/// An `Intended` op of a generation that is gone (the agent was replaced) does
/// not block the nudge of the current generation.
#[test]
fn an_intended_op_of_an_older_generation_does_not_block_the_nudge() {
    let (mut s, seat) = world();
    s.write(|tx| {
        tx.insert_op(&NewOp { project: "p".into(), seat: seat.id.clone(), generation: 0, kind: OpKind::Doorbell,
            task: None, attempt: None, ref_msg: None, plan_version: None,
            payload: serde_json::json!({"text": "doorbell"}), created: T0 })
            .map(|_| ())
    })
    .unwrap();
    let t = T0 + COOLDOWN + 1;
    assert_eq!(
        nudge_stopped_reviewers(&mut s, "p", &idle(&seat), COOLDOWN, t).unwrap(),
        (vec![seat.id.clone()], vec![])
    );
}

/// An `Intended` op of the current generation still blocks: two deliveries must
/// never be in flight.
#[test]
fn an_intended_op_of_the_current_generation_still_blocks_the_nudge() {
    let (mut s, seat) = world();
    s.write(|tx| {
        tx.insert_op(&NewOp { project: "p".into(), seat: seat.id.clone(), generation: 1, kind: OpKind::Doorbell,
            task: None, attempt: None, ref_msg: None, plan_version: None,
            payload: serde_json::json!({"text": "doorbell"}), created: T0 })
            .map(|_| ())
    })
    .unwrap();
    let t = T0 + COOLDOWN + 1;
    assert_eq!(
        nudge_stopped_reviewers(&mut s, "p", &idle(&seat), COOLDOWN, t).unwrap(),
        (vec![], vec![])
    );
}

// ---------- T-245: a direction check that stops is nudged like a review

/// The same world with the seat on a direction job: the attempt is Started (a
/// hand-in is not in yet) and the job meta says `running`.
fn direction_world() -> (Store, SeatRow) {
    let (mut s, seat) = world();
    let id = seat.id.clone();
    s.write(|tx| {
        let mut attempt = tx.get_attempt("p", "T-7", 1)?.unwrap();
        attempt.state = AttemptState::Started;
        attempt.candidate = None;
        tx.update_attempt(&attempt)?;
        tx.set_meta("engine.direction.p.T-7.1", &serde_json::json!({
            "seat": id, "generation": 1, "sha": "abc1234", "state": "running"
        }).to_string())
    })
    .unwrap();
    (s, seat)
}

/// The seats as a tick sees them, through the Herdr fake.
fn observed(s: &mut Store, status: gear_herdr::AgentStatus) -> Vec<SeatObs> {
    let mut h = gear_herdr::fake::FakeHerdr::new();
    h.add_agent_named("w9:p1", "codex", status, "p-review-t7");
    let agents = gear_engine::observe::snapshot(&mut h).unwrap();
    s.read(|tx| gear_engine::observe::reconcile(tx, &mut h, "p", &agents, &gear_herdr::builtins()))
        .unwrap()
}

/// The idle direction seat whose job still runs is nudged once the cooldown
/// has passed, and the nudge carries the direction packet again.
#[test]
fn an_idle_direction_seat_whose_job_runs_is_nudged() {
    let (mut s, seat) = direction_world();
    // Inside the cooldown: nothing.
    let seen = observed(&mut s, gear_herdr::AgentStatus::Idle);
    assert_eq!(
        nudge_stopped_reviewers(&mut s, "p", &seen, COOLDOWN, T0 + 10).unwrap(),
        (vec![], vec![])
    );
    let t = T0 + COOLDOWN + 1;
    let seen = observed(&mut s, gear_herdr::AgentStatus::Idle);
    let (nudged, flagged) = nudge_stopped_reviewers(&mut s, "p", &seen, COOLDOWN, t).unwrap();
    assert_eq!((nudged, flagged), (vec![seat.id.clone()], vec![]));
    let op = s.read(|tx| tx.last_op(&seat.id, OpKind::Deliver)).unwrap().unwrap();
    let text = op.payload["text"].as_str().unwrap();
    assert!(text.starts_with("You are idle and your direction check"), "{text}");
    assert!(text.contains("Review T-7"), "the packet rides along: {text}");
    assert_eq!(op.payload["nudge"], 1);
}

/// A direction seat that handed its result in is left alone.
#[test]
fn a_direction_seat_whose_result_is_in_is_left_alone() {
    let (mut s, seat) = direction_world();
    s.write(|tx| {
        tx.insert_message(&gear_store::NewMessage { project: "p".into(), thread: "result:1".into(),
            task: Some("T-7".into()), attempt: Some(1), sender: seat.id.clone(), sender_gen: 1,
            recipient: "p:captain".into(), kind: gear_model::MsgKind::Result, urgent: false,
            body: "ok".into(), reply_to: None, needs_ack: false, state: gear_model::MsgState::Info,
            created: T0 + 5, meta: Some(serde_json::json!({"direction": true, "sha": "abc1234"}).to_string()) })
    })
    .unwrap();
    let before = s.read(|tx| tx.ops_of_seat_kind(&seat.id, OpKind::Deliver)).unwrap().len();
    let seen = observed(&mut s, gear_herdr::AgentStatus::Idle);
    assert_eq!(
        nudge_stopped_reviewers(&mut s, "p", &seen, COOLDOWN, T0 + COOLDOWN + 1).unwrap(),
        (vec![], vec![])
    );
    let after = s.read(|tx| tx.ops_of_seat_kind(&seat.id, OpKind::Deliver)).unwrap().len();
    assert_eq!(after, before, "no nudge is sent");
}

/// A direction seat that is working is left alone.
#[test]
fn a_working_direction_seat_is_left_alone() {
    let (mut s, seat) = direction_world();
    let before = s.read(|tx| tx.ops_of_seat_kind(&seat.id, OpKind::Deliver)).unwrap().len();
    let seen = observed(&mut s, gear_herdr::AgentStatus::Working);
    assert_eq!(
        nudge_stopped_reviewers(&mut s, "p", &seen, COOLDOWN, T0 + COOLDOWN + 1).unwrap(),
        (vec![], vec![])
    );
    let after = s.read(|tx| tx.ops_of_seat_kind(&seat.id, OpKind::Deliver)).unwrap().len();
    assert_eq!(after, before, "no nudge is sent");
}
