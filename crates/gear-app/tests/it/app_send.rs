//! Tests for `gear_app::mail::send` (T-14).
//!
//! The doc comment of `send` is the spec: slice 1 routing and states, the
//! `--to` / `--reply-to` rules, request replay, and the kinds that belong to
//! slice 2.

use gear_app::identity::Fingerprint;
use gear_app::mail::{self, SendArgs};
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const OTHER_PANE: &str = "w1:p3";

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

/// A second lane, to prove an ack is only the addressed seat's.
fn other_lane(s: &mut Store) {
    s.write(|tx| {
        testing::seed_active_seat(tx, "p", "cold", SeatRole::Lane, OTHER_PANE, "opencode", NOW)?;
        Ok(())
    })
    .unwrap();
}

fn cold_env() -> Env {
    Env {
        ctx: Some(GearCtx { project: "p".into(), seat: "cold".into(), generation: 1 }),
        pane_id: Some(OTHER_PANE.into()),
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
        friction: None,
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
    // `result` is accepted since T-45 (and needs its four fields), `ack` since
    // T-320 (and needs the instruction it answers); `blocked` and `reply` are
    // still rejected.
    for kind in [MsgKind::Blocked, MsgKind::Reply] {
        let err = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &args(kind), NOW).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat, "{kind:?} must not be stored yet");
    }
    let result_without_fields = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &args(MsgKind::Result), NOW).unwrap_err();
    assert_eq!(result_without_fields.code, ErrorCode::BadFormat, "a result needs its four fields");
    let inbox = s.read(|tx| tx.actionable_for("p", "p:captain", NOW)).unwrap();
    assert!(inbox.is_empty(), "nothing was stored");
}

#[test]
fn stale_generation_rejected() {    let mut s = world();
    let mut env = lane_env();
    env.ctx = Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 7 });
    let err = mail::send(&mut s, &env, Some(&lane_fp()), &args(MsgKind::Question), NOW).unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleGeneration);

    let inbox = s.read(|tx| tx.actionable_for("p", "p:captain", NOW)).unwrap();
    assert!(inbox.is_empty(), "nothing was stored");
}

/// The captain's instruction that asks for an ack (T-320).
fn ack_instruction(s: &mut Store, request: &str) -> String {
    let mut a = args(MsgKind::Instruction);
    a.to = Some("heat".into());
    a.body = "bring the board up to date before the station".into();
    a.request_id = request.into();
    let sent = mail::send_with_ack(s, &captain_env(), Some(&captain_fp()), &a, NOW).unwrap();
    sent.msg_id
}

#[test]
fn ack_instruction_waits_for_the_lane() {
    let mut s = world();
    let id = ack_instruction(&mut s, "r1");

    let instruction = s.read(|tx| tx.get_message("p", &id)).unwrap().unwrap();
    assert!(instruction.needs_ack, "the instruction asks for an ack");
    assert_eq!(instruction.kind, MsgKind::Instruction);
    assert_eq!(instruction.state, MsgState::Open);
    assert!(
        instruction
            .body
            .ends_with(&format!(
                "Acknowledge it: gear send --kind ack --reply-to {id} \"<what you will do>\""
            )),
        "the lane is told how to acknowledge it: {}",
        instruction.body
    );

    let item = s.read(|tx| tx.get_message("p", "M-2")).unwrap().unwrap();
    assert_eq!(item.thread, format!("ack:{id}"));
    assert_eq!(item.kind, MsgKind::System);
    assert_eq!(item.sender, "p:captain");
    assert_eq!(item.recipient, "p:captain");
    assert_eq!(item.state, MsgState::Waiting);
    assert_eq!(item.wait_until, Some(NOW + 600), "back after ten minutes");
    assert_eq!(
        item.body,
        format!("heat has not acknowledged {id} yet: bring the board up to date before the station")
    );

    // Not in the captain's batch while it waits, in it when the wait is due.
    let waiting = s
        .read(|tx| mail::select_batch(tx, "p", "p:captain", NOW + 599, mail::BatchLimits::default()))
        .unwrap();
    assert!(
        waiting.iter().all(|view| view.thread != format!("ack:{id}")),
        "a waiting item is not due yet"
    );
    let due = s
        .read(|tx| mail::select_batch(tx, "p", "p:captain", NOW + 600, mail::BatchLimits::default()))
        .unwrap();
    assert!(
        due.iter().any(|view| view.thread == format!("ack:{id}")),
        "the unacknowledged instruction comes back after ten minutes"
    );
}

#[test]
fn the_lane_acknowledges_and_the_item_settles() {
    let mut s = world();
    let id = ack_instruction(&mut s, "r1");

    let mut a = args(MsgKind::Ack);
    a.reply_to = Some(id.clone());
    a.body = "I will update the board now".into();
    a.request_id = "r2".into();
    let sent = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &a, NOW + 1).unwrap();
    assert_eq!(sent.recipient, "p:captain");

    let ack = s.read(|tx| tx.get_message("p", &sent.msg_id)).unwrap().unwrap();
    assert_eq!(ack.kind, MsgKind::Ack);
    assert_eq!(ack.state, MsgState::Info, "an ack is information for the captain");
    assert_eq!(ack.sender, "p:heat");
    assert_eq!(ack.recipient, "p:captain");
    assert_eq!(ack.reply_to.as_deref(), Some(id.as_str()));

    let item = s.read(|tx| tx.get_message("p", "M-2")).unwrap().unwrap();
    assert_eq!(item.state, MsgState::Resolved, "the reminder is settled");
    assert_eq!(
        item.result,
        Some(format!("acknowledged in {}", sent.msg_id))
    );
}

#[test]
fn only_the_addressed_lane_acknowledges_an_instruction_that_asks() {
    let mut s = world();
    other_lane(&mut s);
    let id = ack_instruction(&mut s, "r1");
    let cold_fp = Fingerprint { pane_id: OTHER_PANE.into(), kind: Some("opencode".into()), session_ref: None };

    // Another lane may not answer it.
    let mut a = args(MsgKind::Ack);
    a.reply_to = Some(id.clone());
    a.request_id = "r2".into();
    let err = mail::send(&mut s, &cold_env(), Some(&cold_fp), &a, NOW + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("an ack answers an instruction that asks for one"),
        "{}",
        err.detail
    );

    // An ack needs the instruction it answers.
    let mut bare = args(MsgKind::Ack);
    bare.request_id = "r3".into();
    let err = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &bare, NOW + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);

    // A plain instruction asks for no ack.
    let mut plain = args(MsgKind::Instruction);
    plain.to = Some("heat".into());
    plain.request_id = "r4".into();
    let plain = mail::send(&mut s, &captain_env(), Some(&captain_fp()), &plain, NOW + 1).unwrap();
    let mut a = args(MsgKind::Ack);
    a.reply_to = Some(plain.msg_id.clone());
    a.request_id = "r5".into();
    let err = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &a, NOW + 2).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);

    // `--ack` is the captain's, and only on an instruction with `--to`.
    let mut question = args(MsgKind::Question);
    question.request_id = "r6".into();
    let err = mail::send_with_ack(&mut s, &captain_env(), Some(&captain_fp()), &question, NOW + 3).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert_eq!(err.detail, "`--ack` belongs to a captain's instruction with `--to`");
}

#[test]
fn a_replayed_ack_returns_the_stored_receipt() {
    let mut s = world();
    let id = ack_instruction(&mut s, "r1");
    let mut a = args(MsgKind::Ack);
    a.reply_to = Some(id.clone());
    a.body = "done".into();
    a.request_id = "r2".into();
    let first = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &a, NOW + 1).unwrap();
    let second = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &a, NOW + 2).unwrap();
    assert_eq!(first.msg_id, second.msg_id);
    assert!(!first.replayed);
    assert!(second.replayed);
    assert!(
        s.read(|tx| tx.get_message("p", "M-4")).unwrap().is_none(),
        "no second ack was stored"
    );
}

// T-440: a lane that waits on something outside GEAR (a CI run, evidence the
// captain must fix) declares it with `--on external` and the reason as the
// message body; the wait lasts at most 120 minutes.

const WAIT_TASK: &str = "T-7";

/// A world where the lane works on T-7: a declared wait needs an attempt that
/// has started, and T-8 is there for the wait on another task.
fn wait_world() -> Store {
    let mut s = world();
    s.write(|tx| {
        for (id, rank) in [("T-7", 1), ("T-8", 2)] {
            tx.insert_task(&TaskRow { repo: None, base: None, gate: None, rework_estimate_min: None, project: "p".into(), id: id.into(), rank,
                state: TaskState::Ready, priority: "must".into(), lane: Some("heat".into()), milestone: None, required: false,
                contract_version: 1, deps: vec![], followup_of: None, wait_on: None, wait_cond: None, rework_note: None,
                estimate_min: None, issue: None, updated: NOW })?;
            tx.insert_contract(&ContractRow { project: "p".into(), task: id.into(), version: 1, goal: "g".into(), accept: "a".into(),
                evidence_files: vec![], evidence_due_min: None, body: String::new(), created: NOW, mechanical: None })?;
        }
        gear_app::tasks::start_attempt(tx, "p", WAIT_TASK, "p:heat", 1, NOW)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", WAIT_TASK, 1, NOW)?;
        Ok(())
    })
    .unwrap();
    s
}

/// A blocked message that declares a wait for `on`.
fn blocked(on: &str, body: &str) -> SendArgs {
    let mut a = args(MsgKind::Blocked);
    a.task = Some(WAIT_TASK.into());
    a.attempt = Some(1);
    a.contract = Some(1);
    a.on = Some(on.into());
    a.body = body.into();
    a
}

#[test]
fn an_external_wait_is_stored_with_its_reason_and_capped_at_two_hours() {
    let mut s = wait_world();
    mail::send(&mut s, &lane_env(), Some(&lane_fp()), &blocked("external", "the CI run on heat/t1"), NOW + 30).unwrap();

    let task = s.read(|tx| tx.get_task("p", WAIT_TASK)).unwrap().unwrap();
    assert_eq!((task.wait_on.as_deref(), task.wait_cond.as_deref()), (Some("external"), Some("external")));

    let pause = s.read(|tx| tx.pauses_of_project("p")).unwrap().into_iter().find(|p| p.kind == "wait").unwrap();
    assert_eq!((pause.seat.as_str(), pause.end_at), ("p:heat", None), "the pause is open");
    assert_eq!(pause.reason.as_deref(), Some("waiting for: the CI run on heat/t1"));

    let until = s.read(|tx| tx.get_meta(&gear_app::mail::external_wait_key("p", WAIT_TASK))).unwrap();
    assert_eq!(until.as_deref(), Some((NOW + 30 + 120 * 60).to_string().as_str()), "the wait ends in 120 min");

    let item = s.read(|tx| tx.thread_messages("p", "wait-external:T-7:1")).unwrap();
    assert_eq!(item.len(), 1, "one info item");
    assert_eq!(item[0].recipient, "p:captain");
    assert_eq!(item[0].state, MsgState::Info);
    assert_eq!(item[0].body, "heat waits on T-7 for: the CI run on heat/t1 (at most 120 min)");
}

#[test]
fn an_external_wait_needs_its_reason() {
    let mut s = wait_world();
    for body in ["", "   "] {
        let err = mail::send(&mut s, &lane_env(), Some(&lane_fp()), &blocked("external", body), NOW + 30).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat);
        assert!(err.detail.contains("external"), "the refusal names what is missing: {}", err.detail);
    }
    let task = s.read(|tx| tx.get_task("p", WAIT_TASK)).unwrap().unwrap();
    assert!(task.wait_on.is_none(), "nothing was kept");
    assert!(s.read(|tx| tx.get_meta(&gear_app::mail::external_wait_key("p", WAIT_TASK))).unwrap().is_none());
}

#[test]
fn a_wait_on_a_task_is_unchanged() {
    let mut s = wait_world();
    mail::send(&mut s, &lane_env(), Some(&lane_fp()), &blocked("T-8", "waiting for the API"), NOW + 30).unwrap();
    let task = s.read(|tx| tx.get_task("p", WAIT_TASK)).unwrap().unwrap();
    assert_eq!((task.wait_on.as_deref(), task.wait_cond.as_deref()), (Some("T-8"), Some("merged")));
    assert!(s.read(|tx| tx.get_meta(&gear_app::mail::external_wait_key("p", WAIT_TASK))).unwrap().is_none(), "no external clock");
    assert!(s.read(|tx| tx.thread_messages("p", "wait-external:T-7:1")).unwrap().is_empty(), "no external item");
}
