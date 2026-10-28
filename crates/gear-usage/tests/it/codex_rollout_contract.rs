//! T-137: Codex's rollout file (`codex-rollout`, §4.3). Every token count
//! carries the totals of the session so far, so the record is the difference
//! to the one before it — and the first valid total counts against zero.

use gear_usage::{codex_day_dir, last_context_in, usage_in, UsageLog};
use serde_json::json;

use crate::shared::{iso, now, write_lines, T12, T13, T14};

const THREAD: &str = "0199aaaa-bbbb-cccc-dddd-eeeeffff0000";

/// One token count of Codex: the totals of the session so far.
fn count(
    time: i64,
    input: i64,
    cached: i64,
    cache_write: i64,
    output: i64,
    reasoning: i64,
) -> serde_json::Value {
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

/// One token count whose `info` is null: it is not a count (§4.3).
fn count_without_info(time: i64) -> serde_json::Value {
    json!({
        "timestamp": iso(time),
        "type": "event_msg",
        "payload": {"type": "token_count", "info": null}
    })
}

/// One token count that carries the usage of its last request.
fn count_with_last_usage(
    time: i64,
    input: i64,
    cached: i64,
    cache_write: i64,
    output: i64,
    last_input: i64,
    last_cache_write: i64,
) -> serde_json::Value {
    let mut value = count(time, input, cached, cache_write, output, 0);
    value["payload"]["info"]["last_token_usage"] = json!({
        "input_tokens": last_input,
        "cached_input_tokens": 0,
        "cache_write_input_tokens": last_cache_write,
        "output_tokens": output,
        "reasoning_output_tokens": 0
    });
    value
}

/// The rollout file of a thread, in the day directory of the Codex sessions.
fn rollout(home: &std::path::Path, lines: &[serde_json::Value]) -> String {
    let dir = codex_day_dir(home, now());
    let path = write_lines(
        &dir.join(format!("rollout-2026-10-26T12-00-00-{THREAD}.jsonl")),
        lines,
    );
    path.to_string_lossy().into_owned()
}

#[test]
fn the_first_total_counts_against_zero() {
    let home = tempfile::tempdir().unwrap();
    // A fresh session with one count: its total is the work of the session so
    // far, and it counts (review F1).
    let _ = rollout(home.path(), &[count(T13, 1000, 200, 0, 100, 10)]);
    let usage = usage_in(UsageLog::CodexRollout, THREAD, T12, T14, home.path()).unwrap();
    assert_eq!(usage.records, 1, "{usage:?}");
    assert_eq!(usage.input, 800, "{usage:?}");
    assert_eq!(usage.cache_read, 200, "{usage:?}");
    assert_eq!(usage.output, 100, "{usage:?}");
    // The context of the turn is there too.
    let context = last_context_in(UsageLog::CodexRollout, THREAD, home.path()).unwrap();
    assert_eq!(context, 1000, "the context of a session with one count");
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
    // Every count is a difference to the one before it: the first one
    // against zero.
    assert_eq!(usage.records, 3, "{usage:?}");
    assert_eq!(usage.input, 800 + 300 + 100, "{usage:?}");
    assert_eq!(usage.cache_read, 200 * 3, "{usage:?}");
    assert_eq!(usage.cache_write, 0 + 50 + 30, "{usage:?}");
    assert_eq!(usage.output, 100 + 100 + 60, "{usage:?}");
    assert_eq!(usage.reasoning, 10 + 10 + 5, "{usage:?}");
    // The input of Codex counts the cached input: the two columns must not
    // count it twice, so their sum is the input of the session.
    assert_eq!(usage.input + usage.cache_read, 1800 - 0);
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
    assert_eq!(usage.input, 300, "{usage:?}");
}

#[test]
fn a_count_before_the_window_still_moves_the_predecessor() {
    let home = tempfile::tempdir().unwrap();
    let _ = rollout(
        home.path(),
        &[
            count(T12, 1000, 200, 0, 100, 10),
            count(T13, 1500, 400, 50, 200, 20),
        ],
    );
    // The first count is before the window: it is not counted, but it is the
    // predecessor of the one inside it.
    let usage = usage_in(UsageLog::CodexRollout, THREAD, T13, T14, home.path()).unwrap();
    assert_eq!(usage.records, 1, "{usage:?}");
    assert_eq!(usage.input, 300, "{usage:?}");
    assert_eq!(usage.cache_read, 200, "{usage:?}");
    assert_eq!(usage.output, 100, "{usage:?}");
}

#[test]
fn a_count_without_info_is_not_a_count() {
    let home = tempfile::tempdir().unwrap();
    // Valid, null, valid: the null one changes neither the totals nor the
    // predecessor (review F3).
    let _ = rollout(
        home.path(),
        &[
            count(T12, 1000, 200, 0, 100, 10),
            count_without_info(T12 + 1),
            count(T13, 1500, 400, 50, 200, 20),
        ],
    );
    let usage = usage_in(UsageLog::CodexRollout, THREAD, T12, T14, home.path()).unwrap();
    assert_eq!(usage.records, 2, "{usage:?}");
    // No negative record: the null one changed nothing.
    assert_eq!(usage.input, 800 + 300, "{usage:?}");
    assert_eq!(usage.cache_read, 400, "{usage:?}");
    assert_eq!(usage.output, 200, "{usage:?}");
    // The context of the last turn survives it.
    let context = last_context_in(UsageLog::CodexRollout, THREAD, home.path()).unwrap();
    assert_eq!(context, 500 + 50, "the context of the last turn");
}

#[test]
fn a_repeated_total_keeps_the_context_of_the_last_turn() {
    let home = tempfile::tempdir().unwrap();
    // Codex sends the same total again: a session must not look empty
    // (review F2).
    let _ = rollout(
        home.path(),
        &[
            count(T12, 1000, 200, 0, 100, 10),
            count(T13, 1500, 300, 0, 120, 15),
            count(T13 + 1, 1500, 300, 0, 120, 15),
        ],
    );
    let context = last_context_in(UsageLog::CodexRollout, THREAD, home.path()).unwrap();
    assert_eq!(context, 500, "the context of the last turn, not zero");
}

#[test]
fn the_context_is_the_usage_of_the_last_request_when_the_count_carries_it() {
    let home = tempfile::tempdir().unwrap();
    let _ = rollout(
        home.path(),
        &[
            count(T12, 1000, 200, 0, 100, 10),
            count_with_last_usage(T13, 1500, 400, 50, 200, 900, 40),
        ],
    );
    // `info.last_token_usage`: the input of the request (the cached input is
    // in it) plus what was written to the cache (§4.3).
    let context = last_context_in(UsageLog::CodexRollout, THREAD, home.path()).unwrap();
    assert_eq!(context, 900 + 40);
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
    assert!(
        usage.is_none(),
        "a thread without a file is unknown, not zero"
    );
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
