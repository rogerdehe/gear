//! T-498: agy's conversation transcript (`agy-jsonl`, §4.3). agy 1.4.12 keeps
//! a conversation under
//! `~/.gemini/antigravity-cli/brain/<conversation>/.system_generated/logs/`;
//! its `transcript.jsonl` carries the model records with the token counts.
//!
//! The fixtures are real local runs (content redacted): two model records of a
//! probe conversation, and one with a cache read. The transcript carries no
//! cost and no model id, so the record's model is None and the caller's model
//! prices it.

use std::path::Path;

use gear_usage::{last_context_in, usage_in, usage_records_in, UsageLog};

/// The real conversation ids the fixtures come from.
const FIRST: &str = "a7d0f269-309f-4b6d-9fe6-420f25e39ee3";
const CACHED: &str = "d4d30827-976d-4bcf-8a7c-79df0182e8f6";
const FIRST_RUN: &str = include_str!("fixtures/agy-transcript.jsonl");
const CACHED_RUN: &str = include_str!("fixtures/agy-transcript-cache.jsonl");
/// 2026-10-31T12:00:00Z and 2026-10-31T13:00:00Z.
const T0: i64 = 1_793_448_000;
const T1: i64 = 1_793_451_600;

/// Write a fixture where agy keeps a conversation and give the conversation
/// id back.
fn session(home: &Path, id: &str, text: &str) -> String {
    let dir = home
        .join(".gemini")
        .join("antigravity-cli")
        .join("brain")
        .join(id)
        .join(".system_generated")
        .join("logs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("transcript.jsonl"), text).unwrap();
    id.to_string()
}

#[test]
fn a_real_agy_conversation_reads_its_tokens() {
    let home = tempfile::tempdir().unwrap();
    let id = session(home.path(), FIRST, FIRST_RUN);
    let usage = usage_in(UsageLog::AgyJsonl, &id, T0, T1, home.path()).unwrap();
    assert_eq!(usage.records, 2, "{usage:?}");
    assert_eq!(usage.input, 6732 + 6982, "{usage:?}");
    assert_eq!(usage.output, 171 + 187, "{usage:?}");
    assert_eq!(usage.cache_read, 0, "{usage:?}");
    assert_eq!(usage.cache_write, 0, "{usage:?}");
    assert!(usage.cost_usd.is_none(), "the transcript carries no cost");
}

#[test]
fn the_cache_read_and_the_last_context_are_read() {
    let home = tempfile::tempdir().unwrap();
    let id = session(home.path(), CACHED, CACHED_RUN);
    let usage = usage_in(UsageLog::AgyJsonl, &id, T0, T1, home.path()).unwrap();
    assert_eq!(usage.records, 1, "{usage:?}");
    assert_eq!(usage.input, 4096, "{usage:?}");
    assert_eq!(usage.cache_read, 3863, "{usage:?}");
    assert_eq!(usage.output, 47, "{usage:?}");
    assert_eq!(
        last_context_in(UsageLog::AgyJsonl, &id, home.path()),
        Some(4096 + 3863)
    );
}

#[test]
fn the_records_carry_their_time_and_no_model_id() {
    let home = tempfile::tempdir().unwrap();
    let id = session(home.path(), FIRST, FIRST_RUN);
    let records = usage_records_in(UsageLog::AgyJsonl, &id, T0, T1, home.path()).unwrap();
    assert_eq!(records.len(), 2, "{records:?}");
    let times: Vec<i64> = records.iter().map(|record| record.at).collect();
    assert_eq!(times, vec![1_793_449_976, 1_793_450_026]);
    for record in &records {
        assert!(
            record.model.is_none(),
            "the transcript carries no model id: {record:?}"
        );
    }
}

#[test]
fn a_window_inside_the_conversation_counts_only_its_records() {
    let home = tempfile::tempdir().unwrap();
    let id = session(home.path(), FIRST, FIRST_RUN);
    // 12:33:00Z: the first model record is older than the window.
    let usage = usage_in(UsageLog::AgyJsonl, &id, 1_793_450_000, T1, home.path()).unwrap();
    assert_eq!(usage.records, 1, "{usage:?}");
    assert_eq!(usage.input, 6982, "{usage:?}");
}

#[test]
fn an_agy_conversation_that_was_never_written_is_unknown() {
    let home = tempfile::tempdir().unwrap();
    assert!(usage_in(UsageLog::AgyJsonl, "no-such-id", T0, T1, home.path()).is_none());
    assert!(usage_in(UsageLog::AgyJsonl, "", T0, T1, home.path()).is_none());
}
