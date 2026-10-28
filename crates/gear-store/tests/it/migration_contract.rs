//! T-41: the store has a schema version.
//!
//! - a fresh store is stamped with the current version;
//! - a store of an older version is migrated up, and keeps its rows;
//! - a store of a newer version is refused for writing, and nothing is written.

use gear_model::ErrorCode;
use gear_store::{Store, SCHEMA_VERSION};
use rusqlite::Connection;

/// The schema of v1, as the first release wrote it.
const V1_SCHEMA: &str = include_str!("v1_schema.sql");

/// The `voyages` of T-22 attempt 1: `done`, `bar` and `eta` were NOT NULL.
/// A store that a development build opened then has this shape.
const VOYAGES_V1_ATTEMPT_1: &str =
    "CREATE TABLE voyages (project TEXT NOT NULL, milestone TEXT NOT NULL,
  title TEXT NOT NULL, done TEXT NOT NULL, bar TEXT NOT NULL, eta TEXT NOT NULL,
  departure INTEGER, departure_tasks INTEGER, updated INTEGER NOT NULL,
  PRIMARY KEY (project, milestone));";

/// The version a store of v1 carries.
const V1: &str = "1";

/// The version in `meta`.
fn version_of(store: &mut Store) -> String {
    store
        .read(|tx| tx.get_meta("schema_version"))
        .unwrap()
        .expect("the store has a version")
}

/// The shape of a store: its tables and, per table, its columns with their
/// NOT NULL flag. The order of the columns is left out: a migration may add a
/// column at the end where the schema puts it in the middle.
fn shape_of(path: &std::path::Path) -> Vec<(String, Vec<(String, bool)>)> {
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

/// A store of v1 with rows in it: the schema of v1, the stamp of v1 and the
/// `voyages` of T-22 attempt 1 (the shape a development build left behind).
fn a_store_of_v1(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(V1_SCHEMA).unwrap();
    conn.execute_batch(VOYAGES_V1_ATTEMPT_1).unwrap();
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
        [V1],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO projects (id, herdr_session, config_toml, config_version, quota_lanes,
            quota_temps, quota_includes_captain, created)
         VALUES ('p', 'sess', '', 1, 4, 2, 0, 100)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO seats (id, project, role, name, profile, generation, state, state_since, slot)
         VALUES ('p:heat', 'p', 'lane', 'heat', 'codex', 1, 'active', 100, 'w1:p2')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (project, id, rank, state, priority, contract_version, updated)
         VALUES ('p', 'T-1', 1, 'done', 'must', 1, 100)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages (project, id, thread, task, sender, sender_gen, recipient, kind,
            urgent, body, needs_ack, state, created)
         VALUES ('p', 'M-1', 'T-1:1', 'T-1', 'p:heat', 1, 'p:captain', 'progress', 0, 'hello', 0, 'open', 100)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO voyages (project, milestone, title, done, bar, eta, departure, departure_tasks, updated)
         VALUES ('p', 'M1', 'the first milestone', 'it works', 'green', 'today', 90, 3, 95)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO slots (project, id, pane_id, worktree, state) VALUES ('p', 'S-1', 'w1:p2', '/tmp/w', 'held')",
        [],
    )
    .unwrap();
}

#[test]
fn a_fresh_store_has_the_current_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");

    let mut store = Store::open(&path).unwrap();
    assert_eq!(version_of(&mut store), SCHEMA_VERSION.to_string());

    // The version is in `meta`.
    let stamp = store.read(|tx| tx.get_meta("schema_version")).unwrap();
    assert_eq!(stamp.as_deref(), Some(SCHEMA_VERSION.to_string().as_str()));
}

#[test]
fn an_old_store_is_migrated_up_and_keeps_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    a_store_of_v1(&path);

    // A development v1 store can already have T-39's estimate. Rebuilding
    // the historical NOT NULL voyage shape must preserve that value too.
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "ALTER TABLE voyages ADD COLUMN departure_estimate_min INTEGER;
         UPDATE voyages SET departure_estimate_min=30;",
    )
    .unwrap();
    drop(conn);

    // The binary opens the store: it is carried up to the current version.
    let mut store = Store::open(&path).unwrap();
    assert_eq!(version_of(&mut store), SCHEMA_VERSION.to_string());
    assert_ne!(version_of(&mut store), V1);

    // T-38: CREATE TABLE IF NOT EXISTS cannot add these to legacy tables.
    let shape = shape_of(&path);
    for (table, column) in [
        ("tasks", "estimate_min"),
        ("attempts", "delivered_at"),
        ("attempts", "started_at"),
        ("attempts", "handed_in_at"),
        ("attempts", "verdict_at"),
        ("attempts", "settled_at"),
        ("attempts", "merged_at"),
        ("voyages", "departure_estimate_min"),
    ] {
        assert!(
            shape.iter().any(|(name, columns)| name == table
                && columns
                    .iter()
                    .any(|(name, not_null)| name == column && !not_null)),
            "missing nullable {table}.{column}"
        );
    }

    // The rows are still there.
    let project = store.read(|tx| tx.get_project("p")).unwrap().unwrap();
    assert_eq!(project.quota_lanes, 4);
    let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    assert_eq!(seat.profile, "codex");
    assert_eq!(seat.generation, 1);
    let task = store.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.priority, "must");
    let message = store
        .read(|tx| tx.get_message("p", "M-1"))
        .unwrap()
        .unwrap();
    assert_eq!(message.body, "hello");

    // The voyage of T-22 attempt 1 is in the canonical shape now, and its row
    // is still there.
    let voyage = store.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(voyage.done.as_deref(), Some("it works"));
    assert_eq!(voyage.departure, Some(90));
    assert_eq!(voyage.departure_tasks, Some(3));
    assert_eq!(voyage.departure_estimate_min, Some(30));
}

#[test]
fn a_migrated_store_has_the_shape_of_a_fresh_one() {
    let dir = tempfile::tempdir().unwrap();
    let migrated = dir.path().join("migrated.db");
    let fresh = dir.path().join("fresh.db");
    a_store_of_v1(&migrated);
    Store::open(&fresh).unwrap();

    Store::open(&migrated).unwrap();

    // Every table of the migrated store is the table of a fresh store: the
    // `slots` table is gone, `voyages` is there, `seats` has its `worktree`
    // and `resources` its branch.
    assert_eq!(shape_of(&migrated), shape_of(&fresh));
}

#[test]
fn a_newer_store_is_refused_for_writes_and_nothing_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    {
        let mut store = Store::open(&path).unwrap();
        store
            .write(|tx| tx.set_meta("schema_version", "999"))
            .unwrap();
        store
            .write(|tx| tx.set_meta("marker", "untouched"))
            .unwrap();
    }

    // The store is of a newer version: it is refused for writing.
    let err = Store::open(&path).err().expect("the store must be refused");
    assert_eq!(err.code, ErrorCode::SchemaTooNew);
    assert!(err.detail.contains("999"), "{}", err.detail);
    assert!(
        err.detail.contains(&SCHEMA_VERSION.to_string()),
        "{}",
        err.detail
    );

    // Nothing was written: the store still says 999 and still has its row.
    let conn = Connection::open(&path).unwrap();
    let version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "999");
    let marker: String = conn
        .query_row("SELECT value FROM meta WHERE key='marker'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(marker, "untouched");
}

#[test]
fn a_store_of_the_current_version_is_opened_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    {
        let mut store = Store::open(&path).unwrap();
        store.write(|tx| tx.set_meta("marker", "kept")).unwrap();
    }

    let mut store = Store::open(&path).unwrap();
    assert_eq!(version_of(&mut store), SCHEMA_VERSION.to_string());
    let marker = store.read(|tx| tx.get_meta("marker")).unwrap();
    assert_eq!(marker.as_deref(), Some("kept"));
}

#[test]
fn a_store_without_a_version_is_migrated_up() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    {
        // A store from before the version existed: tables, no stamp.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(V1_SCHEMA).unwrap();
    }

    let mut store = Store::open(&path).unwrap();
    assert_eq!(version_of(&mut store), SCHEMA_VERSION.to_string());
}
