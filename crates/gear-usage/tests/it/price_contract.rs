//! T-484: the price table — official prices, peak windows, overrides, and a
//! cost that cannot be known as unknown, never zero.

use gear_usage::{round_usage, Price, UsageLog};
use serde_json::json;
use std::collections::BTreeMap;

use crate::shared::write_lines;

/// 2026-10-24T12:00:00Z, a Saturday.
const SAT: i64 = 1_792_843_200;
/// 2026-10-27T02:00:00Z, a Tuesday inside a peak window.
const TUE_PEAK: i64 = 1_793_066_400;

fn assistant(time: i64, tokens: i64, cost: f64, model: Option<&str>) -> serde_json::Value {
    let mut message = json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "done"}],
        "usage": {
            "input": tokens,
            "output": tokens,
            "cacheRead": tokens,
            "cacheWrite": 0,
            "reasoning": 0,
            "cost": {"total": cost}
        }
    });
    if let Some(model) = model {
        message["model"] = json!(model);
    }
    json!({"type": "message", "timestamp": time, "message": message})
}

fn session(dir: &std::path::Path, line: serde_json::Value) -> String {
    write_lines(&dir.join("session.jsonl"), &[line])
        .to_string_lossy()
        .into_owned()
}

fn priced(
    dir: &std::path::Path,
    at: i64,
    model: &str,
    prices: &BTreeMap<String, Price>,
) -> Option<f64> {
    let path = session(dir, assistant(at, 1_000_000, 0.0, None));
    let result = round_usage(UsageLog::PiJsonl, &[path], at, at + 60, Some(model), prices)?;
    assert_eq!(result.usage.records, 1);
    result.usage.cost_usd
}

#[test]
fn a_zero_cost_with_tokens_and_no_price_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let path = session(&dir.path(), assistant(SAT, 6_100_000, 0.0, None));
    let result = round_usage(
        UsageLog::PiJsonl,
        &[path],
        SAT,
        SAT + 60,
        Some("vendor/unknown-model-1"),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(result.usage.records, 1);
    assert_eq!(result.usage.cost_usd, None);
    assert_eq!(result.cost_source, "none");
    assert_eq!(result.usage.input, 6_100_000);
}

#[test]
fn a_deepseek_record_is_priced_at_the_off_peak_rate_on_a_saturday() {
    let dir = tempfile::tempdir().unwrap();
    let cost = priced(
        dir.path(),
        SAT,
        "tokenrhythm/deepseek-v4.1-flash-1017",
        &BTreeMap::new(),
    )
    .unwrap();
    // 1M input at 0.15, 1M output at 0.60, 1M cache read at 0.003.
    assert!((cost - 0.753).abs() < 1e-9, "{cost}");
}

#[test]
fn a_deepseek_record_is_priced_at_the_peak_rate_on_a_tuesday_0200_utc() {
    let dir = tempfile::tempdir().unwrap();
    let cost = priced(
        dir.path(),
        TUE_PEAK,
        "tokenrhythm/deepseek-v4.1-flash-1017",
        &BTreeMap::new(),
    )
    .unwrap();
    // 1M input at 0.30, 1M output at 1.20, 1M cache read at 0.006.
    assert!((cost - 1.506).abs() < 1e-9, "{cost}");
}

#[test]
fn a_provider_prefixed_differently_cased_model_matches_the_builtin_entry() {
    let dir = tempfile::tempdir().unwrap();
    let hk = priced(
        dir.path(),
        SAT,
        "hk-dgx-1/DeepSeek-V4.1-Flash-1017",
        &BTreeMap::new(),
    )
    .unwrap();
    let rhythm = priced(
        dir.path(),
        SAT,
        "tokenrhythm/deepseek-v4.1-flash-1017",
        &BTreeMap::new(),
    )
    .unwrap();
    assert!((hk - 0.753).abs() < 1e-9, "{hk}");
    assert!((rhythm - hk).abs() < 1e-9, "{rhythm} != {hk}");
}

#[test]
fn a_prices_entry_overrides_the_builtin_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut prices = BTreeMap::new();
    prices.insert(
        "vendor/DeepSeek-V4.1-Flash-1017".to_string(),
        Price {
            input: 1.0,
            output: 2.0,
            cache_read: 3.0,
            cache_write: 4.0,
            peak: None,
        },
    );
    // The override has no peak: it prices the Tuesday record at its base.
    // 1M input at 1.0, 1M output at 2.0, 1M cache read at 3.0.
    let cost = priced(
        dir.path(),
        TUE_PEAK,
        "tokenrhythm/deepseek-v4.1-flash-1017",
        &prices,
    )
    .unwrap();
    assert!((cost - 6.0).abs() < 1e-9, "{cost}");
}

#[test]
fn a_real_nonzero_cli_cost_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = session(
        &dir.path(),
        assistant(SAT, 1_000_000, 0.42, Some("vendor/unknown-model-1")),
    );
    let result = round_usage(
        UsageLog::PiJsonl,
        &[path],
        SAT,
        SAT + 60,
        None,
        &BTreeMap::new(),
    )
    .unwrap();
    assert!((result.usage.cost_usd.unwrap() - 0.42).abs() < 1e-9);
    assert_eq!(result.cost_source, "agent");
}

#[test]
fn a_record_with_no_tokens_and_cost_zero_stays_zero() {
    let dir = tempfile::tempdir().unwrap();
    let path = session(&dir.path(), assistant(SAT, 0, 0.0, None));
    let result = round_usage(
        UsageLog::PiJsonl,
        &[path],
        SAT,
        SAT + 60,
        Some("vendor/unknown-model-1"),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(result.usage.records, 1);
    assert_eq!(result.usage.cost_usd, Some(0.0));
    assert_eq!(result.cost_source, "agent");
}

#[test]
fn the_builtin_table_carries_the_official_deepseek_prices() {
    let flash = gear_usage::price_for("deepseek-v4.1-flash-1017", &BTreeMap::new()).unwrap();
    assert_eq!(flash.input, 0.15);
    assert_eq!(flash.output, 0.60);
    assert_eq!(flash.cache_read, 0.003);
    let peak = flash.peak.as_ref().unwrap();
    assert_eq!(peak.rates.input, 0.30);
    assert_eq!(peak.rates.output, 1.20);
    assert_eq!(peak.rates.cache_read, 0.006);
    assert_eq!(peak.windows.len(), 2);
    assert!(gear_usage::in_window(&peak.windows, TUE_PEAK));
    assert!(!gear_usage::in_window(&peak.windows, SAT));

    let pro = gear_usage::price_for("deepseek-v4-pro-0813", &BTreeMap::new()).unwrap();
    assert_eq!(pro.input, 0.66);
    assert_eq!(pro.output, 1.98);
    assert_eq!(pro.cache_read, 0.022);
    assert_eq!(pro.peak.as_ref().unwrap().rates.input, 1.32);
}
