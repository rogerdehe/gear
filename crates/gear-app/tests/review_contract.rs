//! Contract for `gear send --kind review` (T-2): a temp reviewer hands in a
//! verdict on one attempt. The verdict is stored on the attempt's review
//! record and reaches the captain as an action item; it settles nothing.

use gear_app::home;
use gear_app::identity::Fingerprint;
use gear_app::mail::{self, SendArgs};
use gear_app::review::Outcome;
use gear_app::tasks::{on_delivery_confirmed, start_attempt};
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, MsgKind, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const TEMP_PANE: &str = "w1:p3";

/// Project `p`: a captain, the lane that wrote T-1 and a temp reviewer bound
/// to T-1. T-1 has one started attempt.
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
        testing::seed_temp_seat(tx, "p", "r-1", "T-1", TEMP_PANE, "opencode", T0)?;
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

fn env(seat: &str, pane: &str) -> Env {
    Env { ctx: Some(GearCtx { project: "p".into(), seat: seat.into(), generation: 1 }), pane_id: Some(pane.into()), admin: false }
}

fn fp(pane: &str, kind: &str) -> Fingerprint {
    Fingerprint { pane_id: pane.into(), kind: Some(kind.into()), session_ref: None }
}

fn review(outcome: Outcome, findings: &str, rid: &str) -> SendArgs {
    SendArgs { kind: MsgKind::Review, body: findings.into(), task: Some("T-1".into()), attempt: Some(1),
        contract: None, candidate: None, verdict: Some(outcome), about: None, to: None, reply_to: None,
        urgent: false, request_id: rid.into() }
}

#[test]
fn a_temp_review_is_stored_and_reaches_the_captain() {
    let mut s = world();

    let sent = mail::send(&mut s, &env("r-1", TEMP_PANE), Some(&fp(TEMP_PANE, "opencode")),
        &review(Outcome::Pass, "ops/reviews/T-1.md", "r1"), T0 + 5).unwrap();
    assert_eq!(sent.recipient, "p:captain");

    // The verdict sits on the attempt's review record.
    let record = s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().unwrap();
    assert!(record.passed);
    assert_eq!(record.findings, "ops/reviews/T-1.md");
    assert_eq!(record.by_seat, "p:r-1");

    // The captain's inbox holds it as an open item, with the verdict in it.
    let inbox = s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].kind, MsgKind::Review);
    assert_eq!(inbox[0].task.as_deref(), Some("T-1"));
    assert!(inbox[0].body.contains("pass"), "{}", inbox[0].body);
    assert!(inbox[0].body.contains("ops/reviews/T-1.md"), "{}", inbox[0].body);

    // The captain sees it on the home page.
    let home = home::build(&mut s, &env("captain", CAP_PANE), Some(&fp(CAP_PANE, "codex")), T0 + 7).unwrap();
    assert!(home.text.contains("ops/reviews/T-1.md"), "{}", home.text);
    assert!(home.text.contains("INBOX"), "{}", home.text);

    // The task packet shows the review record.
    let packet = s.read(|tx| gear_app::plan::render_task_packet(tx, "p", "T-1")).unwrap();
    assert!(packet.contains("## Review"), "{packet}");
    assert!(packet.contains("pass"), "{packet}");
    assert!(packet.contains("ops/reviews/T-1.md"), "{packet}");

    // Nothing is settled: the attempt and the task keep their state.
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Started);
    assert_eq!(s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state, TaskState::Running);
}

#[test]
fn a_fail_verdict_is_stored_as_a_fail() {
    let mut s = world();
    mail::send(&mut s, &env("r-1", TEMP_PANE), Some(&fp(TEMP_PANE, "opencode")),
        &review(Outcome::Fail, "P1: the store forgets the verdict", "r1"), T0 + 5).unwrap();
    let record = s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().unwrap();
    assert!(!record.passed);
    let inbox = s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap();
    assert!(inbox[0].body.contains("fail"), "{}", inbox[0].body);
}

#[test]
fn a_lane_seat_may_not_hand_in_a_verdict() {
    let mut s = world();
    let err = mail::send(&mut s, &env("heat", LANE_PANE), Some(&fp(LANE_PANE, "opencode")),
        &review(Outcome::Pass, "looks fine to me", "r1"), T0 + 5).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    // Nothing was written: no record, no item in the captain's inbox.
    assert!(s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().is_none());
    assert!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap().is_empty());
}

#[test]
fn a_temp_seat_bound_to_another_task_may_not_review() {
    let mut s = world();
    // The reviewer of T-1 names a task it is not bound to.
    let mut args = review(Outcome::Pass, "looks fine to me", "r1");
    args.task = Some("T-2".into());
    let err = mail::send(&mut s, &env("r-1", TEMP_PANE), Some(&fp(TEMP_PANE, "opencode")),
        &args, T0 + 5).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoTarget);
    assert!(s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().is_none());
    assert!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap().is_empty());
}

#[test]
fn a_review_of_an_unknown_task_is_no_target() {
    let mut s = world();
    let mut args = review(Outcome::Pass, "looks fine to me", "r1");
    args.task = Some("T-9".into());
    let err = mail::send(&mut s, &env("r-1", TEMP_PANE), Some(&fp(TEMP_PANE, "opencode")),
        &args, T0 + 5).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoTarget);
}

#[test]
fn a_review_without_a_verdict_is_bad_format() {
    let mut s = world();
    let mut args = review(Outcome::Pass, "looks fine to me", "r1");
    args.verdict = None;
    let err = mail::send(&mut s, &env("r-1", TEMP_PANE), Some(&fp(TEMP_PANE, "opencode")),
        &args, T0 + 5).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("--verdict"), "{}", err.detail);
}

#[test]
fn the_same_request_id_replays_without_a_second_record() {
    let mut s = world();
    let (env, fp) = (env("r-1", TEMP_PANE), fp(TEMP_PANE, "opencode"));
    let args = review(Outcome::Pass, "ops/reviews/T-1.md", "r1");
    mail::send(&mut s, &env, Some(&fp), &args, T0 + 5).unwrap();
    let again = mail::send(&mut s, &env, Some(&fp), &args, T0 + 6).unwrap();
    assert!(again.replayed);
    assert_eq!(s.read(|tx| tx.review_verdicts_of_task("p", "T-1")).unwrap().len(), 1);
}

#[test]
fn a_second_different_verdict_for_the_same_attempt_is_refused() {
    let mut s = world();
    let (env, fp) = (env("r-1", TEMP_PANE), fp(TEMP_PANE, "opencode"));
    mail::send(&mut s, &env, Some(&fp), &review(Outcome::Pass, "ops/reviews/T-1.md", "r1"), T0 + 5).unwrap();
    let err = mail::send(&mut s, &env, Some(&fp), &review(Outcome::Fail, "P1: wrong", "r2"), T0 + 6).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadTransition);
    let record = s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().unwrap();
    assert!(record.passed, "the first verdict stands");
}
