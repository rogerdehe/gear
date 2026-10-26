//! Tests for `gear_app::mail::send` (T-14).
//!
//! The doc comment of `send` is the spec: slice 1 routing and states, the
//! `--to` / `--reply-to` rules, request replay, and the kinds that belong to
//! slice 2.

use gear_app::identity::Fingerprint;
use gear_app::mail::{self, SendArgs};
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, MsgKind, MsgState, SeatRole};
use gear_store::{testing, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", NOW)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", NOW)?;
            Ok(())
        })
        .unwrap();
    s
}

fn lane_env() -> Env {
    Env {
        ctx: Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 1 }),
        pane_id: Some(LANE_PANE.into()),
        admin: false,
    }
}

fn captain_env() -> Env {
    Env {
        ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
        pane_id: Some(CAP_PANE.into()),
        admin: false,
    }
}

fn lane_fp() -> Fingerprint {
    Fingerprint { pane_id: LANE_PANE.into(), kind: Some("opencode".into()), session_ref: None }
}

fn captain_fp() -> Fingerprint {
    Fingerprint { pane_id: CAP_PANE.into(), kind: Some("codex".into()), session_ref: None }
}

fn args(kind: MsgKind) -> SendArgs {
    SendArgs {
        kind,
        body: "hello".into(),
        task: None,
        attempt: None,
        contract: None,
        candidate: None,
        verdict: None,
        blocking: None,
        base: None,
        about: None,
        to: None,
        reply_to: None,
        urgent: false,
        left: None,
        blocker: None,
        on: None,
        request_id: "r1".into(),
    }
}

#[test]
fn question_goes_to_captain_open() {
    let mut s = world();
    let sent = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &args(MsgKind::Question), NOW).unwrap();
    assert_eq!(sent.msg_id, "M-1");
    assert_eq!(sent.recipient, "p:captain");
    assert!(!sent.replayed);

    let m = s.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap();
    assert_eq!(m.state, MsgState::Open);
    assert_eq!(m.kind, MsgKind::Question);
    assert_eq!(m.sender, "p:heat");
    assert_eq!(m.recipient, "p:captain");
    assert_eq!(m.sender_gen, 1);

    let inbox = s.read(|tx| tx.actionable_for("p", "p:captain", NOW)).unwrap();
    assert_eq!(inbox.len(), 1, "the question is in the captain's inbox");
}

#[test]
fn progress_is_info() {
    let mut s = world();
    let sent = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &args(MsgKind::Progress), NOW).unwrap();
    let m = s.read(|tx| tx.get_message("p", &sent.msg_id)).unwrap().unwrap();
    assert_eq!(m.state, MsgState::Info);
    assert_eq!(m.recipient, "p:captain");
}

#[test]
fn lane_cannot_use_to() {
    let mut s = world();
    let mut a = args(MsgKind::Instruction);
    a.to = Some("heat".into());
    let err = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &a, NOW).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn captain_instructs_lane() {
    let mut s = world();
    let mut a = args(MsgKind::Instruction);
    a.to = Some("heat".into());
    let sent = mail::send(&mut s, &captain_env(), Some(&captain_fp()), &a, NOW).unwrap();
    assert_eq!(sent.recipient, "p:heat");

    let m = s.read(|tx| tx.get_message("p", &sent.msg_id)).unwrap().unwrap();
    assert_eq!(m.state, MsgState::Open);
    assert_eq!(m.kind, MsgKind::Instruction);
    assert_eq!(m.sender, "p:captain");
    assert_eq!(m.recipient, "p:heat");
}

#[test]
fn reply_to_inherits_thread() {
    let mut s = world();
    let question = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &args(MsgKind::Question), NOW).unwrap();

    let mut a = args(MsgKind::Instruction);
    a.reply_to = Some(question.msg_id.clone());
    a.request_id = "r2".into();
    let sent = mail::send(&mut s, &captain_env(), Some(&captain_fp()), &a, NOW + 1).unwrap();

    assert_eq!(sent.recipient, "p:heat", "the answer goes back to the sender");
    assert_eq!(sent.thread, question.thread, "the thread is inherited");

    let m = s.read(|tx| tx.get_message("p", &sent.msg_id)).unwrap().unwrap();
    assert_eq!(m.thread, question.thread);
    assert_eq!(m.reply_to.as_deref(), Some(question.msg_id.as_str()));
}

#[test]
fn to_requires_instruction() {
    let mut s = world();
    // `--to` is captain to lane and always an instruction (design/07:21).
    let mut a = args(MsgKind::Question);
    a.to = Some("heat".into());
    let err = mail::send(&mut s, &captain_env(), Some(&captain_fp()), &a, NOW).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);

    let mut progress = args(MsgKind::Progress);
    progress.to = Some("heat".into());
    let err = mail::send(&mut s, &captain_env(), Some(&captain_fp()), &progress, NOW).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);

    // With an instruction it is stored for that lane.
    let mut instruction = args(MsgKind::Instruction);
    instruction.to = Some("heat".into());
    instruction.request_id = "r2".into();
    let sent = mail::send(&mut s, &captain_env(), Some(&captain_fp()), &instruction, NOW + 1).unwrap();
    assert_eq!(sent.recipient, "p:heat");
    let m = s.read(|tx| tx.get_message("p", &sent.msg_id)).unwrap().unwrap();
    assert_eq!(m.kind, MsgKind::Instruction);
    assert_eq!(m.state, MsgState::Open);
}

#[test]
fn new_thread_starts_with_the_task_or_the_seat() {
    let mut s = world();
    let sent = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &args(MsgKind::Question), NOW).unwrap();
    assert_eq!(sent.thread, "heat:1", "a thread without a task is named after the seat");

    let mut a = args(MsgKind::Question);
    a.request_id = "r2".into();
    let sent = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &a, NOW + 1).unwrap();
    assert_eq!(sent.thread, "heat:2", "the next question gets the next number");

    let mut with_task = args(MsgKind::Question);
    with_task.task = Some("T-1".into());
    with_task.request_id = "r3".into();
    let sent = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &with_task, NOW + 2).unwrap();
    assert_eq!(sent.thread, "T-1:1", "a thread with a task is named after the task");
}

#[test]
fn same_request_is_replayed() {
    let mut s = world();
    let a = args(MsgKind::Question);
    let first = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &a, NOW).unwrap();
    let second = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &a, NOW + 1).unwrap();

    assert_eq!(first.msg_id, second.msg_id);
    assert_eq!(first.thread, second.thread);
    assert!(!first.replayed);
    assert!(second.replayed);

    let inbox = s.read(|tx| tx.actionable_for("p", "p:captain", NOW + 2)).unwrap();
    assert_eq!(inbox.len(), 1, "the replay stored no second message");
    let m2 = s.read(|tx| tx.get_message("p", "M-2")).unwrap();
    assert!(m2.is_none());

    // The replay numbered no thread: the next question is the second one.
    let mut next = args(MsgKind::Question);
    next.request_id = "r2".into();
    let sent = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &next, NOW + 3).unwrap();
    assert_eq!(sent.thread, "heat:2");
}

#[test]
fn request_conflict() {
    let mut s = world();
    mail::send(&mut s, &lane_env(), Some(&lane_fp()), &args(MsgKind::Question), NOW).unwrap();

    let mut other = args(MsgKind::Question);
    other.body = "a different question".into();
    let err = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &other, NOW + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::RequestConflict);
}

#[test]
fn slice2_kinds_rejected() {
    let mut s = world();
    // `result` is accepted since T-45 (and needs its four fields); ack, blocked
    // and reply are still rejected.
    for kind in [MsgKind::Ack, MsgKind::Blocked, MsgKind::Reply] {
        let err = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &args(kind), NOW).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat, "{kind:?} must not be stored yet");
    }
    let result_without_fields = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &args(MsgKind::Result), NOW).unwrap_err();
    assert_eq!(result_without_fields.code, ErrorCode::BadFormat, "a result needs its four fields");
    let inbox = s.read(|tx| tx.actionable_for("p", "p:captain", NOW)).unwrap();
    assert!(inbox.is_empty(), "nothing was stored");
}

#[test]
fn stale_generation_rejected() {
    let mut s = world();
    let mut env = lane_env();
    env.ctx = Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 7 });
    let err = mail::send(&mut s, &env, Some(&lane_fp()), &args(MsgKind::Question), NOW).unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleGeneration);

    let inbox = s.read(|tx| tx.actionable_for("p", "p:captain", NOW)).unwrap();
    assert!(inbox.is_empty(), "nothing was stored");
}
