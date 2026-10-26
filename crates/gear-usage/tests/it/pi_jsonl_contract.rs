//! T-137: pi's session file (`pi-jsonl`, §4.3).

use gear_usage::{last_context_in, usage_in, UsageLog};
use serde_json::json;

use crate::shared::{write_lines, T12, T13, T14};

/// One assistant message of pi, with its usage.
fn assistant(time: i64, input: i64, output: i64, cache_read: i64, cache_write: i64, cost: f64) -> serde_json::Value {
    json!({
        "type": "message",
        "timestamp": time,
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "done"}],
            "usage": {
                "input": input,
                "output": output,
                "cacheRead": cache_read,
                "cacheWrite": cache_write,
                "reasoning": 7,
                "cost": {"total": cost}
            }
        }
    })
}

fn session(dir: &std::path::Path, lines: &[serde_json::Value]) -> String {
    write_lines(&dir.join("session.jsonl"), lines).to_string_lossy().into_owned()
}

#[test]
fn the_records_of_the_window_are_summed() {
    let dir = tempfile::tempdir().unwrap();
    let path = session(
        dir.path(),
        &[
            assistant(T12, 10, 1, 100, 0, 0.01),
            assistant(T13, 20, 2, 200, 5, 0.02),
            assistant(T14, 30, 3, 300, 10, 0.03),
        ],
    );
    let usage = usage_in(UsageLog::PiJsonl, &path, T12, T14, dir.path()).unwrap();
    assert_eq!(usage.input, 30);
    assert_eq!(usage.output, 3);
    assert_eq!(usage.cache_read, 300);
    assert_eq!(usage.cache_write, 5);
    assert_eq!(usage.reasoning, 14);
    assert_eq!(usage.records, 2);
    let cost = usage.cost_usd.unwrap();
    assert!((cost - 0.03).abs() < 1e-9, "{cost}");
}

#[test]
fn the_window_cuts_the_records_at_its_edges() {
    let dir = tempfile::tempdir().unwrap();
    let path = session(
        dir.path(),
        &[
            assistant(T12, 10, 1, 0, 0, 0.0),
            assistant(T13, 20, 1, 0, 0, 0.0),
            assistant(T14, 30, 1, 0, 0, 0.0),
        ],
    );
    // from is counted, to is not: the window is `from <= time < to`.
    let usage = usage_in(UsageLog::PiJsonl, &path, T13, T14, dir.path()).unwrap();
    assert_eq!(usage.records, 1, "{usage:?}");
    assert_eq!(usage.input, 20, "{usage:?}");
    // A window that holds nothing is zero, not unknown.
    let empty = usage_in(UsageLog::PiJsonl, &path, T14 + 10, T14 + 20, dir.path()).unwrap();
    assert_eq!(empty.records, 0, "{empty:?}");
    assert_eq!(empty.input, 0, "{empty:?}");
}

#[test]
fn a_log_that_cannot_be_read_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nothing.jsonl");
    let usage = usage_in(
        UsageLog::PiJsonl,
        &missing.to_string_lossy(),
        T12,
        T14,
        dir.path(),
    );
    assert!(usage.is_none(), "a file that is not there is unknown, not zero");
    // The same through the door the callers use: the home of the user.
    let usage = gear_usage::usage(
        UsageLog::PiJsonl,
        &missing.to_string_lossy(),
        T12,
        T14,
    );
    assert!(usage.is_none(), "{usage:?}");
}

#[test]
fn the_names_of_the_logs_are_the_names_of_the_profiles() {
    // §2: the profile's `usage_log` names the reader of §4.3.
    for (name, log) in [
        ("pi-jsonl", UsageLog::PiJsonl),
        ("codex-rollout", UsageLog::CodexRollout),
        ("claude-jsonl", UsageLog::ClaudeJsonl),
        ("opencode-db", UsageLog::OpencodeDb),
    ] {
        assert_eq!(UsageLog::from_name(name), Some(log), "{name}");
        assert_eq!(log.name(), name);
    }
    assert_eq!(UsageLog::from_name("nothing"), None);
}

#[test]
fn the_messages_that_are_not_of_the_assistant_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = session(
        dir.path(),
        &[
            json!({"type": "message", "timestamp": T12,
                   "message": {"role": "user", "usage": {"input": 999, "output": 999}}}),
            json!({"type": "session", "timestamp": T12}),
            json!({"type": "message", "timestamp": T12, "message": {"role": "assistant"}}),
            assistant(T13, 20, 1, 0, 0, 0.0),
        ],
    );
    let usage = usage_in(UsageLog::PiJsonl, &path, T12, T14, dir.path()).unwrap();
    assert_eq!(usage.records, 1, "{usage:?}");
    assert_eq!(usage.input, 20, "{usage:?}");
}

#[test]
fn the_context_of_the_last_record_is_its_input_with_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = session(
        dir.path(),
        &[
            assistant(T12, 10, 1, 100, 0, 0.0),
            assistant(T13, 20, 1, 200, 5, 0.0),
        ],
    );
    // input + cacheRead + cacheWrite of the last assistant record (§3.5).
    let context = last_context_in(UsageLog::PiJsonl, &path, dir.path()).unwrap();
    assert_eq!(context, 20 + 200 + 5);
}

#[test]
fn a_time_in_milliseconds_is_read_as_a_time() {
    // pi writes its times in milliseconds; the reader takes both.
    let dir = tempfile::tempdir().unwrap();
    let path = session(
        dir.path(),
        &[assistant(T12 * 1000, 10, 1, 0, 0, 0.0)],
    );
    let usage = usage_in(UsageLog::PiJsonl, &path, T12, T14, dir.path()).unwrap();
    assert_eq!(usage.records, 1, "{usage:?}");
    assert_eq!(usage.input, 10, "{usage:?}");
}
