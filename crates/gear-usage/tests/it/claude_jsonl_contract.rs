//! T-137: Claude's session file (`claude-jsonl`, §4.3). One message is
//! written on several lines, so it is counted once per `message.id`.

use gear_usage::{last_context_in, usage_in, UsageLog};
use serde_json::json;

use crate::shared::{iso, write_lines, T12, T13, T14};

const SESSION: &str = "0199aaaa-bbbb-cccc-dddd-eeeeffff0000";

/// One assistant message of Claude, with its usage.
fn assistant(
    id: &str,
    time: i64,
    input: i64,
    cache_read: i64,
    cache_write: i64,
    output: i64,
) -> serde_json::Value {
    json!({
        "type": "assistant",
        "uuid": format!("uuid-{id}"),
        "timestamp": iso(time),
        "message": {
            "id": id,
            "role": "assistant",
            "model": "claude-opus-5",
            "usage": {
                "input_tokens": input,
                "cache_read_input_tokens": cache_read,
                "cache_creation_input_tokens": cache_write,
                "output_tokens": output
            }
        }
    })
}

/// The session file of a Claude session, under a project of `~/.claude/projects`.
fn session(home: &std::path::Path, lines: &[serde_json::Value]) -> String {
    let path = write_lines(
        &home
            .join(".claude")
            .join("projects")
            .join("home-heat")
            .join(format!("{SESSION}.jsonl")),
        lines,
    );
    path.to_string_lossy().into_owned()
}

#[test]
fn a_message_written_on_three_lines_counts_once() {
    let home = tempfile::tempdir().unwrap();
    let _ = session(
        home.path(),
        &[
            assistant("msg_1", T12, 10, 100, 20, 5),
            assistant("msg_1", T12, 10, 100, 20, 5),
            assistant("msg_1", T13, 10, 100, 20, 5),
            assistant("msg_2", T13, 30, 200, 40, 7),
        ],
    );
    let usage = usage_in(UsageLog::ClaudeJsonl, SESSION, T12, T14, home.path()).unwrap();
    assert_eq!(usage.records, 2, "{usage:?}");
    assert_eq!(usage.input, 40, "{usage:?}");
    assert_eq!(usage.cache_read, 300, "{usage:?}");
    assert_eq!(usage.cache_write, 60, "{usage:?}");
    assert_eq!(usage.output, 12, "{usage:?}");
    // Claude's records carry no cost.
    assert_eq!(usage.cost_usd, None, "{usage:?}");
}

#[test]
fn the_window_cuts_the_records_at_its_edges() {
    let home = tempfile::tempdir().unwrap();
    let _ = session(
        home.path(),
        &[
            assistant("msg_1", T12, 10, 100, 20, 5),
            assistant("msg_2", T13, 30, 200, 40, 7),
            assistant("msg_3", T14, 50, 300, 60, 9),
        ],
    );
    let usage = usage_in(UsageLog::ClaudeJsonl, SESSION, T13, T14, home.path()).unwrap();
    assert_eq!(usage.records, 1, "{usage:?}");
    assert_eq!(usage.input, 30, "{usage:?}");
}

#[test]
fn a_session_without_a_file_is_unknown() {
    let home = tempfile::tempdir().unwrap();
    let usage = usage_in(
        UsageLog::ClaudeJsonl,
        "0199not-a-session",
        T12,
        T14,
        home.path(),
    );
    assert!(
        usage.is_none(),
        "a session without a file is unknown, not zero"
    );
}

#[test]
fn the_context_of_the_last_message_is_its_input_with_the_cache() {
    let home = tempfile::tempdir().unwrap();
    let _ = session(
        home.path(),
        &[
            assistant("msg_1", T12, 10, 100, 20, 5),
            assistant("msg_2", T13, 30, 200, 40, 7),
        ],
    );
    // input + cache_read + cache_creation of the last message (§3.5).
    let context = last_context_in(UsageLog::ClaudeJsonl, SESSION, home.path()).unwrap();
    assert_eq!(context, 30 + 200 + 40);
}
