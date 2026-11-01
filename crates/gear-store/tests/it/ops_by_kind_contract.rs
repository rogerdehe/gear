//! T-523: the engine reads only the ops it needs (schema v9).
//!
//! The merge step asked the store for every op of a project in seven states and
//! kept the merge ones in Rust, and the lane's nudge asked for every delivery
//! the seat ever had and kept one attempt's — on a store with thousands of
//! deliveries and doorbells that parsed every payload to find a few hundred
//! small merge ops. v9 puts the filter in SQL: `ops_of_kind_in_states`,
//! `op_keys_of_kind` (the columns only) and `ops_of_seat_kind_task`, served by
//! `operations_kind` (project, kind, state) and `operations_attempt`
//! (seat, kind, task, attempt).

use std::path::{Path, PathBuf};

use gear_model::{OpKind, OpState};
use gear_store::{NewOp, Store, SCHEMA_VERSION};
use rusqlite::Connection;

const PROJECT: &str = "p";
const NOW: i64 = 1_800_000_000;

fn temp_db(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{name}.db"));
    (dir, path)
}

fn plan_of(path: &Path, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Vec<String> {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    stmt.query_map(params, |row| row.get::<_, String>(3))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

/// A store of several kinds, states, tasks and seats: merge ops in all seven
/// states, deliveries for two seats and two attempts, and the doorbells that
/// used to be parsed with them.
fn a_store_of_ops(path: &Path) -> Store {
    let mut store = Store::open(path).unwrap();
    store
        .write(|tx| {
            gear_store::testing::seed_project(tx, PROJECT, "sess", NOW)?;
            let states = [
                OpState::Intended,
                OpState::Executing,
                OpState::Submitted,
                OpState::Unknown,
                OpState::Confirmed,
                OpState::Failed,
                OpState::Void,
            ];
            // (kind, state, seat, task, attempt)
            let mut plan: Vec<(OpKind, OpState, &str, Option<&str>, Option<i64>)> = Vec::new();
            // One merge op per state, for T-1 and T-2, on two seats.
            for (k, state) in states.iter().enumerate() {
                let task = if k % 2 == 0 { "T-1" } else { "T-2" };
                let seat = if k % 2 == 0 { "p:heat" } else { "p:probe" };
                plan.push((OpKind::Merge, *state, seat, Some(task), Some(1)));
            }
            // Deliveries: two seats, two attempts of T-1, and one of T-2.
            let deliveries = [
                ("p:heat", "T-1", 1),
                ("p:heat", "T-1", 2),
                ("p:heat", "T-2", 1),
                ("p:probe", "T-1", 1),
            ];
            for (k, (seat, task, attempt)) in deliveries.into_iter().cycle().take(12).enumerate() {
                plan.push((
                    OpKind::Deliver,
                    states[k % states.len()],
                    seat,
                    Some(task),
                    Some(attempt),
                ));
            }
            // The noise of a store that has been running: doorbells and forges.
            for _ in 0..10 {
                plan.push((OpKind::Doorbell, OpState::Confirmed, "p:heat", None, None));
                plan.push((
                    OpKind::Forge,
                    OpState::Confirmed,
                    "p:heat",
                    Some("T-9"),
                    Some(1),
                ));
            }
            for (n, (kind, state, seat, task, attempt)) in plan.into_iter().enumerate() {
                let n = n as i64 + 1;
                let id = format!("O-{n}");
                tx.insert_op(&NewOp {
                    project: PROJECT.into(),
                    seat: seat.into(),
                    generation: 1,
                    kind,
                    task: task.map(str::to_string),
                    attempt,
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({"n": n, "pad": "x".repeat(200)}),
                    created: NOW + n,
                })?;
                tx.set_op_state(PROJECT, &id, state, None, NOW + n)?;
            }
            Ok(())
        })
        .unwrap();
    store
}

/// The results are the ones the callers got from the Rust filter.
#[test]
fn the_kind_queries_return_what_the_rust_filter_returned() {
    let (_dir, path) = temp_db("kind");
    let mut store = a_store_of_ops(&path);

    // The merge keys: what merge_keys built from every op of seven states.
    let keys = store
        .read(|tx| tx.op_keys_of_kind(PROJECT, OpKind::Merge))
        .unwrap();
    let mut old_keys = Vec::new();
    for state in [
        OpState::Intended,
        OpState::Executing,
        OpState::Submitted,
        OpState::Unknown,
        OpState::Confirmed,
        OpState::Failed,
        OpState::Void,
    ] {
        for op in store.read(|tx| tx.ops_in_state(PROJECT, state)).unwrap() {
            if op.kind == OpKind::Merge {
                if let (Some(task), Some(attempt)) = (op.task, op.attempt) {
                    old_keys.push((task, attempt));
                }
            }
        }
    }
    old_keys.sort();
    let mut keys = keys;
    keys.sort();
    assert_eq!(keys, old_keys, "the keys are the ones the filter built");
    assert_eq!(keys.len(), 7, "one merge op per state");

    // The merge ops in two states: what ready_merges built from those states.
    for states in [
        vec![OpState::Intended, OpState::Unknown],
        vec![OpState::Confirmed],
        vec![],
    ] {
        let got: Vec<String> = store
            .read(|tx| tx.ops_of_kind_in_states(PROJECT, OpKind::Merge, &states))
            .unwrap()
            .into_iter()
            .map(|op| op.id)
            .collect();
        let mut old: Vec<String> = Vec::new();
        let wanted = if states.is_empty() {
            vec![
                OpState::Intended,
                OpState::Executing,
                OpState::Submitted,
                OpState::Unknown,
                OpState::Confirmed,
                OpState::Failed,
                OpState::Void,
            ]
        } else {
            states.clone()
        };
        for state in wanted {
            for op in store.read(|tx| tx.ops_in_state(PROJECT, state)).unwrap() {
                if op.kind == OpKind::Merge {
                    old.push(op.id);
                }
            }
        }
        // Both read the ops oldest first.
        let mut old_sorted = old.clone();
        old_sorted.sort_by_key(|id| {
            id.split_once('-')
                .and_then(|(_, n)| n.parse::<i64>().ok())
                .unwrap_or(0)
        });
        assert_eq!(got, old_sorted, "the ops of {states:?} are the same");
    }

    // The deliveries of one attempt: what the lane's nudge filtered out of
    // every delivery of the seat.
    for (seat, task, attempt) in [
        ("p:heat", "T-1", 1),
        ("p:heat", "T-1", 2),
        ("p:heat", "T-2", 1),
        ("p:probe", "T-1", 1),
        ("p:probe", "T-9", 3),
    ] {
        let got: Vec<String> = store
            .read(|tx| tx.ops_of_seat_kind_task(seat, OpKind::Deliver, task, attempt))
            .unwrap()
            .into_iter()
            .map(|op| op.id)
            .collect();
        let old: Vec<String> = store
            .read(|tx| tx.ops_of_seat_kind(seat, OpKind::Deliver))
            .unwrap()
            .into_iter()
            .filter(|op| op.task.as_deref() == Some(task) && op.attempt == Some(attempt))
            .map(|op| op.id)
            .collect();
        assert_eq!(
            got, old,
            "{seat} {task}:{attempt} reads the same deliveries"
        );
    }
    // And the one attempt nobody delivered: no ops, not the seat's others.
    assert!(store
        .read(|tx| tx.ops_of_seat_kind_task("p:probe", OpKind::Deliver, "T-9", 3))
        .unwrap()
        .is_empty());
}

/// The plans: each read is answered by its index, and no plan scans the
/// operations table.
#[test]
fn the_kind_queries_use_the_new_indexes() {
    let (_dir, path) = temp_db("kind-plan");
    let _store = a_store_of_ops(&path);
    let merge = OpKind::Merge.as_str().to_string();
    let deliver = OpKind::Deliver.as_str().to_string();
    let intended = OpState::Intended.as_str().to_string();
    let unknown = OpState::Unknown.as_str().to_string();
    let seat = "p:heat".to_string();
    let task = "T-1".to_string();
    let attempt = 1_i64;
    let project = PROJECT.to_string();
    let cases: Vec<(&str, &str, Vec<&dyn rusqlite::ToSql>, &str)> = vec![
        (
            "ops_of_kind_in_states (all states)",
            "SELECT project, id FROM operations WHERE project=?1 AND kind=?2 ORDER BY created, id",
            vec![&project, &merge],
            "operations_kind",
        ),
        (
            "ops_of_kind_in_states (two states)",
            "SELECT project, id FROM operations WHERE project=?1 AND kind=?2 \
             AND state IN (?3, ?4) ORDER BY created, id",
            vec![&project, &merge, &intended, &unknown],
            "operations_kind",
        ),
        (
            "op_keys_of_kind",
            "SELECT task, attempt FROM operations \
             WHERE project=?1 AND kind=?2 AND task IS NOT NULL AND attempt IS NOT NULL \
             ORDER BY created, id",
            vec![&project, &merge],
            "operations_kind",
        ),
        (
            "ops_of_seat_kind_task",
            "SELECT project, id FROM operations \
             WHERE seat=?1 AND kind=?2 AND task=?3 AND attempt=?4 \
             ORDER BY created, CAST(SUBSTR(id, INSTR(id, '-')+1) AS INTEGER)",
            vec![&seat, &deliver, &task, &attempt],
            "operations_attempt",
        ),
    ];
    for (read, sql, params, index) in cases {
        let text = plan_of(&path, sql, &params).join(" | ");
        assert!(
            text.contains("SEARCH operations USING"),
            "{read} is not answered by an index: {text}"
        );
        assert!(
            !text.contains("SCAN operations"),
            "{read} scans the operations table: {text}"
        );
        assert!(text.contains(index), "{read} does not use {index}: {text}");
    }
}

/// A store made before v9 gains both indexes when it is opened, and is stamped
/// 9 — no hand step, no row touched.
#[test]
fn a_version_8_store_gains_the_kind_indexes_when_it_is_opened() {
    let (_dir, path) = temp_db("v8");
    drop(a_store_of_ops(&path));
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP INDEX operations_kind;
         DROP INDEX operations_attempt;
         UPDATE meta SET value='8' WHERE key='schema_version';",
    )
    .unwrap();
    conn.close().unwrap();

    // Before: the merge read walks the operations table.
    let before = plan_of(
        &path,
        "SELECT project, id FROM operations WHERE project=?1 AND kind=?2 ORDER BY created, id",
        &[&PROJECT, &OpKind::Merge.as_str()],
    )
    .join(" | ");
    assert!(
        !before.contains("operations_kind"),
        "the fixture has no index yet: {before}"
    );

    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store.read(|tx| tx.get_meta("schema_version")).unwrap(),
        Some(SCHEMA_VERSION.to_string()),
        "the store was carried up to v9"
    );
    assert_eq!(SCHEMA_VERSION, 9, "this binary writes v9");
    // The rows are untouched: the fixture's seven merge ops are still there.
    let keys = store
        .read(|tx| tx.op_keys_of_kind(PROJECT, OpKind::Merge))
        .unwrap();
    assert_eq!(keys.len(), 7, "the migration only creates indexes");

    let merge = OpKind::Merge.as_str().to_string();
    let deliver = OpKind::Deliver.as_str().to_string();
    let seat = "p:heat".to_string();
    let task = "T-1".to_string();
    let attempt = 1_i64;
    let project = PROJECT.to_string();
    for (sql, params, index) in [
        (
            "SELECT project, id FROM operations WHERE project=?1 AND kind=?2 ORDER BY created, id",
            vec![&project as &dyn rusqlite::ToSql, &merge],
            "operations_kind",
        ),
        (
            "SELECT project, id FROM operations WHERE seat=?1 AND kind=?2 AND task=?3 \
             AND attempt=?4",
            vec![&seat as &dyn rusqlite::ToSql, &deliver, &task, &attempt],
            "operations_attempt",
        ),
    ] {
        let text = plan_of(&path, sql, &params).join(" | ");
        assert!(
            text.contains(index) && !text.contains("SCAN operations"),
            "after the migration the read does not use {index}: {text}"
        );
    }
}
