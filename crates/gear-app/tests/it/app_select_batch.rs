//! Tests for `gear_app::mail::select_batch` (T-16).
//!
//! The doc comment of `select_batch` is the spec: candidates are the open and
//! due waiting messages, grouped by thread, urgent threads first, one slot
//! reserved for the oldest normal thread, at most `per_sender` threads per
//! sender and at most `max_threads` threads. Reading changes nothing.

use gear_app::mail::{select_batch, BatchLimits};
use gear_model::{MsgKind, MsgState, Ts};
use gear_store::{testing, NewMessage, Store};

const NOW: i64 = 1_800_000_000;
const CAPTAIN: &str = "p:captain";

fn msg(thread: &str, sender: &str, kind: MsgKind, created: Ts, urgent: bool) -> NewMessage {
    NewMessage {
        meta: None,
        project: "p".into(),
        thread: thread.into(),
        task: None,
        attempt: None,
        sender: sender.into(),
        sender_gen: 1,
        recipient: CAPTAIN.into(),
        kind,
        urgent,
        body: format!("body {created}"),
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created,
    }
}

fn store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| testing::seed_project(tx, "p", "sess", NOW))
        .unwrap();
    s
}

fn batch(s: &mut Store, now: Ts, limits: BatchLimits) -> Vec<String> {
    s.read(|tx| select_batch(tx, "p", CAPTAIN, now, limits))
        .unwrap()
        .iter()
        .map(|t| t.thread.clone())
        .collect()
}

#[test]
fn groups_by_thread_with_more_count() {
    let mut s = store();
    s.write(|tx| {
        for i in 0..5 {
            tx.insert_message(&msg("T-1:1", "p:heat", MsgKind::Question, NOW + i, false))?;
        }
        tx.insert_message(&msg(
            "T-2:1",
            "p:pricing",
            MsgKind::Question,
            NOW + 5,
            false,
        ))?;
        Ok(())
    })
    .unwrap();

    let views = s
        .read(|tx| select_batch(tx, "p", CAPTAIN, NOW + 10, BatchLimits::default()))
        .unwrap();
    assert_eq!(views.len(), 2);

    let first = &views[0];
    assert_eq!(first.thread, "T-1:1");
    assert_eq!(
        first.open.len(),
        3,
        "at most three unhandled messages are shown"
    );
    assert_eq!(first.more, 2, "the rest is counted");
    // Oldest first.
    assert_eq!(first.open[0].id, "M-1");
    assert_eq!(first.open[1].id, "M-2");
    assert_eq!(first.open[2].id, "M-3");

    let second = &views[1];
    assert_eq!(second.thread, "T-2:1");
    assert_eq!(second.open.len(), 1);
    assert_eq!(second.more, 0);
}

#[test]
fn due_waiting_included_not_yet_due_excluded() {
    let mut s = store();
    s.write(|tx| {
        tx.insert_message(&msg("open", "p:heat", MsgKind::Question, NOW, false))?;
        let due = tx.insert_message(&msg("due", "p:pricing", MsgKind::Question, NOW + 1, false))?;
        tx.update_message_state(
            "p",
            &due.id,
            MsgState::Waiting,
            None,
            Some("user"),
            Some(NOW + 100),
            NOW + 1,
        )?;
        let later =
            tx.insert_message(&msg("later", "p:other", MsgKind::Question, NOW + 2, false))?;
        tx.update_message_state(
            "p",
            &later.id,
            MsgState::Waiting,
            None,
            Some("user"),
            Some(NOW + 200),
            NOW + 2,
        )?;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        batch(&mut s, NOW + 50, BatchLimits::default()),
        vec!["open"]
    );
    assert_eq!(
        batch(&mut s, NOW + 150, BatchLimits::default()),
        vec!["open", "due"]
    );
    assert_eq!(
        batch(&mut s, NOW + 250, BatchLimits::default()),
        vec!["open", "due", "later"]
    );
}

#[test]
fn last_reply_is_newest_reply() {
    let mut s = store();
    s.write(|tx| {
        tx.insert_message(&msg("t1", "p:heat", MsgKind::Question, NOW, false))?;
        // Real replies are stored as info (design/07): they are not candidates
        // themselves, so only a read of the whole thread finds them.
        let mut reply = msg("t1", "p:captain", MsgKind::Reply, NOW + 1, false);
        reply.state = MsgState::Info;
        tx.insert_message(&reply)?;
        let mut newer = msg("t1", "p:captain", MsgKind::Reply, NOW + 2, false);
        newer.state = MsgState::Info;
        tx.insert_message(&newer)?;
        // An older question in another thread must not become the last reply.
        tx.insert_message(&msg("t2", "p:heat", MsgKind::Question, NOW + 3, false))?;
        Ok(())
    })
    .unwrap();

    let views = s
        .read(|tx| select_batch(tx, "p", CAPTAIN, NOW + 10, BatchLimits::default()))
        .unwrap();
    let t1 = views.iter().find(|v| v.thread == "t1").expect("thread t1");
    let last = t1.last_reply.as_ref().expect("the thread has replies");
    assert_eq!(last.id, "M-3");
    assert_eq!(last.kind, MsgKind::Reply);
    // The replies are not unhandled items of the thread.
    assert_eq!(t1.open.len(), 1);
    assert_eq!(t1.open[0].id, "M-1");
    let t2 = views.iter().find(|v| v.thread == "t2").expect("thread t2");
    assert!(t2.last_reply.is_none(), "no reply in this thread");
}

#[test]
fn a_reply_to_another_seat_is_not_this_seats_context() {
    // T-103: the lane reads its own thread, so a reply the captain wrote to
    // the engine (or to another seat) is not its context — only a reply
    // addressed to the lane itself is.
    let mut s = store();
    s.write(|tx| {
        // The lane's own open item (an instruction from the captain).
        let mut item = msg("t1", "p:captain", MsgKind::Instruction, NOW, false);
        item.recipient = "p:heat".into();
        tx.insert_message(&item)?;
        let mut to_engine = msg("t1", "p:captain", MsgKind::Reply, NOW + 1, false);
        to_engine.recipient = "engine".into();
        to_engine.state = MsgState::Info;
        tx.insert_message(&to_engine)?;
        let mut to_other = msg("t1", "p:captain", MsgKind::Reply, NOW + 2, false);
        to_other.recipient = "p:pricing".into();
        to_other.state = MsgState::Info;
        tx.insert_message(&to_other)?;
        Ok(())
    })
    .unwrap();

    let views = s
        .read(|tx| select_batch(tx, "p", "p:heat", NOW + 10, BatchLimits::default()))
        .unwrap();
    let t1 = views.iter().find(|v| v.thread == "t1").expect("thread t1");
    assert!(t1.last_reply.is_none(), "a reply to the engine is not mine");

    // A reply to the lane itself is shown.
    s.write(|tx| {
        let mut to_me = msg("t1", "p:captain", MsgKind::Reply, NOW + 3, false);
        to_me.recipient = "p:heat".into();
        to_me.state = MsgState::Info;
        tx.insert_message(&to_me)?;
        Ok(())
    })
    .unwrap();
    let views = s
        .read(|tx| select_batch(tx, "p", "p:heat", NOW + 10, BatchLimits::default()))
        .unwrap();
    let t1 = views.iter().find(|v| v.thread == "t1").expect("thread t1");
    let last = t1.last_reply.as_ref().expect("the lane's reply");
    assert_eq!(last.recipient, "p:heat");
}

#[test]
fn urgent_first_then_oldest() {
    let mut s = store();
    s.write(|tx| {
        tx.insert_message(&msg("old", "p:heat", MsgKind::Question, NOW, false))?;
        tx.insert_message(&msg(
            "urgent",
            "p:pricing",
            MsgKind::Question,
            NOW + 1,
            true,
        ))?;
        tx.insert_message(&msg("new", "p:other", MsgKind::Question, NOW + 2, false))?;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        batch(&mut s, NOW + 10, BatchLimits::default()),
        vec!["urgent", "old", "new"]
    );
}

#[test]
fn urgent_flood_keeps_one_normal_slot() {
    let mut s = store();
    s.write(|tx| {
        tx.insert_message(&msg("u1", "p:a", MsgKind::Question, NOW, true))?;
        tx.insert_message(&msg("u2", "p:b", MsgKind::Question, NOW + 1, true))?;
        tx.insert_message(&msg("u3", "p:c", MsgKind::Question, NOW + 2, true))?;
        tx.insert_message(&msg("normal", "p:d", MsgKind::Question, NOW + 3, false))?;
        Ok(())
    })
    .unwrap();

    let limits = BatchLimits {
        max_threads: 2,
        per_sender: 2,
    };
    let threads = batch(&mut s, NOW + 10, limits);
    assert_eq!(threads.len(), 2);
    assert!(
        threads.contains(&"normal".to_string()),
        "the oldest normal thread keeps a slot"
    );
    assert!(
        threads.contains(&"u1".to_string()),
        "the other slot goes to the oldest urgent thread"
    );
}

#[test]
fn per_sender_cap() {
    let mut s = store();
    s.write(|tx| {
        tx.insert_message(&msg("t1", "p:heat", MsgKind::Question, NOW, false))?;
        tx.insert_message(&msg("t2", "p:heat", MsgKind::Question, NOW + 1, false))?;
        tx.insert_message(&msg("t3", "p:other", MsgKind::Question, NOW + 2, false))?;
        Ok(())
    })
    .unwrap();

    let limits = BatchLimits {
        max_threads: 6,
        per_sender: 1,
    };
    let threads = batch(&mut s, NOW + 10, limits);
    assert_eq!(threads.len(), 2, "one thread per sender");
    assert_eq!(threads, vec!["t1".to_string(), "t3".to_string()]);
}

#[test]
fn max_threads_cap() {
    let mut s = store();
    s.write(|tx| {
        for i in 0..5 {
            tx.insert_message(&msg(
                &format!("t{i}"),
                "p:heat",
                MsgKind::Question,
                NOW + i,
                false,
            ))?;
        }
        Ok(())
    })
    .unwrap();

    let limits = BatchLimits {
        max_threads: 2,
        per_sender: 6,
    };
    let threads = batch(&mut s, NOW + 10, limits);
    assert_eq!(threads.len(), 2);
    assert_eq!(threads, vec!["t0".to_string(), "t1".to_string()]);
}

#[test]
fn zero_max_threads_returns_nothing() {
    let mut s = store();
    s.write(|tx| {
        tx.insert_message(&msg("normal", "p:heat", MsgKind::Question, NOW, false))?;
        tx.insert_message(&msg("urgent", "p:other", MsgKind::Question, NOW + 1, true))?;
        Ok(())
    })
    .unwrap();

    // No slot at all: not even the reserved one.
    let limits = BatchLimits {
        max_threads: 0,
        per_sender: 2,
    };
    assert!(batch(&mut s, NOW + 10, limits).is_empty());
}

#[test]
fn selection_changes_nothing() {
    let mut s = store();
    s.write(|tx| {
        tx.insert_message(&msg("t1", "p:heat", MsgKind::Question, NOW, true))?;
        let m2 = tx.insert_message(&msg("t2", "p:heat", MsgKind::Question, NOW + 1, false))?;
        tx.update_message_state(
            "p",
            &m2.id,
            MsgState::Waiting,
            None,
            Some("user"),
            Some(NOW + 100),
            NOW + 1,
        )?;
        Ok(())
    })
    .unwrap();

    let before = s
        .read(|tx| tx.actionable_for("p", CAPTAIN, NOW + 10))
        .unwrap();
    let _ = s
        .read(|tx| select_batch(tx, "p", CAPTAIN, NOW + 10, BatchLimits::default()))
        .unwrap();
    let after = s
        .read(|tx| tx.actionable_for("p", CAPTAIN, NOW + 10))
        .unwrap();
    assert_eq!(before, after, "reading the inbox changes no message");

    let m2 = s.read(|tx| tx.get_message("p", "M-2")).unwrap().unwrap();
    assert_eq!(m2.state, MsgState::Waiting);
}
