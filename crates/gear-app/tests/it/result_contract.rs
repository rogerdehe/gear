//! Contract for `gear send --kind result` (T-45, design/06 submit_result via
//! mail::send). Needs T-42 (attempt lifecycle).

use gear_app::identity::Fingerprint;
use gear_app::mail::{self, SendArgs};
use gear_app::tasks::{on_delivery_confirmed, settle_review, start_attempt};
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, MsgKind, SeatRole, TaskState};
use gear_store::{testing, ContractRow, ReviewVerdictRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const LANE_PANE: &str = "w1:p2";

fn world_initial(started: bool) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
        tx.insert_task(&TaskRow { repo: None, base: None, gate: None, rework_estimate_min: None, project: "p".into(), id: "T-1".into(), rank: 1, state: TaskState::Ready, priority: "must".into(),
            lane: Some("heat".into()), milestone: None, required: false, contract_version: 1, deps: vec![], followup_of: None,
            wait_on: None, wait_cond: None, rework_note: None, estimate_min: None,  issue: None,updated: T0 })?;
        tx.insert_contract(&ContractRow { project: "p".into(), task: "T-1".into(), version: 1, goal: "g".into(), accept: "a".into(),
            evidence_files: vec![], evidence_due_min: None, body: String::new(), created: T0, mechanical: None, })?;
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        if started { on_delivery_confirmed(tx, "p", "T-1", 1, T0)?; }
        Ok(())
    }).unwrap();
    s
}

fn world() -> Store { world_initial(true) }

fn lane() -> (Env, Fingerprint) {
    (Env { ctx: Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 1 }), pane_id: Some(LANE_PANE.into()), admin: false },
     Fingerprint { pane_id: LANE_PANE.into(), kind: Some("opencode".into()), session_ref: None })
}

fn result(contract: i64, candidate: Option<&str>, rid: &str) -> SendArgs {
    SendArgs { kind: MsgKind::Result, body: "Report: ops/tasks/T-1.md".into(), task: Some("T-1".into()), attempt: Some(1),
        contract: Some(contract), candidate: candidate.map(String::from), about: None, to: None, reply_to: None,
        verdict: None, blocking: None, base: None, urgent: false,
        left: None,
        blocker: None,
        on: None,
        friction: None, request_id: rid.into() }
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

/// T-118: a candidate that already failed is not a rework. The lane that
/// hands the same one in again read the first report, found nothing to fix
/// and loops (trial B02-06): the hand-in is refused and the refusal names
/// the attempt that failed on it.
#[test]
fn a_result_with_an_already_failed_candidate_is_refused() {
    let mut s = world();
    let (env, fp) = lane();
    // The first round: the candidate is handed in and the review fails.
    mail::send(&mut s, &env, Some(&fp), &result(1, Some("aaa1111"), "r1"), T0 + 5).unwrap();
    s.write(|tx| {
        tx.insert_review_verdict(&ReviewVerdictRow {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            passed: false,
            findings: "F1: the rework is missing".into(),
            by_seat: "p:r-1".into(),
            created: T0 + 6,
        })?;
        tx.set_review_blocking("p", "T-1", 1, "F1", T0 + 6)?;
        settle_review(tx, "p", "T-1", 1, T0 + 7)?;
        // The lane starts the rework and works on it.
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + 8)?;
        on_delivery_confirmed(tx, "p", "T-1", 2, T0 + 8)
    })
    .unwrap();

    // The same candidate again: refused, and the refusal says which attempt
    // failed on it.
    let args = SendArgs { attempt: Some(2), candidate: Some("aaa1111".into()), request_id: "r2".into(), ..result(1, None, "") };
    let error = mail::send(&mut s, &env, Some(&fp), &args, T0 + 9).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(error.detail.contains("aaa1111"), "{}", error.detail);
    assert!(error.detail.contains("attempt 1"), "{}", error.detail);
    // Nothing was written: attempt 2 is still started, the task is running.
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 2)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Started, "the hand-in was refused");
    assert_eq!(s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state, TaskState::Running);

    // A new candidate: the hand-in goes through.
    let args = SendArgs { attempt: Some(2), candidate: Some("bbb2222".into()), request_id: "r3".into(), ..result(1, None, "") };
    mail::send(&mut s, &env, Some(&fp), &args, T0 + 10).unwrap();
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 2)).unwrap().unwrap();
    assert_eq!((attempt.state, attempt.candidate.as_deref()), (AttemptState::Submitted, Some("bbb2222")));
    assert_eq!(s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state, TaskState::Verifying);
}

#[test]
fn delivered_owner_can_hand_in_and_refreshes_session_atomically() {
    let mut s=world_initial(false);
    s.write(|tx| {
        tx.set_attempt_session("p","T-1",1,"old-session","opencode")?;
        tx.set_binding_session_ref("p:heat",1,"new-session")
    }).unwrap();
    let (env,fp)=lane(); mail::send(&mut s,&env,Some(&fp),&result(1,Some("abc1234"),"delivered-result"),T0+20).unwrap();
    let attempt=s.read(|tx|tx.get_attempt("p","T-1",1)).unwrap().unwrap();
    assert_eq!(attempt.state,AttemptState::Submitted); assert_eq!(attempt.session_ref.as_deref(),Some("new-session"));
    assert_eq!(attempt.session_cli.as_deref(),Some("opencode"));
    let times=s.read(|tx|tx.attempt_times("p","T-1",1)).unwrap();
    assert_eq!(times.started,Some(T0+20)); assert_eq!(times.handed_in,Some(T0+20));
    assert_eq!(s.read(|tx|tx.get_task("p","T-1")).unwrap().unwrap().state,TaskState::Verifying);
}

#[test]
fn another_seat_cannot_hand_in_a_delivered_attempt() {
    let mut s=world_initial(false);
    s.write(|tx| {
        testing::seed_active_seat(tx,"p","other",SeatRole::Lane,"w1:p3","opencode",T0)
    }).unwrap();
    let env=Env {ctx:Some(GearCtx{project:"p".into(),seat:"other".into(),generation:1}),pane_id:Some("w1:p3".into()),admin:false};
    let fp=Fingerprint{pane_id:"w1:p3".into(),kind:Some("opencode".into()),session_ref:None};
    assert_eq!(mail::send(&mut s,&env,Some(&fp),&result(1,Some("abc1234"),"other-result"),T0+20).unwrap_err().code,ErrorCode::Forbidden);
    assert_eq!(s.read(|tx|tx.get_attempt("p","T-1",1)).unwrap().unwrap().state,AttemptState::Delivered);
    assert_eq!(s.read(|tx|tx.get_task("p","T-1")).unwrap().unwrap().state,TaskState::Ready);
}

#[test]
fn hand_in_refreshes_session_after_normal_check_in() {
    let mut s=world(); let (env,fp)=lane();
    s.write(|tx| {tx.set_attempt_session("p","T-1",1,"old","opencode")?;tx.set_binding_session_ref("p:heat",1,"new")}).unwrap();
    mail::send(&mut s,&env,Some(&fp),&result(1,Some("abc1234"),"refreshed-session"),T0+20).unwrap();
    assert_eq!(s.read(|tx|tx.get_attempt("p","T-1",1)).unwrap().unwrap().session_ref.as_deref(),Some("new"));
}

#[test]
fn stale_delivered_generation_cannot_be_handed_in() {
    let mut s=world_initial(false);
    s.write(|tx| {let mut a=tx.get_attempt("p","T-1",1)?.unwrap();a.generation=2;tx.update_attempt(&a)}).unwrap();
    let (env,fp)=lane();
    assert_eq!(mail::send(&mut s,&env,Some(&fp),&result(1,Some("abc1234"),"old-generation"),T0+20).unwrap_err().code,ErrorCode::Forbidden);
    assert_eq!(s.read(|tx|tx.get_attempt("p","T-1",1)).unwrap().unwrap().state,AttemptState::Delivered);
}

#[test]
fn rejected_delivered_hand_in_does_not_start_attempt() {
    let mut s=world_initial(false); let (env,fp)=lane();
    assert_eq!(mail::send(&mut s,&env,Some(&fp),&result(2,Some("abc1234"),"bad-delivered-contract"),T0+20).unwrap_err().code,ErrorCode::BadFormat);
    assert_eq!(s.read(|tx|tx.get_attempt("p","T-1",1)).unwrap().unwrap().state,AttemptState::Delivered);
    assert_eq!(s.read(|tx|tx.attempt_times("p","T-1",1)).unwrap().started,None);
}

#[test]
fn current_contract_mismatch_rolls_back_the_implicit_start() {
    let mut s=world_initial(false); let (env,fp)=lane();
    s.write(|tx| {let mut task=tx.get_task("p","T-1")?.unwrap();task.contract_version=2;tx.update_task(&task)}).unwrap();
    assert_eq!(mail::send(&mut s,&env,Some(&fp),&result(1,Some("abc1234"),"new-current-contract"),T0+20).unwrap_err().code,ErrorCode::BadFormat);
    assert_eq!(s.read(|tx|tx.get_attempt("p","T-1",1)).unwrap().unwrap().state,AttemptState::Delivered);
    assert_eq!(s.read(|tx|tx.attempt_times("p","T-1",1)).unwrap().started,None);
    assert_eq!(s.read(|tx|tx.get_task("p","T-1")).unwrap().unwrap().state,TaskState::Ready);
}

#[test]
fn hand_in_clears_a_stale_session_when_current_binding_has_none() {
    let mut s=world(); let (env,fp)=lane();
    s.write(|tx|tx.set_attempt_session("p","T-1",1,"old-session","pi")).unwrap();
    mail::send(&mut s,&env,Some(&fp),&result(1,Some("abc1234"),"missing-current-session"),T0+20).unwrap();
    let attempt=s.read(|tx|tx.get_attempt("p","T-1",1)).unwrap().unwrap();
    assert_eq!(attempt.session_ref,None); assert_eq!(attempt.session_cli.as_deref(),Some("opencode"));
}

/// T-266: after a fail verdict the engine opens the rework and delivers its
/// packet only when the lane is idle. A lane still in its turn hands in the
/// settled attempt again; the refusal must say that a rework waits for it.
fn failed_with_rework_open() -> Store {
    let mut s = world();
    let (env, fp) = lane();
    mail::send(&mut s, &env, Some(&fp), &result(1, Some("aaa1111"), "round1"), T0 + 5).unwrap();
    s.write(|tx| {
        tx.insert_review_verdict(&ReviewVerdictRow {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            passed: false,
            findings: "F1: the rework is missing".into(),
            by_seat: "p:r-1".into(),
            created: T0 + 6,
        })?;
        tx.set_review_blocking("p", "T-1", 1, "F1", T0 + 6)?;
        settle_review(tx, "p", "T-1", 1, T0 + 7)?;
        // The engine opens attempt 2 on the lane's seat.
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + 8)?;
        Ok(())
    })
    .unwrap();
    s
}

#[test]
fn hand_in_after_a_fail_names_the_rework_that_waits() {
    let mut s = failed_with_rework_open();
    let (env, fp) = lane();
    let error = mail::send(&mut s, &env, Some(&fp), &result(1, Some("bbb2222"), "again"), T0 + 9).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadTransition, "{}", error.detail);
    assert_eq!(
        error.detail,
        "a rework of T-1 (attempt 2) is waiting for you: end your turn now and its packet \
         arrives; do not hand in attempt 1 again"
    );
    // Nothing was written: attempt 1 is still the rejected one, attempt 2 still
    // waits for the lane's turn.
    assert_eq!(s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap().state, AttemptState::Rejected);
    assert_eq!(s.read(|tx| tx.get_attempt("p", "T-1", 2)).unwrap().unwrap().state, AttemptState::Delivered);
    assert!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 10)).unwrap().is_empty(), "no message stored");
}

#[test]
fn hand_in_of_the_rework_itself_keeps_its_own_error() {
    let mut s = failed_with_rework_open();
    let (env, fp) = lane();
    // Attempt 2 before its packet was read, handed in with the candidate the
    // first round already failed: the hand-in fails on that, and the refusal
    // stays that one — there is no newer attempt than the one it names.
    let args = SendArgs { attempt: Some(2), candidate: Some("aaa1111".into()), request_id: "r2".into(), ..result(1, None, "") };
    let error = mail::send(&mut s, &env, Some(&fp), &args, T0 + 9).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(error.detail.contains("already failed on attempt 1"), "{}", error.detail);
    assert!(!error.detail.contains("a rework of"), "{}", error.detail);
}

#[test]
fn hand_in_with_no_newer_attempt_keeps_todays_error() {
    let mut s = world();
    let (env, fp) = lane();
    mail::send(&mut s, &env, Some(&fp), &result(1, Some("abc1234"), "first"), T0 + 5).unwrap();
    // No rework was opened: a second hand-in meets the state error it met
    // before T-266.
    let error = mail::send(&mut s, &env, Some(&fp), &result(1, Some("abc1234"), "second"), T0 + 6).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadTransition, "{}", error.detail);
    assert_eq!(error.detail, "attempt: submitted -> submitted");
    assert_eq!(s.read(|tx| tx.attempts_of_task("p", "T-1")).unwrap().len(), 1);
}
