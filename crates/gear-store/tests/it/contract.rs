//! Contract tests for gear-store. The implementation is done when all pass.
//! Do not edit these tests to make them pass; report back if one looks wrong.

use gear_model::{ErrorCode, MsgKind, MsgState, OpKind, OpState, SeatRole, SeatState};
use gear_store::{
    testing, BindingStatus, NewMessage, NewOp, RequestOutcome, Store, ViewRecord, VoyageStation,
    SCHEMA_VERSION,
};
use serde_json::json;

const NOW: i64 = 1_800_000_000;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn msg(sender: &str, recipient: &str, kind: MsgKind, state: MsgState, created: i64) -> NewMessage {
    NewMessage {
        meta: None,
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
    // T-41: the store stamps its version; it is the current one.
    assert_eq!(ver.as_deref(), Some(SCHEMA_VERSION.to_string().as_str()));
}

#[test]
fn refuses_newer_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    {
        let mut s = Store::open(&path).unwrap();
        s.write(|tx| tx.set_meta("schema_version", "999")).unwrap();
    }
    // T-41: a store of a newer version is refused for writing, with its own
    // error code: the schema only migrates up.
    let err = Store::open(&path).err().expect("must refuse");
    assert_eq!(err.code, ErrorCode::SchemaTooNew);
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
        .write(|tx| {
            tx.run_request("R1", "p|seat:1|send|hello", NOW, |_| {
                Ok(json!({"id": "M-1"}))
            })
        })
        .unwrap();
    assert_eq!(first, RequestOutcome::Executed(json!({"id": "M-1"})));

    let again = s
        .write(|tx| {
            tx.run_request("R1", "p|seat:1|send|hello", NOW, |_| {
                panic!("must not run twice")
            })
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
        tx.run_request("R2", "k", NOW, |_| {
            Err(gear_model::GearError::new(ErrorCode::NoTarget, "x"))
        })
    });
    // Retrying with the same id runs again (nothing was stored).
    let out = s
        .write(|tx| tx.run_request("R2", "k", NOW, |_| Ok(json!(1))))
        .unwrap();
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
        tx.insert_message(&msg(
            "p:a",
            "p:captain",
            MsgKind::Question,
            MsgState::Open,
            NOW,
        ))?;
        tx.insert_message(&msg(
            "p:a",
            "p:captain",
            MsgKind::Progress,
            MsgState::Info,
            NOW + 1,
        ))?;
        let m3 = tx.insert_message(&msg(
            "p:b",
            "p:captain",
            MsgKind::Blocked,
            MsgState::Open,
            NOW + 2,
        ))?;
        tx.update_message_state(
            "p",
            &m3.id,
            MsgState::Waiting,
            None,
            Some("user"),
            Some(NOW + 100),
            NOW + 3,
        )?;
        Ok(())
    })
    .unwrap();

    let now_items = s
        .read(|tx| tx.actionable_for("p", "p:captain", NOW + 10))
        .unwrap();
    assert_eq!(
        now_items.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["M-1"]
    );

    let later = s
        .read(|tx| tx.actionable_for("p", "p:captain", NOW + 100))
        .unwrap();
    assert_eq!(
        later.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["M-1", "M-3"]
    );

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

    s.write(|tx| tx.set_op_state("p", "O-1", OpState::Confirmed, None, NOW + 3))
        .unwrap();
    assert!(s.read(|tx| tx.slot_ops("p:captain")).unwrap().is_empty());

    let last = s
        .read(|tx| tx.last_op("p:captain", OpKind::Doorbell))
        .unwrap()
        .unwrap();
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
    assert_eq!(
        s.read(|tx| tx.views_since("p:captain", 1, NOW)).unwrap(),
        vec![v.clone()]
    );
    assert!(s
        .read(|tx| tx.views_since("p:captain", 2, NOW))
        .unwrap()
        .is_empty());
    assert!(s
        .read(|tx| tx.views_since("p:captain", 1, NOW + 1))
        .unwrap()
        .is_empty());
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
                (0..25)
                    .map(|_| s.write(|tx| tx.next_id("p", 'M')).unwrap())
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut all: Vec<String> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 100);
}

/// T-10: a raw state write whose target row does not exist is `NoTarget`,
/// never a silent success, and writes nothing.
#[test]
fn raw_writes_on_a_missing_target_are_no_target() {
    let mut s = store();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", NOW)?;
        Ok(())
    })
    .unwrap();
    let code = |r: gear_model::GearResult<()>| r.unwrap_err().code;
    s.write(|tx| {
        assert_eq!(
            code(tx.set_seat_state("p:ghost", SeatState::Standby, None, NOW)),
            ErrorCode::NoTarget
        );
        assert_eq!(
            code(tx.set_seat_generation("p:ghost", 2)),
            ErrorCode::NoTarget
        );
        assert_eq!(
            code(tx.set_binding_status("p:heat", 9, BindingStatus::Void)),
            ErrorCode::NoTarget
        );
        assert_eq!(
            code(tx.update_message_state("p", "M-404", MsgState::Resolved, None, None, None, NOW)),
            ErrorCode::NoTarget
        );
        assert_eq!(
            code(tx.set_op_state("p", "O-404", OpState::Void, None, NOW)),
            ErrorCode::NoTarget
        );
        // the existing rows still update fine
        tx.set_seat_state("p:heat", SeatState::Standby, Some("idle"), NOW)?;
        tx.set_binding_status("p:heat", 1, BindingStatus::Void)?;
        Ok(())
    })
    .unwrap();
    let seat = s.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Standby);
}

#[test]
fn project_ids_lists_every_project_in_order() {
    let mut s = store();
    assert!(s.read(|tx| tx.project_ids()).unwrap().is_empty());
    s.write(|tx| {
        testing::seed_project(tx, "zeta", "default", NOW)?;
        testing::seed_project(tx, "alpha", "default", NOW)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.read(|tx| tx.project_ids()).unwrap(),
        vec!["alpha".to_string(), "zeta".to_string()]
    );
}

/// T-6: a config change is the next config version of the project row.
#[test]
fn a_project_row_takes_the_next_config_version() {
    let mut s = store();
    s.write(|tx| testing::seed_project(tx, "p", "sess", NOW).map(|_| ()))
        .unwrap();
    let mut row = s.read(|tx| tx.get_project("p")).unwrap().unwrap();
    assert_eq!(row.config_version, 1);

    row.config_toml = "[project]\nid = \"p\"\n".to_string();
    row.config_version = 2;
    row.quota_lanes = 5;
    row.quota_temps = 2;
    row.quota_includes_captain = true;
    s.write(|tx| tx.update_project(&row)).unwrap();

    let after = s.read(|tx| tx.get_project("p")).unwrap().unwrap();
    assert_eq!(after.config_version, 2);
    assert_eq!(after.config_toml, row.config_toml);
    assert_eq!(
        (
            after.quota_lanes,
            after.quota_temps,
            after.quota_includes_captain
        ),
        (5, 2, true)
    );
    assert_eq!(after.created, NOW, "the creation time does not change");

    // A missing project is NoTarget.
    let mut ghost = row.clone();
    ghost.id = "ghost".to_string();
    assert_eq!(
        s.write(|tx| tx.update_project(&ghost)).unwrap_err().code,
        ErrorCode::NoTarget
    );
}

/// T-6: a lane's profile follows the config.
#[test]
fn a_seat_profile_is_written_and_read_back() {
    let mut s = store();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", NOW)
    })
    .unwrap();
    s.write(|tx| tx.set_seat_profile("p:heat", "codex"))
        .unwrap();
    assert_eq!(
        s.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap().profile,
        "codex"
    );
    assert_eq!(
        s.write(|tx| tx.set_seat_profile("p:ghost", "codex"))
            .unwrap_err()
            .code,
        ErrorCode::NoTarget
    );
}

#[test]
fn a_captain_quota_episode_survives_reopening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quota.db");
    let state = gear_store::CaptainQuota {
        generation: 7,
        detected_at: 100,
        reset_at: 200,
        bar: "5h 100% ↻01:10".into(),
        wake_op: Some("O-1".into()),
        views_before_wake: 3,
        resumed: false,
    };
    {
        let mut store = Store::open(&path).unwrap();
        store
            .write(|tx| tx.set_captain_quota("p", Some(&state)))
            .unwrap();
    }
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.read(|tx| tx.captain_quota("p")).unwrap(), Some(state));
    assert!(store.read(|tx| tx.captain_quota_paused("p")).unwrap());
    store.write(|tx| tx.set_captain_quota("p", None)).unwrap();
    assert!(!store.read(|tx| tx.captain_quota_paused("p")).unwrap());
}

/// T-329: the voyage's stations round-trip in plan order, in meta.
#[test]
fn voyage_stations_round_trip_in_order() {
    let mut s = store();
    let stations = vec![
        VoyageStation {
            id: "S1".into(),
            needs: "the window answers".into(),
        },
        VoyageStation {
            id: "S2".into(),
            needs: "the board shows the course".into(),
        },
    ];
    s.write(|tx| tx.set_voyage_stations("p", "M5", &stations))
        .unwrap();
    assert_eq!(
        s.read(|tx| tx.voyage_stations("p", "M5")).unwrap(),
        stations
    );
    // The key holds one `<id>: <needs>` line per station.
    assert_eq!(
        s.read(|tx| tx.get_meta("voyage-stations:p:M5"))
            .unwrap()
            .as_deref(),
        Some("S1: the window answers\nS2: the board shows the course")
    );
}

/// T-329: a needs text with a colon of its own round-trips; the line splits
/// only at its first `": "`.
#[test]
fn voyage_station_needs_keep_their_colons() {
    let mut s = store();
    let stations = vec![VoyageStation {
        id: "S2".into(),
        needs: "the board: all four views".into(),
    }];
    s.write(|tx| tx.set_voyage_stations("p", "M5", &stations))
        .unwrap();
    assert_eq!(
        s.read(|tx| tx.voyage_stations("p", "M5")).unwrap(),
        stations
    );
}

/// T-329: an empty list reads back empty, and so does another milestone or
/// another project.
#[test]
fn an_empty_or_other_voyage_has_no_stations() {
    let mut s = store();
    let stations = vec![VoyageStation {
        id: "S1".into(),
        needs: "the window answers".into(),
    }];
    s.write(|tx| tx.set_voyage_stations("p", "M5", &stations))
        .unwrap();
    assert!(s
        .read(|tx| tx.voyage_stations("p", "M6"))
        .unwrap()
        .is_empty());
    assert!(s
        .read(|tx| tx.voyage_stations("q", "M5"))
        .unwrap()
        .is_empty());
    s.write(|tx| tx.set_voyage_stations("p", "M5", &[]))
        .unwrap();
    assert_eq!(
        s.read(|tx| tx.get_meta("voyage-stations:p:M5"))
            .unwrap()
            .as_deref(),
        Some("")
    );
    assert!(s
        .read(|tx| tx.voyage_stations("p", "M5"))
        .unwrap()
        .is_empty());
}

/// T-329: a task's station round-trips and `None` clears it.
#[test]
fn a_task_station_round_trips_and_clears() {
    let mut s = store();
    assert_eq!(s.read(|tx| tx.task_station("p", "T-1")).unwrap(), None);
    s.write(|tx| tx.set_task_station("p", "T-1", Some("S2")))
        .unwrap();
    assert_eq!(
        s.read(|tx| tx.task_station("p", "T-1")).unwrap(),
        Some("S2".into())
    );
    // Another task and another project have none.
    assert_eq!(s.read(|tx| tx.task_station("p", "T-2")).unwrap(), None);
    assert_eq!(s.read(|tx| tx.task_station("q", "T-1")).unwrap(), None);
    s.write(|tx| tx.set_task_station("p", "T-1", None)).unwrap();
    assert_eq!(s.read(|tx| tx.task_station("p", "T-1")).unwrap(), None);
}
