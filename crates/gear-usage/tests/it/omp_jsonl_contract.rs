//! T-496: omp's session log (`pi-jsonl`, §4.3). omp is a pi fork and writes
//! the same `message`/`usage` records under
//! `~/.omp/agent/sessions/<path>/*.jsonl`, so the pi-jsonl reader reads it.
//!
//! The fixture is a real omp 18.12.6 session (the T-495 probe, redacted): its
//! calls failed with `invalid api key`, so every usage number in the log is
//! 0; the three assistant records here carry hand-set non-zero numbers, with
//! the log's own model, times and session id kept.

use std::path::Path;

use gear_usage::{last_context_in, usage_in, usage_records_in, UsageLog};

/// The real session id in the fixture's `session` record.
const SESSION: &str = "01a1917f-fad9-7369-ac78-80c212a8a31f";
const SAMPLE: &str = include_str!("fixtures/omp-session.jsonl");
/// 2026-10-31T10:00:00Z and 2026-10-31T10:30:00Z: the whole probe.
const T0: i64 = 1_793_440_800;
const T1: i64 = 1_793_442_600;

/// Write the fixture where omp writes a session, named as omp names it, and
/// give its path back.
fn session(dir: &Path) -> String {
    let path = dir.join(format!("2026-10-31T10-27-26-041Z_{SESSION}.jsonl"));
    std::fs::write(&path, SAMPLE).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn a_real_omp_session_reads_its_tokens_and_cost() {
    let dir = tempfile::tempdir().unwrap();
    let path = session(dir.path());
    let usage = usage_in(UsageLog::PiJsonl, &path, T0, T1, dir.path()).unwrap();
    assert_eq!(usage.records, 3, "{usage:?}");
    assert_eq!(usage.input, 2900, "{usage:?}");
    assert_eq!(usage.output, 650, "{usage:?}");
    assert_eq!(usage.cache_read, 17_000, "{usage:?}");
    assert_eq!(usage.cache_write, 750, "{usage:?}");
    assert_eq!(usage.reasoning, 0, "{usage:?}");
    let cost = usage.cost_usd.unwrap();
    assert!((cost - 0.0314).abs() < 1e-9, "{cost}");
}

#[test]
fn the_model_and_the_time_of_each_omp_record_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = session(dir.path());
    let records = usage_records_in(UsageLog::PiJsonl, &path, T0, T1, dir.path()).unwrap();
    assert_eq!(records.len(), 3, "{records:?}");
    for record in &records {
        assert_eq!(
            record.model.as_deref(),
            Some("minimax/MiniMax-M2"),
            "{record:?}"
        );
    }
    let times: Vec<i64> = records.iter().map(|record| record.at).collect();
    assert_eq!(times, vec![1_793_442_498, 1_793_442_569, 1_793_442_585]);
}

#[test]
fn a_window_inside_the_omp_session_counts_only_its_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = session(dir.path());
    // 10:29:30Z: the first two assistant records are older than the window.
    let usage = usage_in(UsageLog::PiJsonl, &path, 1_793_442_570, T1, dir.path()).unwrap();
    assert_eq!(usage.records, 1, "{usage:?}");
    assert_eq!(usage.input, 900, "{usage:?}");
    assert_eq!(usage.cache_read, 7000, "{usage:?}");
}

#[test]
fn the_context_of_the_last_omp_record_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = session(dir.path());
    assert_eq!(
        last_context_in(UsageLog::PiJsonl, &path, dir.path()),
        Some(900 + 7000 + 250)
    );
}

#[test]
fn an_omp_session_that_was_never_written_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir
        .path()
        .join("nothing.jsonl")
        .to_string_lossy()
        .into_owned();
    assert!(usage_in(UsageLog::PiJsonl, &missing, T0, T1, dir.path()).is_none());
}
