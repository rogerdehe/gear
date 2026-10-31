//! The quota of the providers (§9.1): one reading per call.
//!
//! The screen is not the source of quota facts. Each provider's quota is
//! read from its API:
//!
//! - `minimax` — `GET https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains`
//!   with the key as a bearer token: per entry of `model_remains` the
//!   remaining percent of the 5 h window (`current_interval_remaining_percent`,
//!   `end_time` in ms) and of the weekly one
//!   (`current_weekly_remaining_percent`, `weekly_end_time`);
//! - `opencode-go` — `GET https://opencode.ai/zen/go/v1/usage` with the key as
//!   a bearer token: `usage.rolling/weekly/monthly`, each a `percent` (used)
//!   and a `resetsAt`;
//! - `codex` — the newest `token_count` line of the reviewer's rollout
//!   (§4.3): `rate_limits.primary/secondary`, each a `used_percent`, a
//!   `window_minutes` and a `resets_at` (in seconds).
//!
//! The HTTP call goes through `curl -sS --max-time 15` (the workspace has no
//! HTTP crate). **The key never appears on the command line**, where `ps`
//! would show it: it is written to curl's standard input with `-H @-`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use gear_model::Ts;
use serde_json::Value;

use crate::{codex_rollout_path, number_to_seconds, parse, read_lines, rfc3339};

/// The readers of §9.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaReader {
    Minimax,
    OpencodeGo,
    Codex,
}

impl QuotaReader {
    /// The name of the reader in the config (`reader` of §9.1).
    pub fn name(self) -> &'static str {
        match self {
            QuotaReader::Minimax => "minimax",
            QuotaReader::OpencodeGo => "opencode-go",
            QuotaReader::Codex => "codex",
        }
    }

    /// The reader a config names; None for a name that is not one of them.
    pub fn from_name(name: &str) -> Option<QuotaReader> {
        match name.trim() {
            "minimax" => Some(QuotaReader::Minimax),
            "opencode-go" => Some(QuotaReader::OpencodeGo),
            "codex" => Some(QuotaReader::Codex),
            _ => None,
        }
    }
}

/// One window of a provider's quota (§9.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// What the window is: `5h`, `weekly`, `rolling`, `primary`…
    pub name: String,
    /// How much of the window is used, in percent.
    pub used_percent: f64,
    /// When the window starts again. None when the provider does not say.
    pub resets_at: Option<Ts>,
}

/// One reading of a provider's quota (§9.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// The provider the reading is of.
    pub provider: String,
    pub windows: Vec<Window>,
    /// When the reading was taken.
    pub read_at: Ts,
}

/// Read the quota of a provider (§9.1).
///
/// `key` is the API key of the provider (the readers of `minimax` and
/// `opencode-go`); `codex_session` is the thread of the reviewer whose
/// rollout holds the limits (the reader of `codex`).
///
/// Err: the reason — the call failed, the response is not the one of the
/// provider, or the key is missing.
pub fn read_quota(
    reader: QuotaReader,
    key: &str,
    codex_session: Option<&str>,
    now: Ts,
) -> Result<Reading, String> {
    read_quota_in(reader, key, codex_session, now, &home())
}

/// The same, with the home folder the Codex sessions live in.
pub fn read_quota_in(
    reader: QuotaReader,
    key: &str,
    codex_session: Option<&str>,
    now: Ts,
    home: &Path,
) -> Result<Reading, String> {
    match reader {
        QuotaReader::Minimax => {
            let body = http_get(
                QuotaReader::Minimax.name(),
                "https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains",
                key,
            )?;
            minimax(&body, now)
        }
        QuotaReader::OpencodeGo => {
            let body = http_get(
                QuotaReader::OpencodeGo.name(),
                "https://opencode.ai/zen/go/v1/usage",
                key,
            )?;
            opencode_go(&body, now)
        }
        QuotaReader::Codex => {
            let session = codex_session
                .filter(|session| !session.trim().is_empty())
                .ok_or("the codex reader needs the session of the reviewer")?;
            codex(session, now, home)
        }
    }
}

/// The reset a provider's own limit text states (§9.1, M4 #20).
///
/// Some providers GEAR has no reader for say when they are available again
/// only in the text the agent shows. The lines are read from the last to the
/// first, and only a line that speaks of a reset counts: one that holds
/// `reset`, `try again`, `retry` or `available again` (ASCII
/// case-insensitively). Such a line gives, in this order:
///
/// 1. an RFC 3339 timestamp among its words (`quota resets at
///    2026-10-29T05:00:00Z`) — that time;
/// 2. `retry-after: <n>` — `now` plus `n` seconds;
/// 3. `in <duration>` or `after <duration>` — `now` plus the duration, which
///    is one or more `<number><unit>` groups (`2h 13m`, `45 minutes`,
///    `3 hours 5 min`, `90s`).
///
/// None when no line states a reset, or when the time it states is not after
/// `now` or more than eight days after it: a clock time such as `resets 3pm`
/// has no time zone GEAR could read, and a window longer than eight days is
/// none of the windows GEAR knows. A time out of that window is None for the
/// **whole call**: the last line that states a reset is the provider's latest
/// word, and an earlier line may be stale.
pub fn reset_from_text(text: &str, now: Ts) -> Option<Ts> {
    // The longest window a stated reset can be of: eight days, in seconds.
    const LONGEST: Ts = 8 * 86_400;
    for line in text.lines().rev() {
        if !speaks_of_a_reset(line) {
            continue;
        }
        let Some(time) = reset_of_line(line, now) else {
            continue;
        };
        if time > now && time - now <= LONGEST {
            return Some(time);
        }
        // The provider's latest word names a time GEAR cannot use: an earlier
        // line would only be staler (contract v3).
        return None;
    }
    None
}

/// Does the line speak of a reset? These four words are the ones the limit
/// texts of the providers use.
fn speaks_of_a_reset(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    ["reset", "try again", "retry", "available again"]
        .iter()
        .any(|word| line.contains(word))
}

/// The reset the one line states, in epoch seconds.
fn reset_of_line(line: &str, now: Ts) -> Option<Ts> {
    let words: Vec<&str> = line.split_whitespace().collect();
    // 1. An RFC 3339 time among the words, without the punctuation the
    // sentence puts after it.
    for word in &words {
        if let Some(time) = rfc3339(trim_punctuation(word)) {
            return Some(time);
        }
    }
    // 2. `retry-after: <n>` — the seconds HTTP names.
    for (index, word) in words.iter().enumerate() {
        let lower = trim_punctuation(word).to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("retry-after") else {
            continue;
        };
        let rest = rest.trim_start_matches(':').trim();
        let seconds = if rest.is_empty() {
            // `Retry-After: 120` — the number is the next word.
            words
                .get(index + 1)
                .map(|next| trim_punctuation(next))
                .and_then(|next| next.parse::<Ts>().ok())
        } else {
            // `Retry-After:120` — the number is in the same word.
            rest.parse::<Ts>().ok()
        };
        if let Some(seconds) = seconds.filter(|seconds| *seconds >= 0) {
            return Some(now + seconds);
        }
    }
    // 3. `in <duration>` or `after <duration>`.
    for (index, word) in words.iter().enumerate() {
        let lower = trim_punctuation(word).to_ascii_lowercase();
        if lower != "in" && lower != "after" {
            continue;
        }
        if let Some(seconds) = duration_seconds(&words[index + 1..]) {
            return Some(now + seconds);
        }
    }
    None
}

/// A word with the punctuation a sentence puts after it trimmed.
fn trim_punctuation(word: &str) -> &str {
    word.trim_end_matches(['.', ',', ';', ':', ')'])
}

/// The seconds of the duration at the words: one or more `<number><unit>`
/// groups, with or without a space between the number and its unit.
fn duration_seconds(words: &[&str]) -> Option<Ts> {
    let mut total: Ts = 0;
    let mut groups = 0;
    let mut index = 0;
    while index < words.len() {
        let token = trim_punctuation(words[index]);
        let digits = token
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .count();
        if digits == 0 {
            // The duration ends at the first word that is not a number.
            break;
        }
        let number: f64 = token.get(..digits)?.parse().ok()?;
        let rest = token.get(digits..)?;
        let unit = if rest.is_empty() {
            // `45 minutes` — the unit is the next word.
            index += 1;
            trim_punctuation(words.get(index)?)
        } else {
            // `2h` — the unit is in the same word.
            rest
        };
        total += (number * unit_seconds(unit)? as f64).round() as Ts;
        groups += 1;
        index += 1;
    }
    if groups == 0 {
        return None;
    }
    Some(total)
}

/// The seconds of one unit of a duration.
fn unit_seconds(unit: &str) -> Option<Ts> {
    match unit.to_ascii_lowercase().as_str() {
        "d" | "day" | "days" => Some(86_400),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(3_600),
        "m" | "min" | "mins" | "minute" | "minutes" => Some(60),
        "s" | "sec" | "secs" | "second" | "seconds" => Some(1),
        _ => None,
    }
}

/// The call of §9.1: `curl -sS --max-time 15` with the key in a header that
/// goes in through standard input, never on the command line.
fn http_get(reader: &str, url: &str, key: &str) -> Result<String, String> {
    if key.trim().is_empty() {
        return Err("the provider needs an API key".to_string());
    }
    let mut child = Command::new("curl")
        // `-w` writes the HTTP status on the last line of the answer: curl
        // itself is happy with a 401 or a 500, so the status has to be read.
        .args([
            "-sS",
            "--max-time",
            "15",
            "-w",
            "\\n%{http_code}",
            "-H",
            "@-",
            url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("curl could not be started: {error}"))?;
    {
        // The header goes in through standard input: `ps` never shows it.
        let stdin = child
            .stdin
            .as_mut()
            .ok_or("curl has no standard input to write the key to")?;
        stdin
            .write_all(format!("Authorization: Bearer {key}\n").as_bytes())
            .map_err(|error| format!("the key could not be written to curl: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("curl could not be waited for: {error}"))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        let reason = reason.trim();
        return Err(match output.status.code() {
            Some(code) if reason.is_empty() => format!("curl failed with the code {code}"),
            Some(code) => format!("curl failed with the code {code}: {reason}"),
            None => format!("curl was killed: {reason}"),
        });
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("the response is not text: {error}"))?;
    // The last line of the answer is the HTTP status that `-w` wrote.
    let (body, status) = match stdout.rsplit_once('\n') {
        Some((body, status)) => (body.to_string(), status.trim().to_string()),
        None => (stdout.clone(), String::new()),
    };
    let status: u16 = status.parse().map_err(|_| {
        format!(
            "{reader}: the answer of the provider has no HTTP status: {}",
            head(&body)
        )
    })?;
    if status >= 400 {
        // An HTTP error is a normal thing (an expired key, a throttled
        // request, an outage): it must say what it was.
        return Err(format!("{reader}: HTTP {status}: {}", head(&body)));
    }
    Ok(body)
}

/// The first 200 characters of a body, for the reason of an error.
fn head(body: &str) -> String {
    body.chars().take(200).collect()
}

/// MiniMax: per entry of `model_remains` the 5 h window and the weekly one.
fn minimax(body: &str, now: Ts) -> Result<Reading, String> {
    let value: Value = serde_json::from_str(body)
        .map_err(|error| format!("the response of minimax is not JSON: {error}"))?;
    let entries = value
        .get("model_remains")
        .and_then(Value::as_array)
        .ok_or("the response of minimax has no model_remains")?;
    let mut windows = Vec::new();
    for entry in entries {
        let model = entry
            .get("model_name")
            .and_then(Value::as_str)
            .unwrap_or("minimax");
        // The provider says how much is left; the reading says how much is
        // used.
        if let Some(left) = entry
            .get("current_interval_remaining_percent")
            .and_then(Value::as_f64)
        {
            windows.push(Window {
                name: format!("{model} 5h"),
                used_percent: 100.0 - left,
                resets_at: entry.get("end_time").and_then(time_of),
            });
        }
        if let Some(left) = entry
            .get("current_weekly_remaining_percent")
            .and_then(Value::as_f64)
        {
            windows.push(Window {
                name: format!("{model} weekly"),
                used_percent: 100.0 - left,
                resets_at: entry.get("weekly_end_time").and_then(time_of),
            });
        }
    }
    Ok(Reading {
        provider: QuotaReader::Minimax.name().to_string(),
        windows,
        read_at: now,
    })
}

/// OpenCode Go: `usage.rolling/weekly/monthly`, each a percent that is used.
fn opencode_go(body: &str, now: Ts) -> Result<Reading, String> {
    let value: Value = serde_json::from_str(body)
        .map_err(|error| format!("the response of opencode-go is not JSON: {error}"))?;
    let usage = value
        .get("usage")
        .ok_or("the response of opencode-go has no usage")?;
    let mut windows = Vec::new();
    for name in ["rolling", "weekly", "monthly"] {
        let Some(entry) = usage.get(name) else {
            continue;
        };
        let Some(percent) = entry.get("percent").and_then(Value::as_f64) else {
            continue;
        };
        windows.push(Window {
            name: name.to_string(),
            used_percent: percent,
            resets_at: entry.get("resetsAt").and_then(time_of),
        });
    }
    if windows.is_empty() {
        return Err("the response of opencode-go has no window".to_string());
    }
    Ok(Reading {
        provider: QuotaReader::OpencodeGo.name().to_string(),
        windows,
        read_at: now,
    })
}

/// Codex: the newest token count of the reviewer's rollout that carries the
/// rate limits (§4.3, §9.1).
fn codex(session: &str, now: Ts, home: &Path) -> Result<Reading, String> {
    let path = codex_rollout_path(home, session)
        .ok_or_else(|| format!("the rollout of the session {session} is not there"))?;
    let lines = read_lines(&path)
        .ok_or_else(|| format!("the rollout of the session {session} cannot be read"))?;
    let mut windows = Vec::new();
    for line in lines.iter().rev() {
        let Some(value) = parse(line) else { continue };
        if value.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        let Some(limits) = payload.get("rate_limits") else {
            continue;
        };
        for name in ["primary", "secondary"] {
            let Some(entry) = limits.get(name) else {
                continue;
            };
            let Some(used) = entry.get("used_percent").and_then(Value::as_f64) else {
                continue;
            };
            windows.push(Window {
                name: window_name(name, entry),
                used_percent: used,
                resets_at: entry.get("resets_at").and_then(time_of),
            });
        }
        if !windows.is_empty() {
            // The newest count that carries the limits is the reading.
            break;
        }
    }
    if windows.is_empty() {
        return Err("the rollout of the session carries no rate limits".to_string());
    }
    Ok(Reading {
        provider: QuotaReader::Codex.name().to_string(),
        windows,
        read_at: now,
    })
}

/// The name of a Codex window: its length when the rollout says one (T-383).
///
/// `5h` for 300 minutes, `weekly` for 10080, else `<minutes / 60>h` when the
/// minutes divide by 60, else `<minutes>m`. The API's own key
/// (`primary`/`secondary`) is the name only when there is no `window_minutes`.
fn window_name(key: &str, entry: &Value) -> String {
    let Some(minutes) = entry.get("window_minutes").and_then(Value::as_f64) else {
        return key.to_string();
    };
    let minutes = minutes.round() as i64;
    match minutes {
        300 => "5h".to_string(),
        10_080 => "weekly".to_string(),
        minutes if minutes % 60 == 0 => format!("{}h", minutes / 60),
        minutes => format!("{minutes}m"),
    }
}

/// A time of a provider's answer: a number (seconds or milliseconds) or a
/// string (RFC 3339).
fn time_of(value: &Value) -> Option<Ts> {
    match value {
        Value::Number(number) => number.as_f64().map(number_to_seconds),
        Value::String(text) => {
            let text = text.trim();
            if let Ok(seconds) = text.parse::<f64>() {
                return Some(number_to_seconds(seconds));
            }
            rfc3339(text)
        }
        _ => None,
    }
}

/// The home folder of the user; an empty path when there is none.
fn home() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
}
