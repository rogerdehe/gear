//! Tests for `gear_app::mail::reply` (T-15).
//!
//! The doc comment of `reply` is the spec: captain only, it closes exactly
//! the listed ids, and it writes one Reply (info) per original sender in the
//! same thread.

use gear_app::identity::Fingerprint;
use gear_app::mail::{self, ReplyArgs, ReplyMode, SendArgs};
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, MsgKind, MsgState, SeatRole, SeatState};
use gear_store::{testing, NewMessage, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const OTHER_PANE: &str = "w1:p3";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(
            tx,
            "p",
            "captain",
            SeatRole::Captain,
            CAP_PANE,
            "codex",
            NOW,
        )?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", NOW)?;
        testing::seed_active_seat(
            tx,
            "p",
            "pricing",
            SeatRole::Lane,
            OTHER_PANE,
            "opencode",
            NOW,
        )?;
        Ok(())
    })
    .unwrap();
    s
}

fn seat_env(seat: &str, pane: &str) -> Env {
    Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: seat.into(),
            generation: 1,
        }),
        pane_id: Some(pane.into()),
        admin: false,
    }
}

fn captain_env() -> Env {
    seat_env("captain", CAP_PANE)
}

fn fp(pane: &str, kind: &str) -> Fingerprint {
    Fingerprint {
        pane_id: pane.into(),
        kind: Some(kind.into()),
        session_ref: None,
    }
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
        request_id: request_id.into(),
    };
    mail::send(
        s,
        &seat_env(seat, pane),
        Some(&fp(pane, "opencode")),
        &args,
        now,
    )
    .unwrap()
    .msg_id
}

/// The captain raises an item in its own inbox (no `--to`): the captain is
/// the default recipient of a send.
fn raise_own(s: &mut Store, body: &str, request_id: &str, now: i64) -> String {
    let args = SendArgs {
        kind: MsgKind::Question,
        body: body.into(),
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
        request_id: request_id.into(),
    };
    mail::send(s, &captain_env(), Some(&fp(CAP_PANE, "codex")), &args, now)
        .unwrap()
        .msg_id
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
        blocking: None,
        base: None,
        about: None,
        to: Some(seat.into()),
        reply_to: None,
        urgent: false,
        left: None,
        blocker: None,
        on: None,
        friction: None,
        request_id: request_id.into(),
    };
    mail::send(s, &captain_env(), Some(&fp(CAP_PANE, "codex")), &args, now)
        .unwrap()
        .msg_id
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
            meta: None,
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

    let answer = s
        .read(|tx| tx.get_message("p", &r.replies[0]))
        .unwrap()
        .unwrap();
    assert_eq!(answer.recipient, "p:heat");
    assert_eq!(answer.sender, "p:captain");
    assert_eq!(answer.kind, MsgKind::Reply);
    assert_eq!(answer.state, MsgState::Info);
    assert_eq!(
        answer.thread,
        s.read(|tx| tx.get_message("p", &id))
            .unwrap()
            .unwrap()
            .thread
    );
    assert_eq!(answer.reply_to.as_deref(), Some(id.as_str()));
}

#[test]
fn wait_sets_waiting() {
    let mut s = world();
    let id = ask(&mut s, "heat", LANE_PANE, "which schema?", "s1", NOW);

    let mut args = reply_args(&[&id], ReplyMode::Wait, "r1");
    args.wait_on = Some("user".into());
    args.wait_until = Some(NOW + 100);
    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &args,
        NOW + 1,
    )
    .unwrap();

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
    assert_eq!(
        state_of(&mut s, &id),
        MsgState::Open,
        "a note changes no state"
    );
    // T-103: a note is the captain's record on the item, never a message.
    assert!(r.replies.is_empty(), "a note is sent to nobody");
    let item = s.read(|tx| tx.get_message("p", &id)).unwrap().unwrap();
    assert_eq!(
        item.result.as_deref(),
        Some("the answer"),
        "kept on the item"
    );
    let messages = s.read(|tx| tx.thread_messages("p", &item.thread)).unwrap();
    assert_eq!(messages.len(), 1, "no message was written: {messages:?}");
}

#[test]
fn a_note_reaches_no_seat() {
    // T-103: whatever the mode, a note is not typed into anyone's inbox.
    let mut s = world();
    let id = ask(&mut s, "heat", LANE_PANE, "which schema?", "s1", NOW);
    let before = s
        .read(|tx| tx.thread_messages("p", "T-0:1"))
        .map(|m| m.len())
        .unwrap_or(0);

    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id], ReplyMode::Note, "r1"),
        NOW + 1,
    )
    .unwrap();

    assert!(r.replies.is_empty());
    let after = s
        .read(|tx| tx.thread_messages("p", "T-0:1"))
        .map(|m| m.len())
        .unwrap_or(0);
    assert_eq!(before, after, "no message anywhere");
    // The lane's own batch shows nothing new from the captain.
    let views = s
        .read(|tx| mail::select_batch(tx, "p", "p:heat", NOW + 2, mail::BatchLimits::default()))
        .unwrap();
    assert!(
        views.iter().all(|view| view.last_reply.is_none()),
        "the note is not a reply: {views:?}"
    );
}

#[test]
fn a_resolve_to_a_released_reviewer_sends_nothing() {
    // T-103: a released reviewer is gone; its item is resolved, and no answer
    // is written into a seat that will never read it.
    let mut s = world();
    s.write(|tx| {
        testing::seed_active_seat(
            tx,
            "p",
            "review-1",
            SeatRole::Temp,
            "w1:p4",
            "opencode",
            NOW,
        )
    })
    .unwrap();
    let id = ask(
        &mut s,
        "review-1",
        "w1:p4",
        "the checkout is held",
        "s1",
        NOW,
    );
    s.write(|tx| {
        tx.set_seat_state(
            "p:review-1",
            SeatState::Released,
            Some("review done"),
            NOW + 1,
        )
    })
    .unwrap();

    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id], ReplyMode::Resolve, "r1"),
        NOW + 2,
    )
    .unwrap();

    assert_eq!(r.handled, vec![(id.clone(), "resolved".to_string())]);
    assert!(r.replies.is_empty(), "a released reviewer is not answered");
    assert_eq!(state_of(&mut s, &id), MsgState::Resolved);
}

#[test]
fn a_retiring_lane_still_gets_its_answer() {
    // F1 of the T-103 review: a retiring lane finishes the attempt it holds,
    // so the captain's answer to its question must reach it.
    let mut s = world();
    let id = ask(&mut s, "heat", LANE_PANE, "which schema?", "s1", NOW);
    s.write(|tx| tx.set_seat_state("p:heat", SeatState::Retiring, Some("retiring"), NOW + 1))
        .unwrap();

    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id], ReplyMode::Resolve, "r1"),
        NOW + 2,
    )
    .unwrap();

    assert_eq!(r.handled, vec![(id.clone(), "resolved".to_string())]);
    assert_eq!(r.replies.len(), 1, "a retiring lane is still live");
    let answer = s
        .read(|tx| tx.get_message("p", &r.replies[0]))
        .unwrap()
        .unwrap();
    assert_eq!(answer.recipient, "p:heat");
    assert_eq!(answer.body, "the answer");
}

#[test]
fn a_reused_reviewer_gets_no_answer_to_an_earlier_jobs_item() {
    // T-103: the same seat reused for a later job (generation 2) must not read
    // the captain's note on the generation-1 item.
    let mut s = world();
    s.write(|tx| {
        testing::seed_active_seat(
            tx,
            "p",
            "review-1",
            SeatRole::Temp,
            "w1:p4",
            "opencode",
            NOW,
        )
    })
    .unwrap();
    let id = ask(
        &mut s,
        "review-1",
        "w1:p4",
        "the checkout is held",
        "s1",
        NOW,
    );
    s.write(|tx| {
        tx.set_seat_state(
            "p:review-1",
            SeatState::Released,
            Some("review done"),
            NOW + 1,
        )?;
        gear_app::seats::begin_onboard(tx, "p:review-1", "w1:p4", "opencode", NOW + 2)?;
        Ok(())
    })
    .unwrap();
    let generation = s
        .read(|tx| tx.get_seat("p:review-1"))
        .unwrap()
        .unwrap()
        .generation;
    assert_eq!(generation, 2, "the seat was reused");

    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id], ReplyMode::Resolve, "r1"),
        NOW + 3,
    )
    .unwrap();

    assert!(
        r.replies.is_empty(),
        "an earlier job's note stays with the job"
    );
    assert_eq!(state_of(&mut s, &id), MsgState::Resolved);
}

#[test]
fn a_resolve_of_the_engines_item_sends_nothing() {
    // T-103 (the trial): the captain's item about a re-queued review seat was
    // written by the engine; resolving it with an instruction must not send
    // that instruction to the engine.
    let mut s = world();
    let id = s
        .write(|tx| {
            Ok(tx
                .insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "review-seat:1".into(),
                    task: None,
                    attempt: None,
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: "p:captain".into(),
                    kind: MsgKind::System,
                    urgent: false,
                    body: "the review seat was re-queued".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: NOW,
                })?
                .id)
        })
        .unwrap();

    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id], ReplyMode::Resolve, "r1"),
        NOW + 1,
    )
    .unwrap();

    assert_eq!(r.handled, vec![(id.clone(), "resolved".to_string())]);
    assert!(r.replies.is_empty(), "the engine has no inbox to read");
}

#[test]
fn answering_its_own_item_sends_the_captain_nothing() {
    // T-119 (trial B02-03): the captain can raise an item in its own inbox (a
    // question it answers itself, a self-sent wait). Answering it settles the
    // item; a reply addressed to the captain is the same seat and no new fact,
    // and it would ring the captain's own doorbell for nothing.
    let mut s = world();
    let resolved = raise_own(&mut s, "the release waits on ci", "s1", NOW);
    let waited = raise_own(&mut s, "the schema waits on the user", "s2", NOW + 1);
    let other = ask(&mut s, "heat", LANE_PANE, "which schema?", "s3", NOW + 2);

    let mut args = reply_args(&[&resolved, &waited, &other], ReplyMode::Resolve, "r1");
    args.wait_until = Some(NOW + 100);
    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &args,
        NOW + 3,
    )
    .unwrap();

    assert_eq!(r.handled.len(), 3, "every listed id is still handled");
    assert_eq!(state_of(&mut s, &resolved), MsgState::Resolved);
    assert_eq!(state_of(&mut s, &waited), MsgState::Resolved);
    assert_eq!(r.replies.len(), 1, "only the lane is answered: {r:?}");
    let answer = s
        .read(|tx| tx.get_message("p", &r.replies[0]))
        .unwrap()
        .unwrap();
    assert_eq!(answer.recipient, "p:heat");

    // Nothing new lands in the captain's own inbox, so no doorbell follows:
    // `messages_to_since` lists exactly the reply/instruction kinds a doorbell
    // counts.
    let delivered = s
        .read(|tx| tx.messages_to_since("p", "p:captain", 0))
        .unwrap();
    assert!(
        delivered.is_empty(),
        "no message reaches the captain: {delivered:?}"
    );

    // A wait on its own item is silent the same way.
    let own_wait = raise_own(&mut s, "waiting on the user", "s4", NOW + 4);
    let mut args = reply_args(&[&own_wait], ReplyMode::Wait, "r2");
    args.wait_on = Some("user".into());
    args.wait_until = Some(NOW + 200);
    let r = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &args,
        NOW + 5,
    )
    .unwrap();
    assert_eq!(r.handled, vec![(own_wait.clone(), "waiting".to_string())]);
    assert!(
        r.replies.is_empty(),
        "a waiting captain rings nobody: {r:?}"
    );
    let item = s
        .read(|tx| tx.get_message("p", &own_wait))
        .unwrap()
        .unwrap();
    assert_eq!(item.state, MsgState::Waiting);
    assert_eq!(
        item.result.as_deref(),
        Some("the answer"),
        "the text is kept on the item"
    );
    assert!(s
        .read(|tx| tx.messages_to_since("p", "p:captain", 0))
        .unwrap()
        .is_empty());
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
        .map(|id| {
            s.read(|tx| tx.get_message("p", id))
                .unwrap()
                .unwrap()
                .thread
        })
        .collect();
    assert_eq!(
        threads,
        vec![
            "heat:1".to_string(),
            "heat:2".to_string(),
            "pricing:1".to_string()
        ]
    );
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
        .map(|id| {
            s.read(|tx| tx.get_message("p", id))
                .unwrap()
                .unwrap()
                .thread
        })
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

    assert_eq!(
        r.handled,
        vec![(id.clone(), "resolved".to_string())],
        "the id is listed once"
    );
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
    let err = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &args,
        NOW + 2,
    )
    .unwrap_err();
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
    assert_eq!(
        state_of(&mut s, &open),
        MsgState::Open,
        "the first id is untouched"
    );

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
    assert_eq!(
        state_of(&mut s, &other),
        MsgState::Open,
        "unlisted messages stay open"
    );
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

#[test]
fn resolving_information_is_silent_and_replays_without_a_reply() {
    let mut s = world();
    let id = s
        .write(|tx| {
            Ok(tx
                .insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "routine:1".into(),
                    task: None,
                    attempt: None,
                    sender: "p:heat".into(),
                    sender_gen: 1,
                    recipient: "p:captain".into(),
                    kind: MsgKind::Progress,
                    urgent: false,
                    body: "finished a step".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Info,
                    created: NOW,
                })?
                .id)
        })
        .unwrap();
    let args = reply_args(&[&id], ReplyMode::Resolve, "resolve-info");
    let first = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &args,
        NOW + 1,
    )
    .unwrap();
    assert_eq!(first.handled, vec![(id.clone(), "resolved".into())]);
    assert!(first.replies.is_empty());
    assert_eq!(state_of(&mut s, &id), MsgState::Resolved);
    let replay = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &args,
        NOW + 2,
    )
    .unwrap();
    assert!(replay.replayed);
    assert!(replay.replies.is_empty());
    assert!(s
        .read(|tx| tx.messages_to_since("p", "p:heat", 0))
        .unwrap()
        .is_empty());
}

#[test]
fn resolving_a_legacy_open_result_does_not_echo_to_the_lane() {
    let mut s = world();
    let id = s
        .write(|tx| {
            Ok(tx
                .insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "legacy:1".into(),
                    task: None,
                    attempt: None,
                    sender: "p:heat".into(),
                    sender_gen: 1,
                    recipient: "p:captain".into(),
                    kind: MsgKind::Result,
                    urgent: false,
                    body: "old hand-in".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: NOW,
                })?
                .id)
        })
        .unwrap();
    let result = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&id], ReplyMode::Resolve, "resolve-old"),
        NOW + 1,
    )
    .unwrap();
    assert!(result.replies.is_empty());
    assert_eq!(state_of(&mut s, &id), MsgState::Resolved);
    assert!(s
        .read(|tx| tx.messages_to_since("p", "p:heat", 0))
        .unwrap()
        .is_empty());
}

#[test]
fn resolving_info_and_a_question_in_one_thread_answers_only_the_question() {
    let mut s = world();
    let info = ask(&mut s, "heat", LANE_PANE, "routine record", "s1", NOW);
    s.write(|tx| tx.update_message_state("p", &info, MsgState::Info, None, None, None, NOW))
        .unwrap();
    let question = insert_question(
        &mut s,
        "heat:1",
        "p:heat",
        "p:captain",
        "needs an answer",
        NOW + 1,
    );
    let result = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &reply_args(&[&info, &question], ReplyMode::Resolve, "mixed"),
        NOW + 2,
    )
    .unwrap();
    assert_eq!(result.handled.len(), 2);
    assert_eq!(result.replies.len(), 1);
    let answer = s
        .read(|tx| tx.get_message("p", &result.replies[0]))
        .unwrap()
        .unwrap();
    assert_eq!(answer.reply_to.as_deref(), Some(question.as_str()));
}

#[test]
fn waiting_on_info_is_rejected_without_reopening_it_or_changing_other_items() {
    let mut s = world();
    let info = ask(&mut s, "heat", LANE_PANE, "routine record", "s1", NOW);
    s.write(|tx| tx.update_message_state("p", &info, MsgState::Info, None, None, None, NOW))
        .unwrap();
    let question = ask(&mut s, "heat", LANE_PANE, "a question", "s2", NOW + 1);
    let mut args = reply_args(&[&question, &info], ReplyMode::Wait, "wait-info");
    args.wait_until = Some(NOW + 100);
    let error = mail::reply(
        &mut s,
        &captain_env(),
        Some(&fp(CAP_PANE, "codex")),
        &args,
        NOW + 2,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadTransition);
    assert_eq!(state_of(&mut s, &info), MsgState::Info);
    assert_eq!(state_of(&mut s, &question), MsgState::Open);
    assert!(s
        .read(|tx| tx.messages_to_since("p", "p:heat", 0))
        .unwrap()
        .is_empty());
}
