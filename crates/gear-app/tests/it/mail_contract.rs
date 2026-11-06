//! Contract: a `gear send` for a task the lane has not been given says what
//! to do (T-613, the T-778 case). A task that is ready for the lane but has no
//! attempt yet has not been delivered: the send fails with NO_TARGET and the
//! text that tells the lane to end its turn.

use gear_app::identity::Fingerprint;
use gear_app::mail::{self, SendArgs};
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, MsgKind, SeatRole, TaskState};
use gear_store::{testing, Store, TaskRow};

const NOW: i64 = 1_800_000_000;
const LANE_PANE: &str = "w1:p2";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", NOW)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", NOW)?;
        // A task ready for the lane, with no attempt: not delivered yet.
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
            updated: NOW,
        })?;
        Ok(())
    })
    .unwrap();
    s
}

fn lane() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx {
                project: "p".into(),
                seat: "heat".into(),
                generation: 1,
            }),
            pane_id: Some(LANE_PANE.into()),
            admin: false,
        },
        Fingerprint {
            pane_id: LANE_PANE.into(),
            kind: Some("opencode".into()),
            session_ref: None,
        },
    )
}

fn result() -> SendArgs {
    SendArgs {
        kind: MsgKind::Result,
        body: "Report: gear/voyages/M6.3/reports/T-1.md".into(),
        task: Some("T-1".into()),
        attempt: Some(1),
        contract: Some(1),
        candidate: Some("abc".into()),
        about: None,
        to: None,
        reply_to: None,
        verdict: None,
        blocking: None,
        base: None,
        urgent: false,
        left: None,
        blocker: None,
        on: None,
        friction: None,
        request_id: "r1".into(),
    }
}

#[test]
fn a_send_for_an_undelivered_ready_task_says_what_to_do() {
    let mut s = world();
    let (env, fp) = lane();
    let err = mail::send(&mut s, &env, Some(&fp), &result(), NOW + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoTarget);
    assert_eq!(
        err.detail,
        "T-1 is not delivered yet: end your turn; GEAR delivers it when you are idle"
    );
}

/// The same text for a declared wait (`gear send --kind blocked`) on a task
/// the lane has not been given.
#[test]
fn a_blocked_send_for_an_undelivered_ready_task_says_what_to_do() {
    let mut s = world();
    let (env, fp) = lane();
    let mut args = result();
    args.kind = MsgKind::Blocked;
    args.contract = Some(1);
    args.candidate = None;
    args.on = Some("external".into());
    let err = mail::send(&mut s, &env, Some(&fp), &args, NOW + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoTarget);
    assert_eq!(
        err.detail,
        "T-1 is not delivered yet: end your turn; GEAR delivers it when you are idle"
    );
}
