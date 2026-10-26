//! T-137: Codex's rollout file (`codex-rollout`, §4.3). Every token count
//! carries the totals of the session so far, so the record is the difference
//! to the one before it.

use gear_usage::{codex_day_dir, last_context_in, usage_in, UsageLog};
use serde_json::json;

use crate::shared::{iso, now, write_lines, T12, T13, T14};

const THREAD: &str = "0199aaaa-bbbb-cccc-dddd-eeeeffff0000";

/// One token count of Codex: the totals of the session so far.
fn count(time: i64, input: i64, cached: i64, cache_write: i64, output: i64, reasoning: i64) -> serde_json::Value {
    json!({
        "timestamp": iso(time),
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": {
                    "input_tokens": input,
                    "cached_input_tokens": cached,
                    "cache_write_input_tokens": cache_write,
                    "output_tokens": output,
                    "reasoning_output_tokens": reasoning,
                    "total_tokens": input + output
                }
            }
        }
    })
}

/// The rollout file of a thread, in the day directory of the Codex sessions.
fn rollout(home: &std::path::Path, lines: &[serde_json::Value]) -> String {
    let dir = codex_day_dir(home, now());
    let path = write_lines(&dir.join(format!("rollout-2026-10-26T12-00-00-{THREAD}.jsonl")), lines);
    path.to_string_lossy().into_owned()
}

#[test]
fn a_total_is_never_counted_twice() {
    let home = tempfile::tempdir().unwrap();
    // The counts carry the totals of the session so far.
    let _ = rollout(
        home.path(),
        &[
            count(T12, 1000, 200, 0, 100, 10),
            count(T13, 1500, 400, 50, 200, 20),
            count(T14, 1800, 600, 80, 260, 25),
        ],
    );
    let usage = usage_in(UsageLog::CodexRollout, THREAD, T12, T14 + 1, home.path()).unwrap();
    // The differences of the two counts after the first one.
    assert_eq!(usage.records, 2, "{usage:?}");
    assert_eq!(usage.input, (1500 - 1000 - (400 - 200)) + (1800 - 1500 - (600 - 400)));
    assert_eq!(usage.cache_read, (400 - 200) + (600 - 400));
    assert_eq!(usage.cache_write, 50 + 30);
    assert_eq!(usage.output, 100 + 60);
    assert_eq!(usage.reasoning, 10 + 5);
    // The input of Codex counts the cached input: the two columns must not
    // count it twice, so their sum is the input of the session.
    assert_eq!(usage.input + usage.cache_read, 1500 - 1000 + 1800 - 1500);
}

#[test]
fn the_window_cuts_the_records_at_its_edges() {
    let home = tempfile::tempdir().unwrap();
    let _ = rollout(
        home.path(),
        &[
            count(T12, 1000, 200, 0, 100, 10),
            count(T13, 1500, 400, 50, 200, 20),
            count(T14, 1800, 600, 80, 260, 25),
        ],
    );
    // The count at T13 is inside the window, the one at T14 is not.
    let usage = usage_in(UsageLog::CodexRollout, THREAD, T13, T14, home.path()).unwrap();
    assert_eq!(usage.records, 1, "{usage:?}");
    assert_eq!(usage.output, 100, "{usage:?}");
}

#[test]
fn a_thread_without_a_file_is_unknown() {
    let home = tempfile::tempdir().unwrap();
    let usage = usage_in(
        UsageLog::CodexRollout,
        "0199not-a-thread",
        T12,
        T14,
        home.path(),
    );
    assert!(usage.is_none(), "a thread without a file is unknown, not zero");
}

#[test]
fn the_context_of_the_last_count_is_its_input_with_the_cache() {
    let home = tempfile::tempdir().unwrap();
    let _ = rollout(
        home.path(),
        &[
            count(T12, 1000, 200, 0, 100, 10),
            count(T13, 1500, 400, 50, 200, 20),
        ],
    );
    // The input of the turn with the cached input counted once, plus what was
    // written to the cache (§3.5).
    let context = last_context_in(UsageLog::CodexRollout, THREAD, home.path()).unwrap();
    assert_eq!(context, (1500 - 1000) + 50);
}

#[test]
fn the_rollout_file_of_a_day_that_is_too_old_is_not_read() {
    let home = tempfile::tempdir().unwrap();
    // A file of a day that is more than fourteen days old is not the file of
    // the thread any more.
    let old = codex_day_dir(home.path(), now() - 20 * 86_400);
    let _ = write_lines(
        &old.join(format!("rollout-2026-10-06T12-00-00-{THREAD}.jsonl")),
        &[count(T12, 1000, 200, 0, 100, 10)],
    );
    let usage = usage_in(UsageLog::CodexRollout, THREAD, T12, T14, home.path());
    assert!(usage.is_none(), "{usage:?}");
}
