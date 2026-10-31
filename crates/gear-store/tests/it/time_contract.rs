//! Contract for the per-attempt step times (T-38): every step writes its own
//! time, and no step overwrites another.

use gear_model::AttemptState;
use gear_store::{testing, AttemptRow, AttemptStep, AttemptTimes, Store};

const T0: i64 = 1_800_000_000;

fn attempt() -> AttemptRow {
    AttemptRow {
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
        project: "p".into(),
        task: "T-1".into(),
        n: 1,
        seat: "p:heat".into(),
        generation: 1,
        contract_version: 1,
        adopted_version: 1,
        pending_update: None,
        state: AttemptState::Delivered,
        candidate: None,
        report: None,
        safe_point: None,
        started: T0,
        updated: T0,
    }
}

/// A store with one attempt.
fn store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        tx.insert_attempt(&attempt())?;
        Ok(())
    })
    .unwrap();
    s
}

#[test]
fn every_step_writes_its_own_time() {
    let mut s = store();
    s.write(|tx| {
        for (i, step) in AttemptStep::ALL.iter().enumerate() {
            tx.set_attempt_time("p", "T-1", 1, *step, T0 + (i as i64 + 1) * 60)?;
        }
        Ok(())
    })
    .unwrap();

    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.delivered, Some(T0 + 60));
    assert_eq!(times.started, Some(T0 + 120));
    assert_eq!(times.handed_in, Some(T0 + 180));
    assert_eq!(times.verdict, Some(T0 + 240));
    assert_eq!(times.settled, Some(T0 + 300));
    assert_eq!(times.merged, Some(T0 + 360));
}

#[test]
fn a_step_never_overwrites_another() {
    let mut s = store();
    // The steps in another order: each one keeps its own column.
    s.write(|tx| {
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Merged, T0 + 60)?;
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Delivered, T0 + 120)?;
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Settled, T0 + 180)?;
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0 + 240)?;
        Ok(())
    })
    .unwrap();

    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.delivered, Some(T0 + 120));
    assert_eq!(times.started, Some(T0 + 240));
    assert_eq!(times.settled, Some(T0 + 180));
    assert_eq!(times.merged, Some(T0 + 60));
    // The steps that did not happen are empty.
    assert_eq!(times.handed_in, None);
    assert_eq!(times.verdict, None);
}

#[test]
fn the_first_time_of_a_step_stands() {
    let mut s = store();
    s.write(|tx| {
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0 + 60)?;
        // The same step again (a replayed settle, a repeated delivery).
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0 + 600)?;
        Ok(())
    })
    .unwrap();
    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.started, Some(T0 + 60), "a fact does not move");
}

#[test]
fn a_row_replacement_keeps_the_times() {
    let mut s = store();
    s.write(|tx| {
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Delivered, T0 + 60)?;
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0 + 120)?;
        Ok(())
    })
    .unwrap();

    // The attempt row is replaced (a state change, a contract update): the
    // step times stay where they are.
    s.write(|tx| {
        let mut row = tx.get_attempt("p", "T-1", 1)?.unwrap();
        row.state = AttemptState::Started;
        row.updated = T0 + 900;
        tx.update_attempt(&row)
    })
    .unwrap();

    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.delivered, Some(T0 + 60));
    assert_eq!(times.started, Some(T0 + 120));
}

#[test]
fn the_time_of_an_unknown_attempt_is_no_target() {
    let mut s = store();
    let err = s
        .write(|tx| tx.set_attempt_time("p", "T-9", 1, AttemptStep::Started, T0))
        .unwrap_err();
    assert_eq!(err.code, gear_model::ErrorCode::NoTarget);
}

#[test]
fn the_times_of_a_task_are_read_in_order() {
    let mut s = store();
    s.write(|tx| {
        let mut second = attempt();
        second.n = 2;
        tx.insert_attempt(&second)?;
        tx.set_attempt_time("p", "T-1", 2, AttemptStep::Started, T0 + 60)?;
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0 + 120)?;
        Ok(())
    })
    .unwrap();

    let times = s.read(|tx| tx.attempt_times_of_task("p", "T-1")).unwrap();
    assert_eq!(times.len(), 2);
    assert_eq!(
        times[0],
        (
            1,
            AttemptTimes {
                started: Some(T0 + 120),
                ..Default::default()
            }
        )
    );
    assert_eq!(
        times[1],
        (
            2,
            AttemptTimes {
                started: Some(T0 + 60),
                ..Default::default()
            }
        )
    );
}

/// A database from before the times existed gains the columns when it is
/// opened (T-38): the columns are nullable, so the rows that are already
/// there stay readable.
#[test]
fn an_older_database_gains_the_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    {
        let mut s = Store::open(&path).unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            tx.insert_attempt(&attempt())?;
            tx.set_voyage_departure("p", "M2", T0, 1, T0)?;
            tx.set_voyage_departure_estimate("p", "M2", 30)?;
            Ok(())
        })
        .unwrap();
    }
    // Take the columns away again: the database is as it was before T-38.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "ALTER TABLE attempts DROP COLUMN delivered_at;
             ALTER TABLE attempts DROP COLUMN started_at;
             ALTER TABLE attempts DROP COLUMN handed_in_at;
             ALTER TABLE attempts DROP COLUMN verdict_at;
             ALTER TABLE attempts DROP COLUMN settled_at;
             ALTER TABLE attempts DROP COLUMN merged_at;
             ALTER TABLE tasks DROP COLUMN estimate_min;
             ALTER TABLE voyages DROP COLUMN departure_estimate_min;",
        )
        .unwrap();
        // The old shape predates v2. Restore its stamp too, so this fixture
        // represents an older store rather than a corrupt current store.
        conn.execute(
            "UPDATE meta SET value=?1 WHERE key='schema_version'",
            [gear_store::OLDEST_VERSION.to_string()],
        )
        .unwrap();
    }

    // Opening it migrates it up: the columns are back and the rows are there.
    let mut s = Store::open(&path).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M2")).unwrap().unwrap();
    assert_eq!(voyage.departure, Some(T0));
    assert_eq!(voyage.departure_estimate_min, None);
    s.write(|tx| tx.set_voyage_departure_estimate("p", "M2", 30))
        .unwrap();
    assert_eq!(
        s.read(|tx| tx.get_voyage("p", "M2"))
            .unwrap()
            .unwrap()
            .departure_estimate_min,
        Some(30)
    );
    s.write(|tx| tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0 + 60))
        .unwrap();
    let times = s.read(|tx| tx.attempt_times("p", "T-1", 1)).unwrap();
    assert_eq!(times.started, Some(T0 + 60));
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .task,
        "T-1"
    );
}

#[test]
fn departure_estimate_is_fixed_once() {
    let mut s = store();
    s.write(|tx| {
        tx.set_voyage_departure("p", "M2", T0, 1, T0)?;
        tx.set_voyage_departure_estimate("p", "M2", 30)?;
        tx.set_voyage_departure_estimate("p", "M2", 90)?;
        Ok(())
    })
    .unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M2")).unwrap().unwrap();
    assert_eq!(voyage.departure_estimate_min, Some(30));
}
