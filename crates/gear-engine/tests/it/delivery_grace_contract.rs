//! T-150: the delivery confirmation (design §10.3) and the taskless doorbell
//! (design §10.4).
//!
//! - the confirmation window is wall clock (`timing.delivery_grace_s`), not a
//!   number of checks: a lane that reads its packet a minute later still
//!   confirms it;
//! - a check-in view that arrives **after** the delivery op failed confirms
//!   the attempt anyway and closes the stall item;
//! - the engine never delivers a second packet to an attempt that already
//!   started;
//! - a doorbell to a lane with no open attempt says so and tells it to stop.

use gear_app::tasks::start_attempt;
use gear_engine::dispatch::{decide, lane_view, DispatchCfg};
use gear_engine::doorbell::{text, DoorbellItem};
use gear_engine::ops::{confirm_late_views, confirm_pending_with};
use gear_engine::stall;
use gear_model::{AttemptState, MsgState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, ContractRow, NewOp, Store, TaskRow, ViewRecord};

const T0: i64 = 1_800_000_000;
const LANE: &str = "p:heat";

/// A project with one lane and a task it was handed: the attempt waits for
/// its check-in and the delivery op is on its way to the pane.
fn world() -> (Store, String) {
    let mut s = Store::open_in_memory().unwrap();
    let id = s
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "opencode", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            tx.insert_task(&TaskRow {
                project: "p".into(),
                id: "T-7".into(),
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
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                updated: T0,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-7".into(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
            })?;
            start_attempt(tx, "p", "T-7", LANE, 1, T0)?;
            let op = tx.insert_op(&NewOp {
                project: "p".into(),
                seat: LANE.into(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some("T-7".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({"text": "New task T-7"}),
                created: T0 + 10,
            })?;
            tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 10)?;
            tx.set_op_state("p", &op.id, OpState::Submitted, None, T0 + 11)?;
            Ok(op.id)
        })
        .unwrap();
    (s, id)
}

/// The lane's check-in: the home page shows the task and the attempt.
fn view(s: &mut Store, ts: i64, task: Option<&str>, attempt: Option<i64>) {
    s.write(|tx| {
        tx.insert_view(&ViewRecord {
            project: "p".into(),
            seat: LANE.into(),
            generation: 1,
            ts,
            shown_msgs: vec![],
            shown_ops: vec![],
            shown_task: task.map(String::from),
            shown_attempt: attempt,
            shown_contract: attempt.map(|_| 1),
        })
    })
    .unwrap();
}

/// T-150: a view 100 s after the delivery confirms it — the window is wall
/// clock, so the packet is not "unconfirmed" after three checks.
#[test]
fn a_view_after_a_minute_confirms_the_delivery() {
    let (mut s, op) = world();

    // Three checks later the delivery is still on its way: the window is
    // 180 s, not three checks of 4 s.
    for at in [T0 + 20, T0 + 30, T0 + 40] {
        let (confirmed, flagged) = confirm_pending_with(&mut s, "p", 3, 180, at).unwrap();
        assert!(confirmed.is_empty(), "{confirmed:?}");
        assert!(flagged.is_empty(), "the seat is not flagged: {flagged:?}");
        assert_eq!(
            s.read(|tx| tx.get_op("p", &op)).unwrap().unwrap().state,
            OpState::Submitted
        );
    }

    // The lane reads its packet 100 s after it was sent.
    view(&mut s, T0 + 111, Some("T-7"), Some(1));
    let (confirmed, flagged) = confirm_pending_with(&mut s, "p", 3, 180, T0 + 120).unwrap();
    assert_eq!(confirmed, vec![op.clone()]);
    assert!(flagged.is_empty());
    assert_eq!(
        s.read(|tx| tx.get_op("p", &op)).unwrap().unwrap().state,
        OpState::Confirmed
    );
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-7", 1)).unwrap().unwrap().state,
        AttemptState::Started
    );
}

/// T-150: a view after the op failed confirms the attempt anyway and closes
/// the stall item.
#[test]
fn a_late_check_in_confirms_the_attempt_and_closes_the_stall() {
    let (mut s, op) = world();
    // The captain's item about the lane being stuck (the tick opens it when
    // the delivery fails).
    s.write(|tx| stall::alert(tx, LANE, T0 + 30)).unwrap();
    let items = s.read(|tx| tx.thread_messages("p", "stall:p:heat")).unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].state, MsgState::Open);

    // The window (8 s) is over: the delivery fails and the seat is flagged.
    let (confirmed, flagged) = confirm_pending_with(&mut s, "p", 3, 8, T0 + 30).unwrap();
    assert!(confirmed.is_empty());
    assert_eq!(flagged, vec![LANE.to_string()]);
    assert_eq!(
        s.read(|tx| tx.get_op("p", &op)).unwrap().unwrap().state,
        OpState::Failed
    );

    // The lane checks in after all — late, but it read the packet.
    view(&mut s, T0 + 40, Some("T-7"), Some(1));
    let late = confirm_late_views(&mut s, "p", T0 + 50).unwrap();
    assert_eq!(late.len(), 1, "{late:?}");
    assert!(
        late[0].contains("T-7/1"),
        "the attempt is named: {late:?}"
    );
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-7", 1)).unwrap().unwrap().state,
        AttemptState::Started,
        "the attempt starts after all"
    );
    let items = s.read(|tx| tx.thread_messages("p", "stall:p:heat")).unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(
        items[0].state,
        MsgState::Resolved,
        "the lane is not stuck any more: {items:?}"
    );
}

/// T-150: the engine never creates a Deliver op for an attempt that already
/// started (one packet per round). The same lane with an attempt that still
/// waits for its check-in is nudged as before.
#[test]
fn a_started_attempt_never_gets_a_second_deliver() {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        tx.insert_task(&TaskRow {
            project: "p".into(),
            id: "T-7".into(),
            rank: 1,
            state: TaskState::Running,
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
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            updated: T0,
        })?;
        tx.insert_attempt(&AttemptRow {
            project: "p".into(),
            task: "T-7".into(),
            n: 1,
            seat: LANE.into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state: AttemptState::Started,
            candidate: None,
            report: None,
            safe_point: None,
            started: T0,
            updated: T0,
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
        })
    })
    .unwrap();

    let seat = s.read(|tx| tx.get_seat(LANE)).unwrap().unwrap();
    let obs = gear_engine::observe::SeatObs {
        seat: seat.clone(),
        presence: gear_engine::observe::Presence::Present,
        status: Some(gear_herdr::AgentStatus::Idle),
        flags: Default::default(),
        idle: true,
        screen: None,
        session: None,
    };
    let view = s
        .read(|tx| lane_view(tx, &seat, Some(obs.clone()), T0))
        .unwrap();
    assert_eq!(view.attempt.as_ref().map(|a| a.state), Some(AttemptState::Started));
    let decision = decide(&view, &DispatchCfg::default(), T0);
    assert!(
        matches!(decision, gear_engine::dispatch::Action::None { .. }),
        "a started attempt gets no second packet: {decision:?}"
    );

    // The same lane with an attempt that still waits for its check-in is
    // nudged, as before.
    s.write(|tx| {
        let mut attempt = tx.get_attempt("p", "T-7", 1)?.unwrap();
        attempt.state = AttemptState::Delivered;
        tx.update_attempt(&attempt)
    })
    .unwrap();
    let view = s
        .read(|tx| lane_view(tx, &seat, Some(obs), T0))
        .unwrap();
    let decision = decide(&view, &DispatchCfg::default(), T0);
    assert_eq!(
        decision,
        gear_engine::dispatch::Action::Continue { task: "T-7".into() },
        "an attempt that waits for its check-in is still nudged"
    );
}

/// T-150 (design §10.4): the doorbell text of a lane with no open attempt.
#[test]
fn the_taskless_doorbell_text_says_so() {
    let items = vec![
        DoorbellItem { id: "M-12".into(), urgent: false, at_once: false },
        DoorbellItem { id: "M-13".into(), urgent: true, at_once: false },
    ];
    let said = text(&items, true);
    assert_eq!(
        said,
        "GEAR: 2 item(s) for you: M-12 M-13. Run gear, answer them, then end your turn. You have no task now: do not continue earlier work."
    );
    assert!(!said.contains('\n'), "one line: {said}");

    // With a task, the usual text is unchanged.
    let said = text(&items, false);
    assert_eq!(
        said,
        "GEAR: 2 item(s) waiting for you (1 urgent): M-12 M-13. Run `gear` now."
    );
}
