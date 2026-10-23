//! Tests for `gear_app::mail::reply` (T-15).
//!
//! The doc comment of `reply` is the spec: captain only, it closes exactly
//! the listed ids, and it writes one Reply (info) per original sender in the
//! same thread.

use gear_app::identity::Fingerprint;
use gear_app::mail::{self, ReplyArgs, ReplyMode, SendArgs};
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, MsgKind, MsgState, SeatRole};
use gear_store::{testing, NewMessage, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const OTHER_PANE: &str = "w1:p3";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", NOW)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", NOW)?;
        testing::seed_active_seat(tx, "p", "pricing", SeatRole::Lane, OTHER_PANE, "opencode", NOW)?;
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

fn captain_env() -> Env {
    seat_env("captain", CAP_PANE)
}

fn fp(pane: &str, kind: &str) -> Fingerprint {
    Fingerprint { pane_id: pane.into(), kind: Some(kind.into()), session_ref: None }
}

/// A lane asks the captain a question.
fn ask(s: &mut Store, seat: &str, pane: &str, body: &str, request_id: &str, now: i64) -> String {
    let args = SendArgs {
        kind: MsgKind::Question,
        body: body.into(),
        task: None,
        attempt: None,
        contract: None,
        candidate: None,
        verdict: None,
        about: None,
        to: None,
        reply_to: None,
        urgent: false,
        request_id: request_id.into(),
    };
    mail::send(s, &seat_env(seat, pane), Some(&fp(pane, "opencode")), &args, now).unwrap().msg_id
}

/// The captain instructs a lane.
fn instruct(s: &mut Store, seat: &str, request_id: &str, now: i64) -> String {
    let args = SendArgs {
        kind: MsgKind::Instruction,
        body: "do it".into(),
        task: None,
        attempt: None,
        contract: None,
        candidate: None,
        verdict: None,
        about: None,
        to: Some(seat.into()),
        reply_to: None,
        urgent: false,
        request_id: request_id.into(),
    };
    mail::send(s, &captain_env(), Some(&fp(CAP_PANE, "codex")), &args, now).unwrap().msg_id
}

fn reply_args(ids: &[&str], mode: ReplyMode, request_id: &str) -> ReplyArgs {
    ReplyArgs {
        ids: ids.iter().map(|id| id.to_string()).collect(),
        text: "the answer".into(),
        mode,
        wait_on: None,
        wait_until: None,
        downgrade: false,
        request_id: request_id.into(),
    }
}

fn state_of(s: &mut Store, id: &str) -> MsgState {
    s.read(|tx| tx.get_message("p", id)).unwrap().unwrap().state
}

/// A question written straight into a given thread.
fn insert_question(
    s: &mut Store,
    thread: &str,
    sender: &str,
    recipient: &str,
    body: &str,
    created: i64,
) -> String {
    s.write(|tx| {
        let row = NewMessage {
            project: "p".into(),
            thread: thread.into(),
            task: None,
            attempt: None,
            sender: sender.into(),
            sender_gen: 1,
            recipient: recipient.into(),
            kind: MsgKind::Question,
            urgent: false,
            body: body.into(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created,
        };
        Ok(tx.insert_message(&row)?.id)
    })
    .unwrap()
}

#[test]
fn resolve_closes_and_replies() {
    let mut s = world();
    let id = ask(&mut s, "heat", LANE_PANE, "which schema?", "s1", NOW);

    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id], ReplyMode::Resolve, "r1"),
        NOW + 1,
    )
    .unwrap();

    assert_eq!(r.handled, vec![(id.clone(), "resolved".to_string())]);
    assert_eq!(r.replies.len(), 1);
    assert!(!r.replayed);
    assert_eq!(state_of(&mut s, &id), MsgState::Resolved);

    let answer = s.read(|tx| tx.get_message("p", &r.replies[0])).unwrap().unwrap();
    assert_eq!(answer.recipient, "p:heat");
    assert_eq!(answer.sender, "p:captain");
    assert_eq!(answer.kind, MsgKind::Reply);
    assert_eq!(answer.state, MsgState::Info);
    assert_eq!(answer.thread, s.read(|tx| tx.get_message("p", &id)).unwrap().unwrap().thread);
    assert_eq!(answer.reply_to.as_deref(), Some(id.as_str()));
}

#[test]
fn wait_sets_waiting() {
    let mut s = world();
    let id = ask(&mut s, "heat", LANE_PANE, "which schema?", "s1", NOW);

    let mut args = reply_args(&[&id], ReplyMode::Wait, "r1");
    args.wait_on = Some("user".into());
    args.wait_until = Some(NOW + 100);
    let r = mail::reply(&mut s, &captain_env(), Some(&fp(CAP_PANE, "codex")), &args, NOW + 1).unwrap();

    assert_eq!(r.handled, vec![(id.clone(), "waiting".to_string())]);
    let m = s.read(|tx| tx.get_message("p", &id)).unwrap().unwrap();
    assert_eq!(m.state, MsgState::Waiting);
    assert_eq!(m.wait_on.as_deref(), Some("user"));
    assert_eq!(m.wait_until, Some(NOW + 100));
    assert_eq!(r.replies.len(), 1);
}

#[test]
fn note_keeps_state() {
    let mut s = world();
    let id = ask(&mut s, "heat", LANE_PANE, "which schema?", "s1", NOW);

    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id], ReplyMode::Note, "r1"),
        NOW + 1,
    )
    .unwrap();

    assert_eq!(r.handled, vec![(id.clone(), "open".to_string())]);
    assert_eq!(state_of(&mut s, &id), MsgState::Open, "a note changes no state");
    assert_eq!(r.replies.len(), 1, "the note is still an answer");
}

#[test]
fn one_reply_per_sender_and_thread() {
    let mut s = world();
    let a = ask(&mut s, "heat", LANE_PANE, "q1", "s1", NOW);
    let b = ask(&mut s, "heat", LANE_PANE, "q2", "s2", NOW + 1);
    let c = ask(&mut s, "pricing", OTHER_PANE, "q3", "s3", NOW + 2);

    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&a, &b, &c], ReplyMode::Resolve, "r1"),
        NOW + 3,
    )
    .unwrap();

    assert_eq!(r.handled.len(), 3, "every listed id is handled");
    assert_eq!(r.replies.len(), 3, "one answer per thread of a sender");

    let threads: Vec<String> = r
        .replies
        .iter()
        .map(|id| s.read(|tx| tx.get_message("p", id)).unwrap().unwrap().thread)
        .collect();
    assert_eq!(threads, vec!["heat:1".to_string(), "heat:2".to_string(), "pricing:1".to_string()]);
}

#[test]
fn answer_reaches_every_thread_of_a_sender() {
    let mut s = world();
    let a = ask(&mut s, "heat", LANE_PANE, "q1", "s1", NOW);
    let b = ask(&mut s, "heat", LANE_PANE, "q2", "s2", NOW + 1);
    // A second question of the same lane in the same thread.
    let c = insert_question(&mut s, "heat:1", "p:heat", "p:captain", "q3", NOW + 2);

    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&a, &b, &c], ReplyMode::Resolve, "r1"),
        NOW + 3,
    )
    .unwrap();

    assert_eq!(r.handled.len(), 3, "every listed id is handled");
    // heat:1 (twice) and heat:2 — the thread of the lane appears twice, so
    // two answers, and the second id of heat:1 does not add a third.
    assert_eq!(r.replies.len(), 2, "one answer per thread of a sender");

    let threads: Vec<String> = r
        .replies
        .iter()
        .map(|id| s.read(|tx| tx.get_message("p", id)).unwrap().unwrap().thread)
        .collect();
    assert_eq!(threads, vec!["heat:1".to_string(), "heat:2".to_string()]);
}

#[test]
fn repeated_ids_are_handled_once() {
    let mut s = world();
    let id = ask(&mut s, "heat", LANE_PANE, "which schema?", "s1", NOW);

    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id, &id], ReplyMode::Resolve, "r1"),
        NOW + 1,
    )
    .unwrap();

    assert_eq!(r.handled, vec![(id.clone(), "resolved".to_string())], "the id is listed once");
    assert_eq!(r.replies.len(), 1, "one answer");
    assert_eq!(state_of(&mut s, &id), MsgState::Resolved);
}

#[test]
fn wait_without_time_is_bad_format() {
    let mut s = world();
    let id = ask(&mut s, "heat", LANE_PANE, "which schema?", "s1", NOW);

    // A wait without a recheck time would never become due again.
    let err = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id], ReplyMode::Wait, "r1"),
        NOW + 1,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert_eq!(state_of(&mut s, &id), MsgState::Open, "nothing was written");

    // A label without a time is not enough either.
    let mut args = reply_args(&[&id], ReplyMode::Wait, "r2");
    args.wait_on = Some("user".into());
    let err = mail::reply(&mut s, &captain_env(), Some(&fp(CAP_PANE, "codex")), &args, NOW + 2).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert_eq!(state_of(&mut s, &id), MsgState::Open, "nothing was written");
}

#[test]
fn lane_cannot_reply() {
    let mut s = world();
    let id = ask(&mut s, "heat", LANE_PANE, "which schema?", "s1", NOW);

    let err = mail::reply(
        &mut s,
        &seat_env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        &reply_args(&[&id], ReplyMode::Resolve, "r1"),
        NOW + 1,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(state_of(&mut s, &id), MsgState::Open, "nothing was written");
}

#[test]
fn foreign_or_closed_id_rejected_atomically() {
    let mut s = world();
    let open = ask(&mut s, "heat", LANE_PANE, "q1", "s1", NOW);
    let foreign = instruct(&mut s, "heat", "s2", NOW + 1);
    let closed = ask(&mut s, "pricing", OTHER_PANE, "q2", "s3", NOW + 2);
    mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&closed], ReplyMode::Resolve, "r1"),
        NOW + 3,
    )
    .unwrap();

    // A message of another seat's inbox: nothing is written, not even for
    // the valid id in the same call.
    let err = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&open, &foreign], ReplyMode::Resolve, "r2"),
        NOW + 4,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(state_of(&mut s, &open), MsgState::Open, "the first id is untouched");

    // An unknown id.
    let err = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&["M-999"], ReplyMode::Resolve, "r3"),
        NOW + 5,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::NoTarget);

    // An id that is already closed.
    let err = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&closed], ReplyMode::Resolve, "r4"),
        NOW + 6,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadTransition);
}

#[test]
fn only_listed_ids_change() {
    let mut s = world();
    let listed = ask(&mut s, "heat", LANE_PANE, "q1", "s1", NOW);
    let other = ask(&mut s, "heat", LANE_PANE, "q2", "s2", NOW + 1);

    mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&listed], ReplyMode::Resolve, "r1"),
        NOW + 2,
    )
    .unwrap();

    assert_eq!(state_of(&mut s, &listed), MsgState::Resolved);
    assert_eq!(state_of(&mut s, &other), MsgState::Open, "unlisted messages stay open");
}

#[test]
fn reply_is_replayed() {
    let mut s = world();
    let id = ask(&mut s, "heat", LANE_PANE, "which schema?", "s1", NOW);

    let first = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id], ReplyMode::Resolve, "r1"),
        NOW + 1,
    )
    .unwrap();
    let second = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id], ReplyMode::Resolve, "r1"),
        NOW + 2,
    )
    .unwrap();

    assert_eq!(first.handled, second.handled);
    assert_eq!(first.replies, second.replies);
    assert!(!first.replayed);
    assert!(second.replayed);

    // The replay wrote no second answer.
    let inbox = s.read(|tx| tx.messages_to_since("p", "p:heat", 0)).unwrap();
    assert_eq!(inbox.len(), 1, "one answer in the lane inbox");
}
