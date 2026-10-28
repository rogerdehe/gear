//! Messages to a reviewer seat carry its task (design §3.8, T-139).
//!
//! The rules under test:
//! - a reviewer seat is bound to a task for life, so the messages it writes
//!   carry that task, and the captain's answer to them comes back to the
//!   right job;
//! - an instruction the captain addresses to a reviewer without `--task`
//!   takes the seat's `task_id`; an idle reviewer has no job, and the
//!   instruction would show nowhere — it is refused;
//! - a hand-in is answered (design §10.7, row B03-04/10): the lane is told
//!   that its work is in and that it ends its turn;
//! - an item resolved by mistake is reopened (design §10.7, row B03-07).

use gear_app::home;
use gear_app::identity::Fingerprint;
use gear_app::mail::{self, ReplyArgs, ReplyMode, SendArgs};
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const REVIEW_PANE: &str = "w1:p4";

/// The captain, a lane, an idle reviewer, and the task itself. The reviewer
/// seat is bound to a job with [`bind_job`].
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", NOW)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", NOW)?;
        testing::seed_active_seat(tx, "p", "review-1", SeatRole::Temp, REVIEW_PANE, "codex", NOW)?;
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
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-1".into(),
            version: 1,
            goal: "g".into(),
            accept: "a".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: NOW,
            mechanical: None,
        })?;
        Ok(())
    })
    .unwrap();
    s
}

fn seat_env(seat: &str, pane: &str) -> Env {
    Env {
        ctx: Some(GearCtx { project: "p".into(), seat: seat.into(), generation: 1 }),
        pane_id: Some(pane.into()),
        admin: false,
    }
}

/// The reviewer seat takes the job it reviews: it is bound to a task for life.
fn bind_job(s: &mut Store) {
    s.write(|tx| tx.set_seat_task("p:review-1", "T-1")).unwrap();
}

fn captain_env() -> Env {
    seat_env("captain", CAP_PANE)
}

fn reviewer_env() -> Env {
    seat_env("review-1", REVIEW_PANE)
}

fn fp(pane: &str, kind: &str) -> Fingerprint {
    Fingerprint { pane_id: pane.into(), kind: Some(kind.into()), session_ref: None }
}

fn send_args(kind: MsgKind) -> SendArgs {
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

/// The reviewer asks the captain a question, without naming the task.
fn reviewer_asks(s: &mut Store, body: &str, request_id: &str, now: i64) -> String {
    let mut args = send_args(MsgKind::Question);
    args.body = body.into();
    args.request_id = request_id.into();
    mail::send(s, &reviewer_env(), Some(&fp(REVIEW_PANE, "codex")), &args, now)
        .unwrap()
        .msg_id
}

/// The captain answers an item.
fn reply(s: &mut Store, ids: &[&str], mode: ReplyMode, text: &str, request_id: &str, now: i64) -> Vec<String> {
    let args = ReplyArgs {
        ids: ids.iter().map(|id| id.to_string()).collect(),
        text: text.into(),
        mode,
        wait_on: None,
        wait_until: None,
        downgrade: false,
        request_id: request_id.into(),
    };
    mail::reply(s, &captain_env(), Some(&fp(CAP_PANE, "codex")), &args, now)
        .unwrap()
        .replies
}

/// The home page of the reviewer seat.
fn reviewer_home(s: &mut Store, now: i64) -> String {
    home::build(s, &reviewer_env(), Some(&fp(REVIEW_PANE, "codex")), now)
        .unwrap()
        .text
}

#[test]
fn a_reviewers_question_carries_its_task() {
    // §3.8: the reviewer seat is bound to a task for life, so the question
    // it writes is about that task, and the captain sees which one.
    let mut s = world();
    bind_job(&mut s);
    let id = reviewer_asks(&mut s, "the candidate has two crates", "s1", NOW);
    let message = s.read(|tx| tx.get_message("p", &id)).unwrap().unwrap();
    assert_eq!(message.task.as_deref(), Some("T-1"), "the question is about the job");
    assert_eq!(message.thread, "T-1:1", "the thread is the task's: {}", message.thread);
}

#[test]
fn a_reply_to_a_reviewers_question_shows_on_its_home_page() {
    // §3.8: the answer copies the task and the attempt of the message it
    // answers, so it belongs to the reviewer's job and shows on its page.
    let mut s = world();
    bind_job(&mut s);
    let id = reviewer_asks(&mut s, "the candidate has two crates", "s1", NOW);
    let replies = reply(&mut s, &[&id], ReplyMode::Resolve, "keep one crate", "r1", NOW + 1);
    assert_eq!(replies.len(), 1, "the reviewer is answered");

    let answer = s.read(|tx| tx.get_message("p", &replies[0])).unwrap().unwrap();
    assert_eq!(answer.task.as_deref(), Some("T-1"), "the answer carries the task");
    assert_eq!(answer.recipient, "p:review-1");

    let page = reviewer_home(&mut s, NOW + 2);
    assert!(page.contains("keep one crate"), "the answer is on the page: {page}");
    assert!(page.contains("UNSHOWN"), "the answer is unshown: {page}");
}

#[test]
fn an_instruction_without_task_to_a_reviewer_in_a_job_carries_it() {
    // §3.8: the captain addresses a reviewer without `--task`: the message
    // takes the seat's `task_id`, so it shows on the reviewer's home page.
    let mut s = world();
    bind_job(&mut s);
    let mut args = send_args(MsgKind::Instruction);
    args.to = Some("review-1".into());
    args.body = "look at the second crate".into();
    args.request_id = "s1".into();
    let sent = mail::send(&mut s, &captain_env(), Some(&fp(CAP_PANE, "codex")), &args, NOW).unwrap();

    assert_eq!(sent.recipient, "p:review-1");
    let message = s.read(|tx| tx.get_message("p", &sent.msg_id)).unwrap().unwrap();
    assert_eq!(message.task.as_deref(), Some("T-1"), "the instruction is about the job");

    let page = reviewer_home(&mut s, NOW + 1);
    assert!(page.contains("look at the second crate"), "the instruction is on the page: {page}");
}

#[test]
fn an_instruction_to_an_idle_reviewer_is_refused() {
    // §3.8: an idle reviewer has no job and no task, so the instruction would
    // show nowhere. It is refused, with the reason.
    let mut s = world();
    // The reviewer seat has no job: it is idle.
    let mut args = send_args(MsgKind::Instruction);
    args.to = Some("review-1".into());
    args.request_id = "s1".into();
    let error = mail::send(&mut s, &captain_env(), Some(&fp(CAP_PANE, "codex")), &args, NOW).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error.detail.contains("has no review job; nothing would show"),
        "{}",
        error.detail
    );
}

#[test]
fn a_resolved_item_is_reopened() {
    // §10.7 row B03-07: an item resolved by mistake goes back to the
    // captain's inbox, and nobody is answered on the way.
    let mut s = world();
    bind_job(&mut s);
    let id = reviewer_asks(&mut s, "the candidate has two crates", "s1", NOW);
    reply(&mut s, &[&id], ReplyMode::Resolve, "keep one crate", "r1", NOW + 1);
    assert_eq!(
        s.read(|tx| tx.get_message("p", &id)).unwrap().unwrap().state,
        MsgState::Resolved
    );

    let args = ReplyArgs {
        ids: vec![id.clone()],
        text: "resolved by mistake".into(),
        mode: ReplyMode::Reopen,
        wait_on: None,
        wait_until: None,
        downgrade: false,
        request_id: "r2".into(),
    };
    let result = mail::reply(&mut s, &captain_env(), Some(&fp(CAP_PANE, "codex")), &args, NOW + 2).unwrap();
    assert_eq!(result.handled, vec![(id.clone(), "open".to_string())]);
    assert!(result.replies.is_empty(), "a reopen answers nobody: {result:?}");
    assert_eq!(
        s.read(|tx| tx.get_message("p", &id)).unwrap().unwrap().state,
        MsgState::Open
    );
    // The item is back in the captain's inbox.
    let inbox = s.read(|tx| tx.actionable_for("p", "p:captain", NOW + 3)).unwrap();
    assert_eq!(inbox.len(), 1, "the item is actionable again: {inbox:?}");
}

#[test]
fn a_reopen_of_an_open_item_is_refused() {
    // Only a resolved item is reopened.
    let mut s = world();
    let id = reviewer_asks(&mut s, "the candidate has two crates", "s1", NOW);
    let args = ReplyArgs {
        ids: vec![id.clone()],
        text: "resolved by mistake".into(),
        mode: ReplyMode::Reopen,
        wait_on: None,
        wait_until: None,
        downgrade: false,
        request_id: "r1".into(),
    };
    let error = mail::reply(&mut s, &captain_env(), Some(&fp(CAP_PANE, "codex")), &args, NOW + 1).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadTransition, "{}", error.detail);
    assert_eq!(
        s.read(|tx| tx.get_message("p", &id)).unwrap().unwrap().state,
        MsgState::Open
    );
}
