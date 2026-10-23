//! Contract for `gear send --kind result` (T-45, design/06 submit_result via
//! mail::send). Needs T-42 (attempt lifecycle).

use gear_app::identity::Fingerprint;
use gear_app::mail::{self, SendArgs};
use gear_app::tasks::{on_delivery_confirmed, start_attempt};
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, MsgKind, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const LANE_PANE: &str = "w1:p2";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
        tx.insert_task(&TaskRow { project: "p".into(), id: "T-1".into(), rank: 1, state: TaskState::Ready, priority: "must".into(),
            lane: Some("heat".into()), milestone: None, required: false, contract_version: 1, deps: vec![], followup_of: None,
            wait_on: None, wait_cond: None, rework_note: None, updated: T0 })?;
        tx.insert_contract(&ContractRow { project: "p".into(), task: "T-1".into(), version: 1, goal: "g".into(), accept: "a".into(),
            evidence_files: vec![], evidence_due_min: None, body: String::new(), created: T0 })?;
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)
    }).unwrap();
    s
}

fn lane() -> (Env, Fingerprint) {
    (Env { ctx: Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 1 }), pane_id: Some(LANE_PANE.into()), admin: false },
     Fingerprint { pane_id: LANE_PANE.into(), kind: Some("opencode".into()), session_ref: None })
}

fn result(contract: i64, candidate: Option<&str>, rid: &str) -> SendArgs {
    SendArgs { kind: MsgKind::Result, body: "Report: ops/tasks/T-1.md".into(), task: Some("T-1".into()), attempt: Some(1),
        contract: Some(contract), candidate: candidate.map(String::from), about: None, to: None, reply_to: None,
        verdict: None, urgent: false, request_id: rid.into() }
}

#[test]
fn result_submits_the_attempt_and_reaches_the_captain() {
    let mut s = world();
    let (env, fp) = lane();
    let sent = mail::send(&mut s, &env, Some(&fp), &result(1, Some("abc1234"), "r1"), T0 + 5).unwrap();
    assert_eq!(sent.recipient, "p:captain");
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((a.state, a.candidate.as_deref(), a.report.as_deref()), (AttemptState::Submitted, Some("abc1234"), Some("Report: ops/tasks/T-1.md")));
    assert_eq!(s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state, TaskState::Verifying);
    let again = mail::send(&mut s, &env, Some(&fp), &result(1, Some("abc1234"), "r1"), T0 + 6).unwrap();
    assert!(again.replayed, "same request id replays, no second submit");
}

#[test]
fn bad_result_writes_nothing() {
    let mut s = world();
    let (env, fp) = lane();
    assert_eq!(mail::send(&mut s, &env, Some(&fp), &result(1, None, "a"), T0).unwrap_err().code, ErrorCode::BadFormat, "no candidate");
    assert_eq!(mail::send(&mut s, &env, Some(&fp), &result(2, Some("c"), "b"), T0).unwrap_err().code, ErrorCode::BadFormat, "wrong contract");
    assert_eq!(s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap().state, AttemptState::Started);
    assert!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 1)).unwrap().is_empty(), "no message stored");
}
