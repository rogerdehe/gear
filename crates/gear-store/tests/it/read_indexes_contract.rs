//! T-517: the engine reads only the rows it needs (schema v8).
//!
//! A CPU sample of a running engine put most of its time in SQLite full-table
//! scans: `ops_in_state` (`WHERE project=? AND state=? ORDER BY created, id`)
//! and by-seat reads of attempts and rounds scanned their whole table on every
//! call, and so did `thread_messages` (`WHERE project=? AND thread=?`). Each of
//! them is called many times a tick, so the cost grew with the store.
//!
//! v8 adds `operations_state`, `messages_thread`, `attempts_seat` and
//! `rounds_seat`; a fresh store gets them from the schema and an older store
//! gets them from the migration that opens it.

use std::path::{Path, PathBuf};

use gear_model::{OpState, SeatRole, TaskState};
use gear_store::{Store, SCHEMA_VERSION};
use rusqlite::Connection;

const PROJECT: &str = "p";
const NOW: i64 = 1_800_000_000;

/// A schema-v7 store: the v8 indexes are not there, as on a store the engine
/// already had before this change.
fn a_v7_store(path: &Path) {
    let mut store = Store::open(path).unwrap();
    store
        .write(|tx| {
            gear_store::testing::seed_project(tx, PROJECT, "sess", NOW)?;
            Ok(())
        })
        .unwrap();
    drop(store);
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "DROP INDEX operations_state;
         DROP INDEX messages_thread;
         DROP INDEX attempts_seat;
         DROP INDEX rounds_seat;
         UPDATE meta SET value='7' WHERE key='schema_version';",
    )
    .unwrap();
}

fn temp_db(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{name}.db"));
    (dir, path)
}

/// The query plans of one statement, as SQLite words them.
fn plan_of(path: &Path, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Vec<String> {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    stmt.query_map(params, |row| row.get::<_, String>(3))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

/// What the engine asks for, as the store asks it: the index T-517 adds must
/// serve each read, and no plan may scan the whole table.
const READS: &[(&str, &str)] = &[
    (
        "ops_in_state",
        "SELECT project, id, seat, generation, kind, task, attempt, ref_msg, plan_version, \
         payload, state, result, check_budget, created, updated FROM operations \
         WHERE project=?1 AND state=?2 ORDER BY created, id",
    ),
    (
        "thread_messages",
        "SELECT project, id, thread, task, attempt, sender, sender_gen, recipient, kind, urgent, \
         body, reply_to, needs_ack, state, wait_on, wait_until, result, created, resolved, meta \
         FROM messages WHERE project=?1 AND thread=?2 ORDER BY created, id",
    ),
    (
        "in_progress_attempts_of_seat",
        "SELECT project, task FROM attempts WHERE seat=?1 \
         AND state IN ('delivered', 'started', 'stopped', 'unknown') ORDER BY task, n",
    ),
    (
        "open_attempts_of_seat",
        "SELECT project, task FROM attempts WHERE seat=?1 \
         AND state IN ('delivered', 'started', 'stopped', 'unknown', 'submitted') \
         ORDER BY state = 'submitted', task, n",
    ),
    (
        "open_round_of_seat",
        "SELECT project, task FROM rounds WHERE seat=?1 AND end_at IS NULL \
         ORDER BY start_at DESC LIMIT 1",
    ),
];

/// The index a read must use and the keyword of the scan it must not make.
fn wanted(read: &str) -> (&'static str, &'static str) {
    match read {
        "ops_in_state" => ("operations_state", "SEARCH operations USING"),
        "thread_messages" => ("messages_thread", "SEARCH messages USING"),
        "in_progress_attempts_of_seat" | "open_attempts_of_seat" => {
            ("attempts_seat", "SEARCH attempts USING")
        }
        "open_round_of_seat" => ("rounds_seat", "SEARCH rounds USING"),
        other => panic!("no index named for {other}"),
    }
}

/// Run every read's plan against one store file and check it.
fn plans_use_the_indexes(path: &Path, why: &str) {
    for (read, sql) in READS {
        // By seat, where the read is by seat; by state, where it is by state.
        let state = OpState::Submitted.as_str();
        let seat = format!("{PROJECT}:heat");
        let thread = "hot";
        let params: Vec<&dyn rusqlite::ToSql> = if read.starts_with("open_") {
            vec![&seat]
        } else if *read == "ops_in_state" {
            vec![&PROJECT, &state]
        } else if *read == "thread_messages" {
            vec![&PROJECT, &thread]
        } else {
            vec![&PROJECT]
        };
        let plan = plan_of(path, sql, &params);
        let (index, search) = wanted(read);
        let text = plan.join(" | ");
        assert!(
            text.contains(search),
            "{why}: {read} is not answered by an index: {text}"
        );
        assert!(
            !text.contains("SCAN operations")
                && !text.contains("SCAN messages")
                && !text.contains("SCAN attempts")
                && !text.contains("SCAN rounds"),
            "{why}: {read} scans its table: {text}"
        );
        assert!(
            text.contains(index),
            "{why}: {read} does not use {index}: {text}"
        );
    }
}

/// T-517: a fresh store carries the indexes and every tick read is answered by
/// one.
#[test]
fn a_fresh_store_has_the_indexes_the_tick_reads_by() {
    let (_dir, path) = temp_db("fresh");
    let _store = Store::open(&path).unwrap();
    plans_use_the_indexes(&path, "a fresh store");
}

/// T-517: a store made before v8 gains the indexes when it is opened — no hand
/// step, no row touched.
#[test]
fn an_older_store_gains_the_indexes_when_it_is_opened() {
    let (_dir, path) = temp_db("older");
    a_v7_store(&path);
    // Before: the reads scan.
    let before = plan_of(&path, READS[0].1, &[&PROJECT, &OpState::Submitted.as_str()]).join(" | ");
    assert!(
        !before.contains("operations_state"),
        "the fixture has no index yet: {before}"
    );

    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store.read(|tx| tx.get_meta("schema_version")).unwrap(),
        Some(SCHEMA_VERSION.to_string()),
        "the store was carried up to v8"
    );
    plans_use_the_indexes(&path, "an older store opened");
}

/// T-517 (v3): the plans do not depend on the store being empty. The store
/// holds 5000 operations and 5000 messages; the state the engine asks for on a
/// tick and the thread it reads are served by the same indexes as on a fresh
/// store.
///
/// The check is the plan, not the clock: v3 of the contract drops the timed
/// assertion because a timing test fails under the gate's parallel load.
#[test]
fn the_reads_keep_their_plans_on_a_store_of_five_thousand_rows() {
    let (_dir, path) = temp_db("bulk");
    let mut store = Store::open(&path).unwrap();
    store
        .write(|tx| {
            gear_store::testing::seed_project(tx, PROJECT, "sess", NOW)?;
            for n in 0..5000 {
                tx.insert_op(&gear_store::NewOp {
                    project: PROJECT.into(),
                    seat: format!("{PROJECT}:heat"),
                    generation: 1,
                    kind: gear_model::OpKind::Merge,
                    task: Some(format!("T-{n}")),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({}),
                    created: NOW + n,
                })?;
                // The first 50 are the state the engine reads; the rest are ops
                // that settled long ago.
                let state = if n < HOT as i64 {
                    OpState::Submitted
                } else {
                    OpState::Void
                };
                tx.set_op_state(PROJECT, &format!("O-{}", n + 1), state, None, NOW + n)?;
            }
            for n in 0..5000 {
                tx.insert_message(&gear_store::NewMessage {
                    meta: None,
                    project: PROJECT.into(),
                    thread: if n < HOT as i64 { "hot" } else { "cold" }.into(),
                    task: None,
                    attempt: None,
                    sender: format!("{PROJECT}:heat"),
                    sender_gen: 1,
                    recipient: format!("{PROJECT}:captain"),
                    kind: gear_model::MsgKind::Progress,
                    urgent: false,
                    body: "still working".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: gear_model::MsgState::Open,
                    created: NOW + n,
                })?;
            }
            Ok(())
        })
        .unwrap();

    plans_use_the_indexes(&path, "a store of 5000 ops");

    // The reads still find exactly the rows of their state and their thread.
    let ops = store
        .read(|tx| tx.ops_in_state(PROJECT, OpState::Submitted))
        .unwrap();
    assert_eq!(ops.len(), HOT, "the reads find the state's ops");
    let messages = store.read(|tx| tx.thread_messages(PROJECT, "hot")).unwrap();
    assert_eq!(messages.len(), HOT, "the read finds the thread's messages");
}

/// The rows the engine asks for on a tick, among the 5000 of each table.
const HOT: usize = 50;
