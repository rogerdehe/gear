//! T-529: a tick reads the live work, not the whole history (schema v10).
//!
//! The dispatch, the rounds bookkeeping and the board's idle view read every
//! task of every voyage, every round and every attempt a project ever ran, on
//! every tick, and kept the live ones. On our store that was 528 tasks of six
//! voyages, five of them landed. v10 puts the filter in SQL: a task is live
//! when it is not terminal or when its voyage still has a task that is not,
//! and a round is live while it has no end.

use std::path::{Path, PathBuf};

use gear_model::{TaskState, Ts};
use gear_store::{NewRound, RoundKey, Store, TaskRow, LIVE_TASKS_SQL, SCHEMA_VERSION};
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

fn task(id: &str, rank: i64, state: TaskState, milestone: Option<&str>) -> TaskRow {
    TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: PROJECT.into(),
        id: id.into(),
        rank,
        state,
        priority: "must".into(),
        lane: Some("heat".into()),
        milestone: milestone.map(str::to_string),
        required: false,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(30),
        issue: None,
        updated: NOW,
    }
}

/// A store of six voyages: five landed (every task Done) and one open (three
/// done, one ready), plus rounds: two open and three ended.
fn a_project_of_six_voyages(path: &Path) -> Store {
    let mut store = Store::open(path).unwrap();
    store
        .write(|tx| {
            gear_store::testing::seed_project(tx, PROJECT, "sess", NOW)?;
            gear_store::testing::seed_active_seat(
                tx,
                PROJECT,
                "heat",
                gear_model::SeatRole::Lane,
                "w1:p2",
                "opencode",
                NOW,
            )?;
            let mut rank = 0;
            // Five landed voyages: forty tasks each, all done.
            for voyage in 0..5 {
                for n in 0..40 {
                    rank += 1;
                    tx.insert_task(&task(
                        &format!("T-{voyage}0{n:02}"),
                        rank,
                        TaskState::Done,
                        Some(&format!("M{voice}", voice = voyage)),
                    ))?
                }
            }
            // The open voyage: three done, one ready.
            for (n, state) in [
                TaskState::Done,
                TaskState::Done,
                TaskState::Done,
                TaskState::Ready,
            ]
            .into_iter()
            .enumerate()
            {
                rank += 1;
                tx.insert_task(&task(&format!("T-9{n}"), rank, state, Some("M9")))?;
            }
            // A task that is not in a voyage: it is live on its own.
            rank += 1;
            tx.insert_task(&task("T-100", rank, TaskState::Running, None))?;
            // The rounds: two open, three ended.
            for (n, end) in [
                None,
                None,
                Some(NOW - 100),
                Some(NOW - 200),
                Some(NOW - 300),
            ]
            .into_iter()
            .enumerate()
            {
                let key = RoundKey {
                    project: PROJECT.into(),
                    task: format!("T-{n}"),
                    attempt: 1,
                    role: "dev".into(),
                };
                tx.insert_round(&NewRound {
                    key: key.clone(),
                    seat: format!("{PROJECT}:heat"),
                    generation: 1,
                    cli: "opencode".into(),
                    model: None,
                    start_at: NOW - 400 + n as Ts,
                })?;
                if let Some(end) = end {
                    tx.close_round(&key, end)?;
                    // One ended round keeps an unknown usage: it is still
                    // being counted; the others found theirs.
                    if n == 2 {
                        tx.set_round_tokens(
                            &key,
                            &gear_store::RoundTokens {
                                input: Some(10),
                                output: Some(1),
                                cache_read: None,
                                cache_write: None,
                                reasoning: None,
                                cost_usd: None,
                                cost_source: "none".into(),
                            },
                            end,
                        )?;
                    }
                }
            }
            Ok(())
        })
        .unwrap();
    store
}

/// The live tasks: the open voyage's four, the task that is in no voyage, and
/// nothing of the five landed voyages.
#[test]
fn the_live_tasks_are_the_open_work_and_no_finished_voyage() {
    let (_dir, path) = temp_db("live");
    let mut store = a_project_of_six_voyages(&path);
    let all = store.read(|tx| tx.tasks_of_project(PROJECT)).unwrap().len();
    let live: Vec<String> = store
        .read(|tx| tx.tasks_live_of_project(PROJECT))
        .unwrap()
        .into_iter()
        .map(|task| task.id)
        .collect();
    assert_eq!(all, 205, "the seeded store's history");
    assert_eq!(
        live,
        vec!["T-90", "T-91", "T-92", "T-93", "T-100"],
        "the open voyage, the task in no voyage, and no landed voyage"
    );
}

/// A finished voyage's tasks are read only where a report asks for them: the
/// hint of a voyage whose tasks are all done takes no row.
#[test]
fn a_landed_voyage_leaves_no_live_task() {
    let (_dir, path) = temp_db("landed");
    let mut store = a_project_of_six_voyages(&path);
    let live = store.read(|tx| tx.tasks_live_of_project(PROJECT)).unwrap();
    assert!(
        live.iter()
            .all(|task| task.milestone.as_deref() != Some("M0")),
        "a voyage whose tasks are all done is history"
    );
    // The voyage is still readable as history, in full and in rank order.
    let all = store.read(|tx| tx.tasks_of_project(PROJECT)).unwrap();
    assert_eq!(
        all.iter()
            .filter(|t| t.milestone.as_deref() == Some("M0"))
            .count(),
        40
    );
}

/// The rounds: the live ones and the ended ones whose usage is still counted.
#[test]
fn the_open_rounds_and_the_rounds_still_being_counted() {
    let (_dir, path) = temp_db("rounds");
    let mut store = a_project_of_six_voyages(&path);
    let open: Vec<i64> = store
        .read(|tx| tx.open_rounds_of_project(PROJECT))
        .unwrap()
        .into_iter()
        .map(|round| round.start_at)
        .collect();
    assert_eq!(open, vec![NOW - 400, NOW - 399], "the two open rounds");
    let counting: Vec<i64> = store
        .read(|tx| tx.rounds_to_count(PROJECT))
        .unwrap()
        .into_iter()
        .map(|round| round.start_at)
        .collect();
    assert_eq!(
        counting,
        vec![NOW - 397, NOW - 396],
        "the two ended rounds without a found usage, not the one that found one"
    );
}

/// The plans: the live-task read and the open-round read are answered by the
/// v10 indexes, not by a scan of the table.
#[test]
fn the_live_reads_use_the_new_indexes() {
    let (_dir, path) = temp_db("plan");
    let _store = a_project_of_six_voyages(&path);
    let project = PROJECT.to_string();
    let cases: Vec<(&str, &str, &[&str])> = vec![
        (
            "tasks not terminal",
            "SELECT project, id FROM tasks t WHERE t.project=?1 AND t.state NOT IN ('done', 'failed', 'cancelled')",
            // SQLite picks either index whose first column is the project;
            // both are the v10 ones.
            &["tasks_state", "tasks_milestone"],
        ),
        (
            "the voyage's open tasks",
            "SELECT 1 FROM tasks o WHERE o.project=?1 AND o.milestone=?2 \
             AND o.state NOT IN ('done', 'failed', 'cancelled')",
            &["tasks_milestone"],
        ),
        (
            "rounds not closed",
            "SELECT project, task FROM rounds WHERE project=?1 AND end_at IS NULL",
            &["rounds_open"],
        ),
    ];
    for (read, sql, indexes) in cases {
        let milestone = "M9".to_string();
        let params: Vec<&dyn rusqlite::ToSql> = if sql.contains("milestone") {
            vec![&project, &milestone]
        } else {
            vec![&project]
        };
        let text = plan_of(&path, sql, &params).join(" | ");
        assert!(
            text.contains("USING INDEX") || text.contains("USING COVERING INDEX"),
            "{read} is not answered by an index: {text}"
        );
        assert!(
            !text.contains("SCAN tasks") && !text.contains("SCAN rounds"),
            "{read} scans its table: {text}"
        );
        assert!(
            indexes.iter().any(|index| text.contains(index)),
            "{read} does not use one of {indexes:?}: {text}"
        );
    }
}

/// The live-task query itself, as the engine asks it (F1 of the review of
/// attempt 1): two index-served reads — the open tasks from `tasks_state`, the
/// terminal ones of a voyage still open from `tasks_milestone` — and no scan of
/// the project's tasks. The plan at v10 is
/// `MERGE (UNION ALL) · SEARCH tasks USING INDEX tasks_state (project=? AND state=?)
/// · SEARCH tasks USING COVERING INDEX tasks_milestone (project=? AND milestone>?)
/// · SEARCH t USING INDEX tasks_milestone (project=? AND milestone=? AND state=?)`.
#[test]
fn the_live_task_query_reads_both_halves_by_index() {
    let (_dir, path) = temp_db("live-plan");
    let _store = a_project_of_six_voyages(&path);
    // The store's own query text, not a copy of it.
    let text = plan_of(&path, LIVE_TASKS_SQL, &[&PROJECT]).join(" | ");
    assert!(
        !text.contains("SCAN tasks"),
        "the live-task read scans the project: {text}"
    );
    assert!(
        !text.contains("sqlite_autoindex_tasks_1"),
        "the live-task read walks the project's primary key: {text}"
    );
    assert!(
        text.contains("tasks_state") && text.contains("tasks_milestone"),
        "the two halves are not both index-served: {text}"
    );
}

/// A version-9 store opened by this code gets the v10 indexes and is stamped
/// 10 — no row touched.
#[test]
fn a_version_9_store_is_carried_up_to_10() {
    let (_dir, path) = temp_db("v9");
    drop(a_project_of_six_voyages(&path));
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP INDEX tasks_state;
         DROP INDEX tasks_milestone;
         DROP INDEX rounds_open;
         UPDATE meta SET value='9' WHERE key='schema_version';",
    )
    .unwrap();
    conn.close().unwrap();
    let before = plan_of(
        &path,
        "SELECT project, task FROM rounds WHERE project=?1 AND end_at IS NULL",
        &[&PROJECT],
    )
    .join(" | ");
    assert!(
        !before.contains("rounds_open"),
        "the fixture has no v10 index yet: {before}"
    );

    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .read(|tx| tx.get_meta("schema_version"))
            .unwrap()
            .as_deref(),
        Some(SCHEMA_VERSION.to_string().as_str())
    );
    assert_eq!(SCHEMA_VERSION, 11, "this binary writes v11");
    // The rows are untouched: the live read still finds the open work.
    assert_eq!(
        store
            .read(|tx| tx.tasks_live_of_project(PROJECT))
            .unwrap()
            .len(),
        5
    );
    let after = plan_of(
        &path,
        "SELECT project, task FROM rounds WHERE project=?1 AND end_at IS NULL",
        &[&PROJECT],
    )
    .join(" | ");
    assert!(
        after.contains("rounds_open") && !after.contains("SCAN rounds"),
        "after the migration the read uses rounds_open: {after}"
    );
}
