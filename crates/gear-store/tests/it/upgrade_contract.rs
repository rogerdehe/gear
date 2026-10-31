//! T-491: a store made by the v0.5.6 schema opens on v0.6 with no hand step —
//! it migrates (or, while the schema is the same, opens) with no loss of
//! tasks, attempts, seats, messages or plan versions, and a second open is a
//! no-op. The fixture is a store as v0.5.6 wrote it: its schema (copied from
//! tag v0.5.6) and the rows of a project whose voyage is in progress.

use gear_model::{MsgState, TaskState};
use gear_store::{Store, SCHEMA_VERSION};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

const V056: &str = include_str!("v056_store.sql");

/// Write the fixture as a database file.
fn a_store_of_v056(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(V056).unwrap();
}

/// The version in `meta`.
fn version_of(store: &mut Store) -> String {
    store
        .read(|tx| tx.get_meta("schema_version"))
        .unwrap()
        .expect("the store has a version")
}

/// The shape of a store: its tables and, per table, its columns with their
/// NOT NULL flag (the same measure `migration_contract` uses).
fn shape_of(path: &Path) -> Vec<(String, Vec<(String, bool)>)> {
    let conn = Connection::open(path).unwrap();
    let mut tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    tables.sort();
    let mut shape = Vec::new();
    for table in tables {
        let mut columns: Vec<(String, bool)> = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(3)? != 0))
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        columns.sort();
        shape.push((table, columns));
    }
    shape
}

fn temp_db(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{name}.db"));
    (dir, path)
}

/// Everything the fixture holds, as the test reads it: the voyage, its task and
/// attempt, the seats, the open item and the plan version.
fn rows(store: &mut Store) -> (String, String, String, Vec<String>, String, i64) {
    let voyage = store
        .read(|tx| tx.voyages_of_project("p"))
        .unwrap()
        .into_iter()
        .find(|voyage| voyage.milestone == "M5")
        .expect("the voyage");
    let tasks = store.read(|tx| tx.tasks_of_project("p")).unwrap();
    let task = tasks
        .iter()
        .find(|task| task.id == "T-1")
        .expect("the task");
    assert_eq!(task.state, TaskState::Running);
    let attempts = store.read(|tx| tx.attempts_of_task("p", "T-1")).unwrap();
    let attempt = attempts.first().expect("the attempt");
    let seats: Vec<String> = store
        .read(|tx| tx.seats_of_project("p"))
        .unwrap()
        .into_iter()
        .map(|seat| format!("{}:{}", seat.id, seat.state.as_str()))
        .collect();
    let message = store
        .read(|tx| tx.get_message("p", "M-1"))
        .unwrap()
        .expect("the open item");
    assert_eq!(message.state, MsgState::Open);
    let plan = store.read(|tx| tx.plan_version("p")).unwrap();
    (
        voyage.title,
        voyage.done.unwrap_or_default(),
        format!("{}:{}", attempt.task, attempt.n),
        seats,
        message.body,
        plan,
    )
}

#[test]
fn a_v056_store_opens_with_no_loss_and_a_second_open_is_a_no_op() {
    let (_dir, path) = temp_db("upgrade-v056");
    a_store_of_v056(&path);

    // The current binary opens it: the version moves to the current one (the
    // fixture is v7 and the current schema is v7, so no step runs; when a later
    // schema bumps, the steps carry it there) and the shape is the current
    // schema's.
    let mut store = Store::open(&path).unwrap();
    assert_eq!(version_of(&mut store), SCHEMA_VERSION.to_string());
    let before = rows(&mut store);
    assert_eq!(before.0, "the next voyage");
    assert_eq!(before.1, "it sails");
    assert_eq!(before.2, "T-1:1");
    assert_eq!(before.3, vec!["p:captain:active", "p:heat:active"]);
    assert_eq!(before.4, "the voyage is in progress");
    assert_eq!(before.5, 1, "the plan version");
    drop(store);

    // The shape after the open is the shape of a fresh current store.
    let (_fresh_dir, fresh_path) = temp_db("upgrade-fresh");
    Store::open(&fresh_path).unwrap();
    assert_eq!(shape_of(&path), shape_of(&fresh_path));

    // A second open changes nothing: the version stays, every row is there.
    let mut store = Store::open(&path).unwrap();
    assert_eq!(version_of(&mut store), SCHEMA_VERSION.to_string());
    assert_eq!(rows(&mut store), before);
}
