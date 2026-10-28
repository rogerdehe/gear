//! The store's schema version and the steps that carry an old store up (T-41).
//!
//! The store stamps its version in `meta.schema_version` when it is created.
//! A binary that opens a store of an older version migrates it up: one step
//! per transaction, the version moving with the step, so a step that fails
//! leaves the store exactly where it was. A store of a newer version is
//! refused for writing (`SCHEMA_TOO_NEW`) — the schema only ever migrates up,
//! never down.
//!
//! **v1** is the schema of v0.1.0. A store stamped 1 has the v1 tables, but
//! not necessarily in the v1 shape: a development build that opened the store
//! earlier created a table in an older shape (the `voyages` of T-22 attempt 1,
//! the `seats.slot` of the skeleton) and the stamp never told the difference.
//! **v2** is the canonical shape: a store stamped 2 was normalized by a binary
//! that rebuilt every drifted table and kept its rows.
//! **v3** added the `push_destinations` of T-73 (the places a repo of the
//! project may be pushed to); **v5** drops it again — the push allow-list is
//! gone (T-108), and the step only drops the table.
//! **v4** adds `tasks.issue` (the GitHub issue a task came from, T-79).
//! **v5** drops `push_destinations` (T-108: no push allow-list).
//! **v6** is the M4 schema (§1 of gear/voyages/M4/design.md): the session of an
//! attempt, the repo/base/gate a task keeps, the rounds and their tokens, the
//! stretches a seat could not work, the friction reports and the captain's
//! decision log. The migration leaves every new column NULL: the store
//! cannot read the project config, so a reader of `tasks.repo/base/gate`
//! that finds NULL falls back to the lane's current config.
//! **v7** adds `contracts.mechanical` (T-272): the reason a task is mechanical —
//! the merge gate alone proves the change, so the review is skipped. NULL means
//! reviewed as before, and the migration leaves every existing contract NULL.

use gear_model::{ErrorCode, GearError, GearResult};
use rusqlite::{params, Transaction};

use crate::{map_sqlite, SCHEMA_SQL};

/// The version this binary writes.
pub const SCHEMA_VERSION: i64 = 7;

/// The oldest version the steps know. A store without a stamp is this old:
/// nothing ever wrote a version before v1.
pub const OLDEST_VERSION: i64 = 1;

/// One step of the way up: from `from` to `from + 1`.
pub struct Step {
    /// The version the step starts at.
    pub from: i64,
    /// What the step does, inside the caller's transaction.
    pub apply: fn(&Transaction<'_>) -> GearResult<()>,
}

/// The steps, in order. Every step carries the store from `from` to
/// `from + 1`; a store older than the first step starts there.
pub const STEPS: &[Step] = &[
    Step { from: 1, apply: step_1_to_2 },
    Step { from: 2, apply: step_2_to_3 },
    Step { from: 3, apply: step_3_to_4 },
    Step { from: 4, apply: step_4_to_5 },
    Step { from: 5, apply: step_5_to_6 },
    Step { from: 6, apply: step_6_to_7 },
];

/// The columns of the `voyages` table, in the order of the canonical shape.
const VOYAGE_COLUMNS: [&str; 10] = [
    "project",
    "milestone",
    "title",
    "done",
    "bar",
    "eta",
    "departure",
    "departure_tasks",
    "departure_estimate_min",
    "updated",
];

/// v1 → v2: the canonical shape.
///
/// The v1 tables are created where they are missing (every statement of the
/// schema is `IF NOT EXISTS`, so a table that is there keeps its rows), the
/// shapes that `CREATE TABLE IF NOT EXISTS` cannot fix are repaired, and the
/// table v2 dropped is dropped.
fn step_1_to_2(tx: &Transaction<'_>) -> GearResult<()> {
    // The tables a v1 store may not have yet (a store written before T-2,
    // T-22 or T-23). A table that is there is left alone.
    tx.execute_batch(SCHEMA_SQL).map_err(map_sqlite)?;

    // The `voyages` of T-22 attempt 1 stored `done`, `bar` and `eta` as NOT
    // NULL; v2 lets them be NULL until a plan describes the voyage. SQLite
    // cannot relax a constraint in place, so the table is rebuilt and its
    // rows are carried over.
    if column_is_not_null(tx, "voyages", "done")? {
        rebuild_voyages(tx)?;
    }

    // `seats.slot` became `seats.worktree`.
    if column_exists(tx, "seats", "slot")? && !column_exists(tx, "seats", "worktree")? {
        tx.execute_batch("ALTER TABLE seats RENAME COLUMN slot TO worktree")
            .map_err(map_sqlite)?;
    }

    // `resources` learned its branch and the workspace it belongs to.
    for (column, decl) in [
        ("branch", "TEXT NOT NULL DEFAULT ''"),
        ("workspace_id", "TEXT"),
    ] {
        if !column_exists(tx, "resources", column)? {
            tx.execute_batch(&format!("ALTER TABLE resources ADD COLUMN {column} {decl}"))
                .map_err(map_sqlite)?;
        }
    }

    // Time accounting (T-38/T-39): existing tables need their nullable columns
    // added within the same versioned transaction as the rest of v1 -> v2.
    for (table, column) in [
        ("attempts", "delivered_at"),
        ("attempts", "started_at"),
        ("attempts", "handed_in_at"),
        ("attempts", "verdict_at"),
        ("attempts", "settled_at"),
        ("attempts", "merged_at"),
        ("tasks", "estimate_min"),
        ("voyages", "departure_estimate_min"),
    ] {
        if !column_exists(tx, table, column)? {
            tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} INTEGER"))
                .map_err(map_sqlite)?;
        }
    }

    // The `slots` table is gone: its place is `seats.worktree` and
    // `resources`. Nothing has read it since the skeleton.
    tx.execute_batch("DROP TABLE IF EXISTS slots").map_err(map_sqlite)?;

    Ok(())
}

/// v2 → v3: the push destinations of T-73.
///
/// A table of its own: the step creates it and keeps every row of the store
/// as it is. The schema is applied as a whole (every statement of it is
/// `IF NOT EXISTS`), so a store that already has the table keeps it.
fn step_2_to_3(tx: &Transaction<'_>) -> GearResult<()> {
    tx.execute_batch(SCHEMA_SQL).map_err(map_sqlite)?;
    Ok(())
}

/// v4 → v5: the push allow-list is gone (T-108). The destinations table is
/// dropped; nothing else changes, and a store without the table is unchanged.
fn step_4_to_5(tx: &Transaction<'_>) -> GearResult<()> {
    tx.execute_batch("DROP TABLE IF EXISTS push_destinations")
        .map_err(map_sqlite)?;
    Ok(())
}

/// v3 → v4: `tasks.issue` (T-79). A nullable column, so an older binary keeps
/// working on the store.
fn step_3_to_4(tx: &Transaction<'_>) -> GearResult<()> {
    if !column_exists(tx, "tasks", "issue")? {
        tx.execute_batch("ALTER TABLE tasks ADD COLUMN issue INTEGER")
            .map_err(map_sqlite)?;
    }
    Ok(())
}

/// v5 → v6: the M4 schema (§1 of gear/voyages/M4/design.md).
///
/// The new tables (`rounds`, `seat_pauses`, `frictions`, `decisions`) are
/// created by the schema itself (every statement of it is `IF NOT EXISTS`).
/// The new columns of the old tables are added here, each nullable: a store
/// of v5 keeps every row it has, and the new columns are NULL until a writer
/// fills them (the store cannot read the project config, so `tasks.repo`,
/// `tasks.base` and `tasks.gate` stay NULL for the old tasks and their
/// readers fall back to the lane's current config).
fn step_5_to_6(tx: &Transaction<'_>) -> GearResult<()> {
    tx.execute_batch(SCHEMA_SQL).map_err(map_sqlite)?;

    for (table, column, decl) in [
        ("attempts", "session_ref", "TEXT"),
        ("attempts", "session_cli", "TEXT"),
        ("attempts", "session_action", "TEXT"),
        ("attempts", "sent_at", "INTEGER"),
        ("tasks", "repo", "TEXT"),
        ("tasks", "base", "TEXT"),
        ("tasks", "gate", "TEXT"),
        ("tasks", "rework_estimate_min", "INTEGER"),
        ("messages", "meta", "TEXT"),
    ] {
        if !column_exists(tx, table, column)? {
            tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))
                .map_err(map_sqlite)?;
        }
    }

    // The `seat_pauses` key carries the kind (§5.4): a quota park and a
    // declared wait of one seat are independent and may overlap. A store that
    // a first attempt of this task migrated has the older key (without the
    // kind); it is rebuilt with its rows.
    if table_exists(tx, "seat_pauses")? {
        let key = primary_key_of(tx, "seat_pauses")?;
        if key != ["project", "seat", "kind", "start_at"] {
            rebuild_seat_pauses(tx)?;
        }
    }
    Ok(())
}

/// v6 → v7: `contracts.mechanical` (T-272), the reason a task is mechanical.
///
/// One nullable column, added here when it is missing: a v6 store keeps every
/// row it has, and every existing contract is NULL — reviewed as before.
fn step_6_to_7(tx: &Transaction<'_>) -> GearResult<()> {
    if !column_exists(tx, "contracts", "mechanical")? {
        tx.execute_batch("ALTER TABLE contracts ADD COLUMN mechanical TEXT")
            .map_err(map_sqlite)?;
    }
    Ok(())
}

/// Rebuild the `seat_pauses` table with the key of §5.4, carrying the rows
/// over.
fn rebuild_seat_pauses(tx: &Transaction<'_>) -> GearResult<()> {
    tx.execute_batch(
        "CREATE TABLE seat_pauses_v6 (project TEXT NOT NULL, seat TEXT NOT NULL,
           kind TEXT NOT NULL, reason TEXT,
           start_at INTEGER NOT NULL, end_at INTEGER,
           PRIMARY KEY (project, seat, kind, start_at));
         INSERT OR IGNORE INTO seat_pauses_v6 (project, seat, kind, reason, start_at, end_at)
           SELECT project, seat, kind, reason, start_at, end_at FROM seat_pauses;
         DROP TABLE seat_pauses;
         ALTER TABLE seat_pauses_v6 RENAME TO seat_pauses;",
    )
    .map_err(map_sqlite)?;
    Ok(())
}

/// Rebuild the `voyages` table in the canonical shape, carrying the rows over.
fn rebuild_voyages(tx: &Transaction<'_>) -> GearResult<()> {
    for key in ["project", "milestone"] {
        if !column_exists(tx, "voyages", key)? {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!("the voyages table has no {key} column; it cannot be migrated"),
            ));
        }
    }
    // A column the old table never had stays empty; the rest are carried over.
    let mut select = Vec::with_capacity(VOYAGE_COLUMNS.len());
    for column in VOYAGE_COLUMNS {
        if column_exists(tx, "voyages", column)? {
            select.push(column.to_string());
        } else if column == "title" {
            select.push("''".to_string());
        } else if column == "updated" {
            select.push("0".to_string());
        } else {
            select.push("NULL".to_string());
        }
    }
    let sql = format!(
        "CREATE TABLE voyages_v2 (project TEXT NOT NULL, milestone TEXT NOT NULL,
           title TEXT NOT NULL DEFAULT '',
           done TEXT, bar TEXT, eta TEXT,
           departure INTEGER, departure_tasks INTEGER, departure_estimate_min INTEGER, updated INTEGER NOT NULL,
           PRIMARY KEY (project, milestone));
         INSERT INTO voyages_v2 (project, milestone, title, done, bar, eta,
                                 departure, departure_tasks, departure_estimate_min, updated)
           SELECT {select} FROM voyages;
         DROP TABLE voyages;
         ALTER TABLE voyages_v2 RENAME TO voyages;",
        select = select.join(", ")
    );
    tx.execute_batch(&sql).map_err(map_sqlite)?;
    Ok(())
}

/// Does the table exist?
pub(crate) fn table_exists(tx: &Transaction<'_>, table: &str) -> GearResult<bool> {
    let n: i64 = tx
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    Ok(n > 0)
}

/// Does the table have the column?
fn column_exists(tx: &Transaction<'_>, table: &str, column: &str) -> GearResult<bool> {
    if !table_exists(tx, table)? {
        return Ok(false);
    }
    for (name, _) in columns_of(tx, table)? {
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Is the column NOT NULL?
fn column_is_not_null(tx: &Transaction<'_>, table: &str, column: &str) -> GearResult<bool> {
    if !table_exists(tx, table)? {
        return Ok(false);
    }
    for (name, not_null) in columns_of(tx, table)? {
        if name == column {
            return Ok(not_null);
        }
    }
    Ok(false)
}

/// The primary key of a table: the columns of it, in the order of the key.
/// Empty for a table without one.
fn primary_key_of(tx: &Transaction<'_>, table: &str) -> GearResult<Vec<String>> {
    if !table_exists(tx, table)? {
        return Ok(Vec::new());
    }
    let mut stmt = tx
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(map_sqlite)?;
    let mut rows = stmt.query([]).map_err(map_sqlite)?;
    let mut keyed: Vec<(i64, String)> = Vec::new();
    while let Some(row) = rows.next().map_err(map_sqlite)? {
        let name: String = row.get(1).map_err(map_sqlite)?;
        let position: i64 = row.get(5).map_err(map_sqlite)?;
        if position > 0 {
            keyed.push((position, name));
        }
    }
    keyed.sort_by_key(|(position, _)| *position);
    Ok(keyed.into_iter().map(|(_, name)| name).collect())
}

/// The columns of a table: `(name, NOT NULL)` in the order of the table.
///
/// The table name is written into the pragma; it comes from this crate's own
/// code, never from a caller.
fn columns_of(tx: &Transaction<'_>, table: &str) -> GearResult<Vec<(String, bool)>> {
    let mut stmt = tx
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(map_sqlite)?;
    let mut rows = stmt.query([]).map_err(map_sqlite)?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().map_err(map_sqlite)? {
        let name: String = row.get(1).map_err(map_sqlite)?;
        let not_null: i64 = row.get(3).map_err(map_sqlite)?;
        columns.push((name, not_null != 0));
    }
    Ok(columns)
}
