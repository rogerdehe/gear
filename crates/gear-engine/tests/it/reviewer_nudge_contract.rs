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
            tx.insert_task(&TaskRow { project: "p".into(), id: "T-7".into(), rank: 1, state: TaskState::Ready,
                priority: "must".into(), lane: Some("heat".into()), milestone: None, required: false, contract_version: 1,
                deps: vec![], followup_of: None, wait_on: None, wait_cond: None, rework_note: None, estimate_min: None, updated: T0 })?;
            tx.insert_contract(&ContractRow { project: "p".into(), task: "T-7".into(), version: 1, goal: "g".into(),
                accept: "a".into(), evidence_files: vec![], evidence_due_min: None, body: String::new(), created: T0 })?;
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
    vec![SeatObs { seat: seat.clone(), presence: Presence::Present, status: None, flags: BTreeSet::new(), idle: true, screen: None }]
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
