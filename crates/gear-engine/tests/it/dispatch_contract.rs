//! Contract for gear_engine::dispatch::decide (T-35, design/09). Pure.

use gear_engine::dispatch::{decide, Action, DispatchCfg, LaneView};
use gear_engine::observe::{Presence, SeatObs};
use gear_herdr::{AgentStatus, ScreenFlag};
use gear_model::{AttemptState, OpKind, OpState, SeatRole, SeatState, TaskState};
use gear_store::{AttemptRow, OpRow, SeatRow, TaskRow};
use std::collections::BTreeSet;

const NOW: i64 = 1_800_000_000;

fn seat(state: SeatState, role: SeatRole) -> SeatRow {
    SeatRow { id: "p:heat".into(), project: "p".into(), role, name: "heat".into(), profile: "opencode".into(),
        generation: 1, state, state_since: NOW - 1000, state_reason: None, task_id: None }
}

fn obs(state: SeatState, idle: bool, flags: &[ScreenFlag]) -> SeatObs {
    SeatObs { seat: seat(state, SeatRole::Lane), presence: Presence::Present,
        status: Some(if idle { AgentStatus::Idle } else { AgentStatus::Working }),
        flags: flags.iter().copied().collect::<BTreeSet<_>>(), idle, screen: None, session: None }
}

fn task(id: &str) -> TaskRow {
    TaskRow { repo: None, base: None, gate: None, rework_estimate_min: None, project: "p".into(), id: id.into(), rank: 1, state: TaskState::Ready, priority: "must".into(),
        lane: Some("heat".into()), milestone: None, required: false, contract_version: 1, deps: vec![],
        followup_of: None, wait_on: None, wait_cond: None, rework_note: None, estimate_min: None,  issue: None,updated: NOW }
}

fn attempt(task: &str, state: AttemptState) -> AttemptRow {
    AttemptRow { session_ref: None, session_cli: None, session_action: None, sent_at: None, project: "p".into(), task: task.into(), n: 1, seat: "p:heat".into(), generation: 1,
        contract_version: 1, adopted_version: 1, pending_update: None, state, candidate: None,
        report: None, safe_point: None, started: NOW - 500, updated: NOW - 500 }
}

fn op() -> OpRow {
    OpRow { project: "p".into(), id: "O-1".into(), seat: "p:heat".into(), generation: 1, kind: OpKind::Deliver,
        task: Some("T-1".into()), attempt: Some(1), ref_msg: None, plan_version: None,
        payload: serde_json::json!({}), state: OpState::Unknown, result: None, check_budget: 0,
        created: NOW - 10, updated: NOW - 10 }
}

fn view() -> LaneView {
    LaneView { obs: Some(obs(SeatState::Active, true, &[])), attempt: None, pending_op: None,
        last_deliver_at: Some(NOW - 1000), nudges_since_progress: 0, ready_tasks: vec![task("T-2"), task("T-3")],
        safe_for_next: false, switch_pending: false, failed_delivery: None, rework_mid_turn: false }
}

fn is_none(a: &Action) -> bool {
    matches!(a, Action::None { reason } if !reason.is_empty())
}

#[test]
fn idle_lane_without_attempt_gets_first_ready_task() {
    assert_eq!(decide(&view(), &DispatchCfg::default(), NOW), Action::Assign { task: "T-2".into() });
    let mut v = view();
    v.ready_tasks.clear();
    assert!(is_none(&decide(&v, &DispatchCfg::default(), NOW)));
}

#[test]
fn gates_before_any_action() {
    let cfg = DispatchCfg::default();
    for st in [SeatState::Parked, SeatState::Held, SeatState::Missing, SeatState::NeedsCheck, SeatState::Handover] {
        let mut v = view();
        v.obs = Some(obs(st, true, &[]));
        assert!(is_none(&decide(&v, &cfg, NOW)), "{st:?}");
    }
    let mut v = view(); v.obs = None;
    assert!(is_none(&decide(&v, &cfg, NOW)), "observation failed");
    let mut v = view(); v.pending_op = Some(op()); v.last_deliver_at = Some(NOW - 100_000);
    assert!(is_none(&decide(&v, &cfg, NOW)), "pending delivery, even past cooldown");
    let mut v = view(); v.obs = Some(obs(SeatState::Active, false, &[]));
    assert!(is_none(&decide(&v, &cfg, NOW)), "busy");
    for f in [ScreenFlag::RateLimit, ScreenFlag::Quota, ScreenFlag::Auth] {
        let mut v = view(); v.obs = Some(obs(SeatState::Active, true, &[f]));
        assert!(is_none(&decide(&v, &cfg, NOW)), "{f:?}");
    }
    let mut v = view(); v.last_deliver_at = Some(NOW - 89);
    assert!(is_none(&decide(&v, &cfg, NOW)), "cooldown");
    v.last_deliver_at = Some(NOW - 90);
    assert_eq!(decide(&v, &cfg, NOW), Action::Assign { task: "T-2".into() }, "cooldown boundary");
}

/// T-150 (design §10.3): an attempt that already started got its packet and
/// the lane checked in — the engine never delivers it a second one. A stopped
/// attempt is still continued (and escalated when nothing moves).
#[test]
fn a_started_attempt_gets_no_second_packet_and_a_stopped_one_is_continued() {
    let cfg = DispatchCfg::default();
    let mut v = view();
    v.attempt = Some(attempt("T-1", AttemptState::Started));
    assert!(
        is_none(&decide(&v, &cfg, NOW)),
        "no second packet for a started attempt: {:?}",
        decide(&v, &cfg, NOW)
    );
    v.attempt = Some(attempt("T-1", AttemptState::Stopped));
    assert_eq!(decide(&v, &cfg, NOW), Action::Continue { task: "T-1".into() });
    v.nudges_since_progress = 5;
    assert!(matches!(decide(&v, &cfg, NOW), Action::Escalate { task, reason } if task == "T-1" && !reason.is_empty()));
}

#[test]
fn submitted_attempt_assigns_only_when_safe() {
    let cfg = DispatchCfg::default();
    let mut v = view(); v.attempt = Some(attempt("T-1", AttemptState::Submitted));
    assert!(is_none(&decide(&v, &cfg, NOW)));
    v.safe_for_next = true;
    assert_eq!(decide(&v, &cfg, NOW), Action::Assign { task: "T-2".into() });
}

#[test]
fn retiring_and_temp_seats_never_assign() {
    let cfg = DispatchCfg::default();
    let mut v = view(); v.obs = Some(obs(SeatState::Retiring, true, &[]));
    assert!(is_none(&decide(&v, &cfg, NOW)));
    v.attempt = Some(attempt("T-1", AttemptState::Stopped));
    assert_eq!(decide(&v, &cfg, NOW), Action::Continue { task: "T-1".into() });
    let mut v = view();
    let mut o = obs(SeatState::Active, true, &[]); o.seat.role = SeatRole::Temp; v.obs = Some(o);
    assert!(is_none(&decide(&v, &cfg, NOW)));
}

/// T-290: a rework packet reaches a lane whose agent queues input while it
/// works (pi, claude) — the packet waits in the input queue and is read after
/// the current step. A first packet, and a lane that does not queue, wait for
/// idle as before; the gates after rule 4 still hold.
#[test]
fn a_rework_reaches_a_working_lane_that_queues_input() {
    let cfg = DispatchCfg::default();
    let working = || obs(SeatState::Active, false, &[]);

    let mut v = view();
    v.obs = Some(working());
    v.rework_mid_turn = true;
    assert_eq!(
        decide(&v, &cfg, NOW),
        Action::Assign { task: "T-2".into() },
        "the first ready task (the rework) is assigned"
    );

    // No rework: the working agent is not ready.
    let mut v = view();
    v.obs = Some(working());
    assert!(is_none(&decide(&v, &cfg, NOW)));

    // Rules 5 and 6 still hold for a rework mid-turn.
    let mut v = view();
    v.obs = Some(obs(SeatState::Active, false, &[ScreenFlag::RateLimit]));
    v.rework_mid_turn = true;
    assert!(is_none(&decide(&v, &cfg, NOW)), "a ratelimit wall still holds");
    let mut v = view();
    v.obs = Some(working());
    v.rework_mid_turn = true;
    v.last_deliver_at = Some(NOW - 89);
    assert!(is_none(&decide(&v, &cfg, NOW)), "the cooldown still holds");
    v.last_deliver_at = Some(NOW - 90);
    assert_eq!(decide(&v, &cfg, NOW), Action::Assign { task: "T-2".into() });
}
