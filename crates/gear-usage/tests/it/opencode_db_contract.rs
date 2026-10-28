//! T-137: OpenCode's database (`opencode-db`, §4.3). The rows of the
//! `session_message` table carry their tokens and their time in a JSON
//! `data`.

use gear_usage::{last_context_in, usage_in, UsageLog};
use serde_json::json;

use crate::shared::{T12, T13, T14};

const SESSION: &str = "ses_0199aaaa";

/// The database of OpenCode, with the messages of a session. The table needs
/// no column of its own for the time: it is in the JSON of the message
/// (§4.3).
fn database(home: &std::path::Path, rows: &[(i64, serde_json::Value)]) {
    let dir = home.join(".local").join("share").join("opencode");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("opencode.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session_message (id TEXT PRIMARY KEY, sessionID TEXT NOT NULL,
           data TEXT NOT NULL);",
    )
    .unwrap();
    for (n, (_, data)) in rows.iter().enumerate() {
        conn.execute(
            "INSERT INTO session_message (id, sessionID, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![format!("msg_{n}"), SESSION, data.to_string()],
        )
        .unwrap();
    }
}

/// One message of OpenCode, with its tokens, its cost and the time it was
/// completed at.
fn message(
    input: i64,
    output: i64,
    reasoning: i64,
    cache_read: i64,
    cache_write: i64,
    cost: f64,
    completed: i64,
) -> serde_json::Value {
    json!({
        "role": "assistant",
        "tokens": {
            "input": input,
            "output": output,
            "reasoning": reasoning,
            "cache": {"read": cache_read, "write": cache_write}
        },
        "cost": cost,
        "time": {"created": completed, "completed": completed}
    })
}

#[test]
fn the_messages_of_the_window_are_summed() {
    let home = tempfile::tempdir().unwrap();
    database(
        home.path(),
        &[
            (T12, message(10, 1, 2, 100, 5, 0.01, T12 * 1000)),
            (T13, message(20, 2, 3, 200, 6, 0.02, T13 * 1000)),
            (T14, message(30, 3, 4, 300, 7, 0.03, T14 * 1000)),
        ],
    );
    let usage = usage_in(UsageLog::OpencodeDb, SESSION, T12, T14, home.path()).unwrap();
    assert_eq!(usage.records, 2, "{usage:?}");
    assert_eq!(usage.input, 30, "{usage:?}");
    assert_eq!(usage.output, 3, "{usage:?}");
    assert_eq!(usage.reasoning, 5, "{usage:?}");
    assert_eq!(usage.cache_read, 300, "{usage:?}");
    assert_eq!(usage.cache_write, 11, "{usage:?}");
    let cost = usage.cost_usd.unwrap();
    assert!((cost - 0.03).abs() < 1e-9, "{cost}");
}

#[test]
fn the_window_cuts_the_records_at_its_edges() {
    let home = tempfile::tempdir().unwrap();
    database(
        home.path(),
        &[
            (T12, message(10, 1, 2, 100, 5, 0.01, T12 * 1000)),
            (T13, message(20, 2, 3, 200, 6, 0.02, T13 * 1000)),
            (T14, message(30, 3, 4, 300, 7, 0.03, T14 * 1000)),
        ],
    );
    let usage = usage_in(UsageLog::OpencodeDb, SESSION, T13, T14, home.path()).unwrap();
    assert_eq!(usage.records, 1, "{usage:?}");
    assert_eq!(usage.input, 20, "{usage:?}");
}

#[test]
fn a_database_that_cannot_be_read_is_unknown() {
    let home = tempfile::tempdir().unwrap();
    let usage = usage_in(UsageLog::OpencodeDb, SESSION, T12, T14, home.path());
    assert!(
        usage.is_none(),
        "a database that is not there is unknown, not zero"
    );
}

#[test]
fn the_context_is_the_message_that_was_completed_last() {
    let home = tempfile::tempdir().unwrap();
    // The rows of the table are not in the order of their completion.
    database(
        home.path(),
        &[
            (T13, message(20, 2, 3, 200, 6, 0.02, T13 * 1000)),
            (T12, message(10, 1, 2, 100, 5, 0.01, T12 * 1000)),
        ],
    );
    // input + cache.read + cache.write of the last message (§3.5).
    let context = last_context_in(UsageLog::OpencodeDb, SESSION, home.path()).unwrap();
    assert_eq!(context, 20 + 200 + 6);
}
