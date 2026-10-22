//! Contract tests for gear-store. The implementation is done when all pass.
//! Do not edit these tests to make them pass; report back if one looks wrong.

use gear_model::{ErrorCode, MsgKind, MsgState, OpKind, OpState, SeatRole, SeatState};
use gear_store::{testing, BindingStatus, NewMessage, NewOp, RequestOutcome, Store, ViewRecord};
use serde_json::json;

const NOW: i64 = 1_800_000_000;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn msg(sender: &str, recipient: &str, kind: MsgKind, state: MsgState, created: i64) -> NewMessage {
    NewMessage {
        project: "p".into(),
        thread: "T-1:1".into(),
        task: Some("T-1".into()),
        attempt: Some(1),
        sender: sender.into(),
        sender_gen: 1,
        recipient: recipient.into(),
        kind,
        urgent: false,
        body: "hello".into(),
        reply_to: None,
        needs_ack: false,
        state,
        created,
    }
}

#[test]
fn open_file_applies_schema_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    {
        let mut s = Store::open(&path).unwrap();
        s.write(|tx| tx.set_meta("k", "v")).unwrap();
    }
    let mut s = Store::open(&path).unwrap();
    let v = s.read(|tx| tx.get_meta("k")).unwrap();
    assert_eq!(v.as_deref(), Some("v"));
    let ver = s.read(|tx| tx.get_meta("schema_version")).unwrap();
    assert_eq!(ver.as_deref(), Some("1"));
}

#[test]
fn refuses_newer_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    {
        let mut s = Store::open(&path).unwrap();
        s.write(|tx| tx.set_meta("schema_version", "999")).unwrap();
    }
    let err = Store::open(&path).err().expect("must refuse");
    assert_eq!(err.code, ErrorCode::BadFormat);
}

#[test]
fn next_id_is_per_project_and_kind_and_monotonic() {
    let mut s = store();
    let ids = s
        .write(|tx| {
            Ok(vec![
                tx.next_id("p", 'M')?,
                tx.next_id("p", 'M')?,
                tx.next_id("p", 'T')?,
                tx.next_id("q", 'M')?,
            ])
        })
        .unwrap();
    assert_eq!(ids, vec!["M-1", "M-2", "T-1", "M-1"]);
}

#[test]
fn rolled_back_write_leaves_nothing() {
    let mut s = store();
    let r: Result<(), _> = s.write(|tx| {
        tx.set_meta("x", "1")?;
        Err(gear_model::GearError::new(ErrorCode::Forbidden, "boom"))
    });
    assert!(r.is_err());
    assert_eq!(s.read(|tx| tx.get_meta("x")).unwrap(), None);
}

#[test]
fn request_replay_and_conflict() {
    let mut s = store();
    let first = s
        .write(|tx| tx.run_request("R1", "p|seat:1|send|hello", NOW, |_| Ok(json!({"id": "M-1"}))))
        .unwrap();
    assert_eq!(first, RequestOutcome::Executed(json!({"id": "M-1"})));

    let again = s
        .write(|tx| {
            tx.run_request("R1", "p|seat:1|send|hello", NOW, |_| panic!("must not run twice"))
        })
        .unwrap();
    assert_eq!(again, RequestOutcome::Replayed(json!({"id": "M-1"})));

    let conflict = s
        .write(|tx| tx.run_request("R1", "p|seat:1|send|OTHER", NOW, |_| Ok(json!(null))))
        .err()
        .expect("conflict");
    assert_eq!(conflict.code, ErrorCode::RequestConflict);
}

#[test]
fn failed_request_is_not_recorded() {
    let mut s = store();
    let _ = s.write(|tx| {
        tx.run_request("R2", "k", NOW, |_| Err(gear_model::GearError::new(ErrorCode::NoTarget, "x")))
    });
    // Retrying with the same id runs again (nothing was stored).
    let out = s.write(|tx| tx.run_request("R2", "k", NOW, |_| Ok(json!(1)))).unwrap();
    assert_eq!(out, RequestOutcome::Executed(json!(1)));
}

#[test]
fn seats_bindings_roundtrip() {
    let mut s = store();
    let seat = s
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", NOW)
        })
        .unwrap();
    assert_eq!(seat.id, "p:captain");
    assert_eq!(seat.generation, 1);
    assert_eq!(seat.state, SeatState::Active);

    let (cap, b) = s
        .read(|tx| Ok((tx.captain_of("p")?, tx.get_binding("p:captain", 1)?)))
        .unwrap();
    assert_eq!(cap.unwrap().id, "p:captain");
    let b = b.unwrap();
    assert_eq!(b.status, BindingStatus::Active);
    assert_eq!(b.pane_id, "w1:p1");

    s.write(|tx| {
        tx.set_binding_status("p:captain", 1, BindingStatus::Void)?;
        tx.set_seat_state("p:captain", SeatState::Unavailable, Some("quota"), NOW + 5)
    })
    .unwrap();
    let seat = s.read(|tx| tx.get_seat("p:captain")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Unavailable);
    assert_eq!(seat.state_reason.as_deref(), Some("quota"));
    assert_eq!(seat.state_since, NOW + 5);
}

#[test]
fn messages_insert_and_actionable_query() {
    let mut s = store();
    s.write(|tx| {
        tx.insert_message(&msg("p:a", "p:captain", MsgKind::Question, MsgState::Open, NOW))?;
        tx.insert_message(&msg("p:a", "p:captain", MsgKind::Progress, MsgState::Info, NOW + 1))?;
        let m3 = tx.insert_message(&msg("p:b", "p:captain", MsgKind::Blocked, MsgState::Open, NOW + 2))?;
        tx.update_message_state("p", &m3.id, MsgState::Waiting, None, Some("user"), Some(NOW + 100), NOW + 3)?;
        Ok(())
    })
    .unwrap();

    let now_items = s.read(|tx| tx.actionable_for("p", "p:captain", NOW + 10)).unwrap();
    assert_eq!(now_items.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["M-1"]);

    let later = s.read(|tx| tx.actionable_for("p", "p:captain", NOW + 100)).unwrap();
    assert_eq!(later.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["M-1", "M-3"]);

    let m = s.read(|tx| tx.get_message("p", "M-3")).unwrap().unwrap();
    assert_eq!(m.wait_on.as_deref(), Some("user"));
}

#[test]
fn ops_slot_and_states() {
    let mut s = store();
    let op = s
        .write(|tx| {
            tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:captain".into(),
                generation: 1,
                kind: OpKind::Doorbell,
                task: None,
                attempt: None,
                ref_msg: Some("M-1".into()),
                plan_version: None,
                payload: json!({"msgs": ["M-1"]}),
                created: NOW,
            })
        })
        .unwrap();
    assert_eq!(op.id, "O-1");
    assert_eq!(op.state, OpState::Intended);

    let slot = s.read(|tx| tx.slot_ops("p:captain")).unwrap();
    assert_eq!(slot.len(), 1);

    s.write(|tx| {
        tx.set_op_state("p", "O-1", OpState::Executing, None, NOW + 1)?;
        tx.set_op_state("p", "O-1", OpState::Submitted, Some("ok"), NOW + 2)
    })
    .unwrap();
    assert_eq!(s.read(|tx| tx.slot_ops("p:captain")).unwrap().len(), 1);

    s.write(|tx| tx.set_op_state("p", "O-1", OpState::Confirmed, None, NOW + 3)).unwrap();
    assert!(s.read(|tx| tx.slot_ops("p:captain")).unwrap().is_empty());

    let last = s.read(|tx| tx.last_op("p:captain", OpKind::Doorbell)).unwrap().unwrap();
    assert_eq!(last.state, OpState::Confirmed);
    assert_eq!(last.payload, json!({"msgs": ["M-1"]}));
}

#[test]
fn views_roundtrip() {
    let mut s = store();
    let v = ViewRecord {
        project: "p".into(),
        seat: "p:captain".into(),
        generation: 1,
        ts: NOW,
        shown_msgs: vec!["M-1".into(), "M-2".into()],
        shown_ops: vec!["O-1".into()],
        shown_task: None,
        shown_attempt: None,
        shown_contract: None,
    };
    s.write(|tx| tx.insert_view(&v)).unwrap();
    assert_eq!(s.read(|tx| tx.views_since("p:captain", 1, NOW)).unwrap(), vec![v.clone()]);
    assert!(s.read(|tx| tx.views_since("p:captain", 2, NOW)).unwrap().is_empty());
    assert!(s.read(|tx| tx.views_since("p:captain", 1, NOW + 1)).unwrap().is_empty());
}

#[test]
fn concurrent_writers_do_not_duplicate_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    Store::open(&path).unwrap();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut s = Store::open(&path).unwrap();
                (0..25).map(|_| s.write(|tx| tx.next_id("p", 'M')).unwrap()).collect::<Vec<_>>()
            })
        })
        .collect();
    let mut all: Vec<String> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 100);
}
