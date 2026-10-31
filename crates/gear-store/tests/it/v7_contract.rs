//! T-272: schema v7 — `contracts.mechanical`, the reason a task is mechanical.
//!
//! A v6 store migrates to v7 with every existing contract NULL (reviewed as
//! before), and a contract that carries a reason round-trips.

use gear_model::{GearResult, TaskState};
use gear_store::{ContractRow, Store, TaskRow, SCHEMA_VERSION};
use rusqlite::Connection;

const PROJECT: &str = "p";
const NOW: i64 = 1_800_000_000;

fn a_task(tx: &gear_store::Tx<'_>, id: &str) -> GearResult<()> {
    tx.insert_task(&TaskRow {
        project: PROJECT.into(),
        id: id.into(),
        rank: 1,
        state: TaskState::Ready,
        priority: "must".into(),
        lane: Some("hull".into()),
        milestone: Some("M5".into()),
        required: true,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(30),
        issue: None,
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        updated: NOW,
    })
}

fn a_contract(id: &str, mechanical: Option<&str>) -> ContractRow {
    ContractRow {
        project: PROJECT.into(),
        task: id.into(),
        version: 1,
        goal: "g".into(),
        accept: "a".into(),
        evidence_files: vec![],
        evidence_due_min: None,
        body: "b".into(),
        created: NOW,
        mechanical: mechanical.map(str::to_string),
    }
}

/// A store of v6: the v7 store without `contracts.mechanical`.
fn a_store_of_v6(path: &std::path::Path) {
    {
        let mut store = Store::open(path).unwrap();
        store
            .write(|tx| {
                a_task(tx, "T-1")?;
                a_task(tx, "T-2")?;
                tx.insert_contract(&a_contract("T-1", None))?;
                tx.insert_contract(&a_contract("T-2", None))
            })
            .unwrap();
    }
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "ALTER TABLE contracts DROP COLUMN mechanical;
         UPDATE meta SET value='6' WHERE key='schema_version';",
    )
    .unwrap();
}

#[test]
fn a_v6_store_migrates_to_v7_with_existing_contracts_null() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    a_store_of_v6(&path);

    // The store is of v6 before it is opened.
    let conn = Connection::open(&path).unwrap();
    let version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "6");
    drop(conn);

    // Opening it carries it up to v7.
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .read(|tx| tx.get_meta("schema_version"))
            .unwrap()
            .as_deref(),
        Some(SCHEMA_VERSION.to_string().as_str())
    );
    assert_eq!(SCHEMA_VERSION, 7);

    // The rows are still there, and the new column is NULL: the migration
    // does not invent a reason.
    for id in ["T-1", "T-2"] {
        let contract = store
            .read(|tx| tx.get_contract(PROJECT, id, 1))
            .unwrap()
            .unwrap_or_else(|| panic!("{id} has no contract v1"));
        assert_eq!(contract.goal, "g");
        assert_eq!(contract.mechanical, None, "{id}");
    }
}

#[test]
fn a_mechanical_contract_round_trips() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            a_task(tx, "T-1")?;
            a_task(tx, "T-2")?;
            tx.insert_contract(&a_contract("T-1", Some("delete three dead fns")))?;
            tx.insert_contract(&a_contract("T-2", None))
        })
        .unwrap();

    // The reason is read back as it was written; a contract without one stays
    // NULL.
    let mechanical = store
        .read(|tx| tx.get_contract(PROJECT, "T-1", 1))
        .unwrap()
        .unwrap();
    assert_eq!(
        mechanical.mechanical.as_deref(),
        Some("delete three dead fns")
    );
    let plain = store
        .read(|tx| tx.get_contract(PROJECT, "T-2", 1))
        .unwrap()
        .unwrap();
    assert_eq!(plain.mechanical, None);

    // The whole row round-trips: the reason is one column, not a new row.
    assert_eq!(mechanical, a_contract("T-1", Some("delete three dead fns")));
}
