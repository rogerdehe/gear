//! GEAR: the token and cost records of an agent's session (design §4.3).
//!
//! Four kinds of log, one API:
//!
//! - `pi-jsonl` — the session file pi names;
//! - `codex-rollout` — the rollout file of a thread, under
//!   `~/.codex/sessions/YYYY/MM/DD`;
//! - `claude-jsonl` — the session file under `~/.claude/projects/*/`;
//! - `opencode-db` — the `session_message` table of
//!   `~/.local/share/opencode/opencode.db`.
//!
//! [`usage`] sums the records of a session inside a window (`from <= time <
//! to`); [`last_context`] reads the context of its last assistant record
//! (§3.5). A log that cannot be read is **unknown** (`None`), never zero: the
//! caller leaves the columns of its round NULL.
//!
//! The quota of the providers is read apart ([`read_quota`], §9.1): the screen
//! is not the source of quota facts.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gear_model::Ts;
use serde_json::Value;

mod quota;

pub use quota::{read_quota, read_quota_in, reset_from_text, QuotaReader, Reading, Window};

/// The kinds of log of §4.3, named by the profile's `usage_log` (§2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageLog {
    PiJsonl,
    CodexRollout,
    ClaudeJsonl,
    OpencodeDb,
}

impl UsageLog {
    /// The name of the log in the profile.
    pub fn name(self) -> &'static str {
        match self {
            UsageLog::PiJsonl => "pi-jsonl",
            UsageLog::CodexRollout => "codex-rollout",
            UsageLog::ClaudeJsonl => "claude-jsonl",
            UsageLog::OpencodeDb => "opencode-db",
        }
    }

    /// The log a profile names; None for a name that is not one of them.
    pub fn from_name(name: &str) -> Option<UsageLog> {
        match name.trim() {
            "pi-jsonl" => Some(UsageLog::PiJsonl),
            "codex-rollout" => Some(UsageLog::CodexRollout),
            "claude-jsonl" => Some(UsageLog::ClaudeJsonl),
            "opencode-db" => Some(UsageLog::OpencodeDb),
            _ => None,
        }
    }
}

/// The tokens and the cost of a set of records (§4.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Usage {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
    /// None when the log carries no cost.
    pub cost_usd: Option<f64>,
    /// How many records were counted.
    pub records: i64,
}

/// The sum of the records of one session with `from <= time < to` (§4.3).
///
/// None: the log cannot be read (the file is gone, the database is locked,
/// the thread has no file) — the tokens are unknown, not zero.
pub fn usage(log: UsageLog, session_ref: &str, from: Ts, to: Ts) -> Option<Usage> {
    usage_in(log, session_ref, from, to, &home())
}

/// The same, with the home folder the logs live in: the tests use it with a
/// folder of their own, and a caller that knows its home uses it too.
pub fn usage_in(log: UsageLog, session_ref: &str, from: Ts, to: Ts, home: &Path) -> Option<Usage> {
    match log {
        UsageLog::PiJsonl => pi_jsonl(&PathBuf::from(session_ref), from, to),
        UsageLog::CodexRollout => codex_rollout(session_ref, from, to, home),
        UsageLog::ClaudeJsonl => claude_jsonl(session_ref, from, to, home),
        UsageLog::OpencodeDb => opencode_db(session_ref, from, to, home),
    }
}

/// `input + cache_read + cache_write` of the last assistant record of the
/// session (§3.5): the context the agent carries.
///
/// None: the log cannot be read, or it has no assistant record.
pub fn last_context(log: UsageLog, session_ref: &str) -> Option<i64> {
    last_context_in(log, session_ref, &home())
}

/// The same, with the home folder the logs live in.
pub fn last_context_in(log: UsageLog, session_ref: &str, home: &Path) -> Option<i64> {
    match log {
        UsageLog::PiJsonl => pi_last_context(&PathBuf::from(session_ref)),
        UsageLog::CodexRollout => codex_last_context(session_ref, home),
        UsageLog::ClaudeJsonl => claude_last_context(session_ref, home),
        UsageLog::OpencodeDb => opencode_last_context(session_ref, home),
    }
}

// ---------------------------------------------------------------- the logs

/// pi: the session file, one JSON object per line. A record is a message of
/// the assistant that carries its usage.
fn pi_jsonl(path: &Path, from: Ts, to: Ts) -> Option<Usage> {
    let lines = read_lines(path)?;
    let mut totals = Totals::default();
    for line in lines {
        let Some(value) = parse(&line) else { continue };
        if value.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let Some(time) = record_time(&value, message) else {
            continue;
        };
        if time < from || time >= to {
            continue;
        }
        totals.input += number(usage, "input");
        totals.output += number(usage, "output");
        totals.cache_read += number(usage, "cacheRead");
        totals.cache_write += number(usage, "cacheWrite");
        totals.reasoning += number(usage, "reasoning");
        totals.add_cost(
            usage
                .get("cost")
                .and_then(|cost| cost.get("total"))
                .and_then(Value::as_f64),
        );
        totals.records += 1;
    }
    Some(totals.usage())
}

/// pi: the context of the last assistant record (§3.5).
fn pi_last_context(path: &Path) -> Option<i64> {
    let lines = read_lines(path)?;
    let mut context = None;
    for line in lines {
        let Some(value) = parse(&line) else { continue };
        if value.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = message.get("usage") else {
            continue;
        };
        context =
            Some(number(usage, "input") + number(usage, "cacheRead") + number(usage, "cacheWrite"));
    }
    context
}

/// Codex: the rollout file of a thread. Every token count carries the totals
/// of the session so far, so the record is the **difference** to the previous
/// one — the same total is never counted twice.
fn codex_rollout(thread: &str, from: Ts, to: Ts, home: &Path) -> Option<Usage> {
    let path = codex_rollout_path(home, thread)?;
    let lines = read_lines(&path)?;
    let mut totals = Totals::default();
    let mut previous: Option<CodexTokens> = None;
    for line in lines {
        let Some(value) = parse(&line) else { continue };
        if value.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        let Some(info) = payload.get("info") else {
            continue;
        };
        // A count whose info is null or has no object of totals is not a
        // count: it changes neither the totals nor the predecessor (§4.3).
        let Some(current) = CodexTokens::of(info) else {
            continue;
        };
        // The first valid total counts against zero: it is the work of the
        // session so far. The predecessor moves before the window is looked
        // at, so a count before the window still moves it.
        let before = previous.unwrap_or_else(CodexTokens::zero);
        let delta = current.difference(before);
        previous = Some(current);
        let Some(time) = record_time(&value, &value) else {
            continue;
        };
        if time < from || time >= to {
            continue;
        }
        // Codex counts the cached input in its input: the two columns must
        // not count it twice (§4.3).
        totals.input += delta.input_tokens - delta.cached_input_tokens;
        totals.cache_read += delta.cached_input_tokens;
        totals.cache_write += delta.cache_write_input_tokens;
        totals.output += delta.output_tokens;
        totals.reasoning += delta.reasoning_output_tokens;
        totals.records += 1;
    }
    Some(totals.usage())
}

/// Codex: the context of the last token count (§3.5).
///
/// The context is the input of the last assistant turn with the cached input
/// counted once, plus what was written to the cache. Codex says it in
/// `info.last_token_usage` (§4.3); when the line does not carry it, the
/// difference to the count before it is used. A count that repeats the totals
/// of the one before it adds nothing and **keeps** the context of the last
/// turn that had one.
fn codex_last_context(thread: &str, home: &Path) -> Option<i64> {
    let path = codex_rollout_path(home, thread)?;
    let lines = read_lines(&path)?;
    let mut context = None;
    let mut previous: Option<CodexTokens> = None;
    for line in lines {
        let Some(value) = parse(&line) else { continue };
        if value.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        let Some(info) = payload.get("info") else {
            continue;
        };
        // A count whose info is null or has no object of totals is not a
        // count: it changes neither the context nor the predecessor (§4.3).
        let Some(current) = CodexTokens::of(info) else {
            continue;
        };
        let before = previous.unwrap_or_else(CodexTokens::zero);
        let delta = current.difference(before);
        previous = Some(current);
        if let Some(last) = info.get("last_token_usage").filter(|last| last.is_object()) {
            // The context of the request itself: its input (the cached input
            // is in it) plus what was written to the cache.
            context = Some(number(last, "input_tokens") + number(last, "cache_write_input_tokens"));
        } else if delta.input_tokens != 0
            || delta.cached_input_tokens != 0
            || delta.cache_write_input_tokens != 0
            || delta.output_tokens != 0
            || delta.reasoning_output_tokens != 0
        {
            // No `last_token_usage`: the difference to the count before it is
            // the turn.
            context = Some(delta.input_tokens + delta.cache_write_input_tokens);
        }
        // A repeated total adds nothing: the context of the last turn that
        // had one stays.
    }
    context
}

/// Claude: the session file under `~/.claude/projects/*/`. One message is
/// written on several lines, so it is counted once per `message.id`.
fn claude_jsonl(session: &str, from: Ts, to: Ts, home: &Path) -> Option<Usage> {
    let path = claude_json_path(home, session)?;
    let lines = read_lines(&path)?;
    let mut totals = Totals::default();
    let mut seen: HashSet<String> = HashSet::new();
    for line in lines {
        let Some(value) = parse(&line) else { continue };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage") else {
            continue;
        };
        // One message, one count (§4.3).
        let id = message
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| value.get("uuid").and_then(Value::as_str));
        if let Some(id) = id {
            if !seen.insert(id.to_string()) {
                continue;
            }
        }
        let Some(time) = record_time(&value, &value) else {
            continue;
        };
        if time < from || time >= to {
            continue;
        }
        totals.input += number(usage, "input_tokens");
        totals.cache_read += number(usage, "cache_read_input_tokens");
        totals.cache_write += number(usage, "cache_creation_input_tokens");
        totals.output += number(usage, "output_tokens");
        totals.records += 1;
    }
    Some(totals.usage())
}

/// Claude: the context of the last assistant record (§3.5).
fn claude_last_context(session: &str, home: &Path) -> Option<i64> {
    let path = claude_json_path(home, session)?;
    let lines = read_lines(&path)?;
    let mut context = None;
    let mut seen: HashSet<String> = HashSet::new();
    for line in lines {
        let Some(value) = parse(&line) else { continue };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let id = message
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| value.get("uuid").and_then(Value::as_str));
        if let Some(id) = id {
            if !seen.insert(id.to_string()) {
                continue;
            }
        }
        context = Some(
            number(usage, "input_tokens")
                + number(usage, "cache_read_input_tokens")
                + number(usage, "cache_creation_input_tokens"),
        );
    }
    context
}

/// OpenCode: the `session_message` table of its database. A row's `data`
/// carries the tokens of the message and its cost.
fn opencode_db(session: &str, from: Ts, to: Ts, home: &Path) -> Option<Usage> {
    let rows = opencode_messages(home, session)?;
    let mut totals = Totals::default();
    for (data, time) in rows {
        let Some(time) = time else { continue };
        if time < from || time >= to {
            continue;
        }
        let Some(tokens) = data.get("tokens") else {
            continue;
        };
        let cache = tokens.get("cache");
        totals.input += number(tokens, "input");
        totals.output += number(tokens, "output");
        totals.reasoning += number(tokens, "reasoning");
        totals.cache_read += cache.map(|cache| number(cache, "read")).unwrap_or(0);
        totals.cache_write += cache.map(|cache| number(cache, "write")).unwrap_or(0);
        totals.add_cost(data.get("cost").and_then(Value::as_f64));
        totals.records += 1;
    }
    Some(totals.usage())
}

/// OpenCode: the context of the last message with tokens (§3.5).
fn opencode_last_context(session: &str, home: &Path) -> Option<i64> {
    let mut rows = opencode_messages(home, session)?;
    // The messages are taken in the order they were completed: the rows of
    // the table need not be in that order.
    rows.sort_by_key(|(_, time)| time.unwrap_or(0));
    let mut context = None;
    for (data, time) in rows {
        if time.is_none() {
            continue;
        }
        let Some(tokens) = data.get("tokens") else {
            continue;
        };
        let cache = tokens.get("cache");
        context = Some(
            number(tokens, "input")
                + cache.map(|cache| number(cache, "read")).unwrap_or(0)
                + cache.map(|cache| number(cache, "write")).unwrap_or(0),
        );
    }
    context
}

/// The messages of a session in the OpenCode database: their data and the
/// time they were completed at.
fn opencode_messages(home: &Path, session: &str) -> Option<Vec<(Value, Option<Ts>)>> {
    let path = home
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db");
    let conn = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()?;
    let columns = table_columns(&conn, "session_message")?;
    let session_column = pick(&columns, &["sessionID", "session_id", "session"])?;
    let data_column = pick(&columns, &["data"])?;
    // The time of the message is in its JSON (`data.time.completed`, §4.3):
    // the table needs no column of its own for it.
    let sql = format!("SELECT {data_column} FROM session_message WHERE {session_column} = ?1");
    let mut stmt = conn.prepare(&sql).ok()?;
    let rows = stmt
        .query_map(rusqlite::params![session], |row| {
            let data: rusqlite::types::Value = row.get(0)?;
            Ok(data)
        })
        .ok()?;
    let mut messages = Vec::new();
    for row in rows {
        let data = row.ok()?;
        let Some(data) = json_from_sql(&data) else {
            continue;
        };
        let time = data
            .get("time")
            .and_then(|time| time.get("completed"))
            .and_then(timestamp);
        messages.push((data, time));
    }
    Some(messages)
}

/// The JSON of a column of the OpenCode database.
fn json_from_sql(value: &rusqlite::types::Value) -> Option<Value> {
    match value {
        rusqlite::types::Value::Text(text) => serde_json::from_str(text).ok(),
        rusqlite::types::Value::Blob(bytes) => serde_json::from_slice(bytes).ok(),
        _ => None,
    }
}

/// The columns of a table of the OpenCode database.
fn table_columns(conn: &rusqlite::Connection, table: &str) -> Option<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).ok()?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .ok()?
        .collect::<Result<Vec<String>, _>>()
        .ok()?;
    Some(rows)
}

/// The first of the names that the table has.
fn pick(columns: &[String], names: &[&str]) -> Option<String> {
    names
        .iter()
        .find(|name| columns.iter().any(|column| column == *name))
        .map(|name| name.to_string())
}

/// The rollout file of a Codex thread: the newest one of the last fourteen
/// days under `~/.codex/sessions/YYYY/MM/DD`.
fn codex_rollout_path(home: &Path, thread: &str) -> Option<PathBuf> {
    // A session ref that is a file is that file.
    let direct = PathBuf::from(thread);
    if direct.is_file() {
        return Some(direct);
    }
    let root = home.join(".codex").join("sessions");
    let now = now_epoch();
    let mut newest: Option<(i64, PathBuf)> = None;
    for (day, dir) in codex_day_dirs(&root) {
        let age = now - day;
        if !(-86_400..14 * 86_400).contains(&age) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("rollout-") || !name.ends_with(&format!("-{thread}.jsonl")) {
                continue;
            }
            let path = entry.path();
            if newest.as_ref().is_none_or(|(best, _)| day > *best) {
                newest = Some((day, path));
            }
        }
    }
    newest.map(|(_, path)| path)
}

/// The day directories of the Codex sessions: the day (its epoch second) and
/// its directory.
fn codex_day_dirs(root: &Path) -> Vec<(i64, PathBuf)> {
    let mut days = Vec::new();
    for year in entries_of(root) {
        for month in entries_of(&year) {
            for day in entries_of(&month) {
                let Some(day_epoch) = day_epoch(&day) else {
                    continue;
                };
                days.push((day_epoch, day));
            }
        }
    }
    days
}

/// The directories of a folder.
fn entries_of(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// The epoch second of a day directory (`YYYY/MM/DD`).
fn day_epoch(dir: &Path) -> Option<i64> {
    let year: i64 = dir
        .parent()?
        .parent()?
        .file_name()?
        .to_str()?
        .parse()
        .ok()?;
    let month: i64 = dir.parent()?.file_name()?.to_str()?.parse().ok()?;
    let day: i64 = dir.file_name()?.to_str()?.parse().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

/// The directory of the Codex sessions of a day: `~/.codex/sessions/YYYY/MM/DD`.
pub fn codex_day_dir(home: &Path, day: Ts) -> PathBuf {
    let (year, month, date) = civil_from_epoch(day);
    home.join(".codex")
        .join("sessions")
        .join(format!("{year:04}/{month:02}/{date:02}"))
}

/// The session file of a Claude session: `~/.claude/projects/*/<session>.jsonl`.
fn claude_json_path(home: &Path, session: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(session);
    if direct.is_file() {
        return Some(direct);
    }
    let root = home.join(".claude").join("projects");
    for project in entries_of(&root) {
        let candidate = project.join(format!("{session}.jsonl"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

// ---------------------------------------------------------------- the sums

/// The running sum of the records of a session.
#[derive(Debug, Default)]
struct Totals {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    cost: Option<f64>,
    records: i64,
}

impl Totals {
    /// A cost of a record adds to the sum; a record without a cost leaves it.
    fn add_cost(&mut self, cost: Option<f64>) {
        if let Some(cost) = cost {
            self.cost = Some(self.cost.unwrap_or(0.0) + cost);
        }
    }

    fn usage(&self) -> Usage {
        Usage {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
            reasoning: self.reasoning,
            cost_usd: self.cost,
            records: self.records,
        }
    }
}

/// The totals Codex carries in a token count line.
#[derive(Debug, Clone, Copy)]
struct CodexTokens {
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
}

impl CodexTokens {
    /// The totals of a count. None when the count carries none: an `info`
    /// that is null or that has no object of totals is not a count (§4.3).
    fn of(info: &Value) -> Option<CodexTokens> {
        if !info.is_object() {
            return None;
        }
        let total = info.get("total_token_usage")?;
        if !total.is_object() {
            return None;
        }
        Some(CodexTokens {
            input_tokens: number(total, "input_tokens"),
            cached_input_tokens: number(total, "cached_input_tokens"),
            cache_write_input_tokens: number(total, "cache_write_input_tokens"),
            output_tokens: number(total, "output_tokens"),
            reasoning_output_tokens: number(total, "reasoning_output_tokens"),
        })
    }

    /// The totals of nothing: the predecessor of the first count of a file.
    fn zero() -> CodexTokens {
        CodexTokens {
            input_tokens: 0,
            cached_input_tokens: 0,
            cache_write_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
        }
    }

    /// The difference to the count before it: what this line added.
    fn difference(self, previous: CodexTokens) -> CodexTokens {
        CodexTokens {
            input_tokens: self.input_tokens - previous.input_tokens,
            cached_input_tokens: self.cached_input_tokens - previous.cached_input_tokens,
            cache_write_input_tokens: self.cache_write_input_tokens
                - previous.cache_write_input_tokens,
            output_tokens: self.output_tokens - previous.output_tokens,
            reasoning_output_tokens: self.reasoning_output_tokens
                - previous.reasoning_output_tokens,
        }
    }
}

// ---------------------------------------------------------------- the small things

/// The lines of a file; None when it cannot be read.
fn read_lines(path: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.lines().map(|line| line.to_string()).collect())
}

/// One line as JSON; None when the line is not JSON.
fn parse(line: &str) -> Option<Value> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    serde_json::from_str(line).ok()
}

/// A number of a JSON object; 0 when it is not there.
fn number(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// The time of a record: its own `timestamp`, or the one of its message.
fn record_time(value: &Value, message: &Value) -> Option<Ts> {
    value
        .get("timestamp")
        .and_then(timestamp)
        .or_else(|| message.get("timestamp").and_then(timestamp))
}

/// The time of a record in epoch seconds.
///
/// A number is read as seconds or as milliseconds (the CLIs differ); a string
/// is an RFC 3339 time.
fn timestamp(value: &Value) -> Option<Ts> {
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

/// A number of time in epoch seconds: milliseconds when it is large enough
/// to be milliseconds.
fn number_to_seconds(value: f64) -> Ts {
    if value.abs() >= 100_000_000_000.0 {
        (value / 1000.0) as i64
    } else {
        value as i64
    }
}

/// An RFC 3339 time (`2026-10-26T12:00:00.000Z`) in epoch seconds.
fn rfc3339(text: &str) -> Option<Ts> {
    let text = text.trim();
    if text.len() < 19 {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: i64 = text.get(5..7)?.parse().ok()?;
    let day: i64 = text.get(8..10)?.parse().ok()?;
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    let second: i64 = text.get(17..19)?.parse().ok()?;
    let rest = &text[19..];
    let rest = match rest.strip_prefix('.') {
        Some(fraction) => {
            let digits = fraction.chars().take_while(|c| c.is_ascii_digit()).count();
            &fraction[digits..]
        }
        None => rest,
    };
    let offset = if rest.eq_ignore_ascii_case("z") {
        0
    } else if rest.starts_with('+') || rest.starts_with('-') {
        let sign = if rest.starts_with('-') { -1 } else { 1 };
        let hours: i64 = rest.get(1..3)?.parse().ok()?;
        let minutes: i64 = rest.get(4..6)?.parse().ok()?;
        sign * (hours * 3600 + minutes * 60)
    } else {
        0
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// The days since the epoch of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_position = (month + 9) % 12;
    let day_of_year = (153 * month_position + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The civil date of the days since the epoch.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

/// The civil date of a moment in epoch seconds.
fn civil_from_epoch(epoch: Ts) -> (i64, u32, u32) {
    civil_from_days(epoch.div_euclid(86_400))
}

/// Now, in epoch seconds.
fn now_epoch() -> Ts {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// The home folder of the user; an empty path when there is none.
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// One usage-bearing record, retaining its model for per-record pricing.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    pub model: Option<String>,
    pub usage: Usage,
}

/// Records in the same half-open window as [`usage`], or None for an unreadable log.
pub fn usage_records(log: UsageLog, session: &str, from: Ts, to: Ts) -> Option<Vec<UsageRecord>> {
    usage_records_in(log, session, from, to, &home())
}

/// The record-level reader with an explicit home directory.
pub fn usage_records_in(
    log: UsageLog,
    session: &str,
    from: Ts,
    to: Ts,
    home: &Path,
) -> Option<Vec<UsageRecord>> {
    let mut result = Vec::new();
    if log == UsageLog::OpencodeDb {
        for (data, time) in opencode_messages(home, session)? {
            let Some(time) = time else { continue };
            if time < from || time >= to {
                continue;
            }
            let Some(tokens) = data.get("tokens") else {
                continue;
            };
            let cache = tokens.get("cache");
            let model = model_name(&data);
            result.push(UsageRecord {
                model,
                usage: Usage {
                    input: number(tokens, "input"),
                    output: number(tokens, "output"),
                    reasoning: number(tokens, "reasoning"),
                    cache_read: cache.map(|c| number(c, "read")).unwrap_or(0),
                    cache_write: cache.map(|c| number(c, "write")).unwrap_or(0),
                    cost_usd: data.get("cost").and_then(Value::as_f64),
                    records: 1,
                },
            });
        }
        return Some(result);
    }
    let path = match log {
        UsageLog::PiJsonl => PathBuf::from(session),
        UsageLog::CodexRollout => codex_rollout_path(home, session)?,
        UsageLog::ClaudeJsonl => claude_json_path(home, session)?,
        UsageLog::OpencodeDb => unreachable!(),
    };
    let mut previous = CodexTokens::zero();
    let mut seen = HashSet::new();
    let mut current_model = None;
    for line in read_lines(&path)? {
        let Some(value) = parse(&line) else { continue };
        match log {
            UsageLog::PiJsonl => {
                if value.get("type").and_then(Value::as_str) != Some("message") {
                    continue;
                }
                let Some(message) = value.get("message") else {
                    continue;
                };
                if message.get("role").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                let Some(u) = message.get("usage") else {
                    continue;
                };
                let Some(t) = record_time(&value, message) else {
                    continue;
                };
                if t < from || t >= to {
                    continue;
                }
                result.push(UsageRecord {
                    model: model_name(message),
                    usage: Usage {
                        input: number(u, "input"),
                        output: number(u, "output"),
                        cache_read: number(u, "cacheRead"),
                        cache_write: number(u, "cacheWrite"),
                        reasoning: number(u, "reasoning"),
                        cost_usd: u
                            .get("cost")
                            .and_then(|c| c.get("total"))
                            .and_then(Value::as_f64),
                        records: 1,
                    },
                });
            }
            UsageLog::ClaudeJsonl => {
                if value.get("type").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                let Some(message) = value.get("message") else {
                    continue;
                };
                let Some(u) = message.get("usage") else {
                    continue;
                };
                if let Some(id) = message
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("uuid").and_then(Value::as_str))
                {
                    if !seen.insert(id.to_owned()) {
                        continue;
                    }
                }
                let Some(t) = record_time(&value, &value) else {
                    continue;
                };
                if t < from || t >= to {
                    continue;
                }
                result.push(UsageRecord {
                    model: model_name(message),
                    usage: Usage {
                        input: number(u, "input_tokens"),
                        output: number(u, "output_tokens"),
                        cache_read: number(u, "cache_read_input_tokens"),
                        cache_write: number(u, "cache_creation_input_tokens"),
                        reasoning: 0,
                        cost_usd: None,
                        records: 1,
                    },
                });
            }
            UsageLog::CodexRollout => {
                if value.get("type").and_then(Value::as_str) == Some("turn_context") {
                    current_model = value.get("payload").and_then(model_name).or(current_model);
                    continue;
                }
                if value.get("type").and_then(Value::as_str) != Some("event_msg") {
                    continue;
                }
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                if payload.get("type").and_then(Value::as_str) != Some("token_count") {
                    continue;
                }
                let Some(current) = payload.get("info").and_then(CodexTokens::of) else {
                    continue;
                };
                let delta = current.difference(previous);
                previous = current;
                let Some(t) = record_time(&value, &value) else {
                    continue;
                };
                if t < from || t >= to {
                    continue;
                }
                result.push(UsageRecord {
                    model: model_name(payload).or_else(|| current_model.clone()),
                    usage: Usage {
                        input: delta.input_tokens - delta.cached_input_tokens,
                        output: delta.output_tokens,
                        cache_read: delta.cached_input_tokens,
                        cache_write: delta.cache_write_input_tokens,
                        reasoning: delta.reasoning_output_tokens,
                        cost_usd: None,
                        records: 1,
                    },
                });
            }
            UsageLog::OpencodeDb => unreachable!(),
        }
    }
    Some(result)
}

fn model_name(value: &Value) -> Option<String> {
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| value.get("modelID").and_then(Value::as_str))?;
    let provider = value
        .get("provider")
        .and_then(Value::as_str)
        .or_else(|| value.get("providerID").and_then(Value::as_str));
    Some(match provider {
        Some(p) if !model.contains('/') => format!("{p}/{model}"),
        _ => model.to_owned(),
    })
}

/// Prices in USD per million tokens for one model.
#[derive(Debug, Clone, Copy)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// Aggregate tokens and their complete cost for a round window.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundUsage {
    pub usage: Usage,
    pub cost_source: &'static str,
}

/// Count a round without persistence; suitable for live board and report reads.
/// A session whose log cannot be read is skipped: pi may open a session and
/// replace it at once, so a file that was never written says nothing about the
/// round. The token count is unknown only when no session was readable at all.
/// A missing price makes the complete cost unknown while preserving the token
/// totals.
pub fn round_usage(
    log: UsageLog,
    sessions: &[String],
    from: Ts,
    to: Ts,
    default_model: Option<&str>,
    prices: &std::collections::BTreeMap<String, Price>,
) -> Option<RoundUsage> {
    if sessions.is_empty() {
        return None;
    }
    let mut seen = HashSet::new();
    let mut readable = 0usize;
    let mut total = Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        reasoning: 0,
        cost_usd: None,
        records: 0,
    };
    let (mut cost, mut any_cost, mut known, mut priced) = (0.0, false, true, false);
    for session in sessions {
        if !seen.insert(session) {
            continue;
        }
        let Some(u) = usage(log, session, from, to) else {
            continue;
        };
        let Some(records) = usage_records(log, session, from, to) else {
            continue;
        };
        readable += 1;
        total.input += u.input;
        total.output += u.output;
        total.cache_read += u.cache_read;
        total.cache_write += u.cache_write;
        total.reasoning += u.reasoning;
        total.records += u.records;
        let mut check = Usage {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            cost_usd: None,
            records: 0,
        };
        for record in records {
            let u = record.usage;
            check.input += u.input;
            check.output += u.output;
            check.cache_read += u.cache_read;
            check.cache_write += u.cache_write;
            check.reasoning += u.reasoning;
            check.records += u.records;
            if let Some(c) = u.cost_usd {
                check.cost_usd = Some(check.cost_usd.unwrap_or(0.0) + c);
            }
            if let Some(amount) = u.cost_usd {
                cost += amount;
                any_cost = true;
                continue;
            }
            let model = record
                .model
                .as_deref()
                .map(|model| {
                    default_model
                        .filter(|full| {
                            !model.contains('/')
                                && full.rsplit_once('/').is_some_and(|(_, id)| id == model)
                        })
                        .unwrap_or(model)
                })
                .or(default_model);
            let price = model.and_then(|m| {
                prices.get(m).or_else(|| {
                    if m.contains('/') {
                        return None;
                    }
                    let mut candidates = prices
                        .iter()
                        .filter(|(key, _)| key.rsplit_once('/').is_some_and(|(_, id)| id == m));
                    let (_, price) = candidates.next()?;
                    if candidates.next().is_some() {
                        None
                    } else {
                        Some(price)
                    }
                })
            });
            if let Some(p) = price {
                cost += (u.input as f64 * p.input
                    + u.output as f64 * p.output
                    + u.cache_read as f64 * p.cache_read
                    + u.cache_write as f64 * p.cache_write)
                    / 1_000_000.0;
                any_cost = true;
                priced = true;
            } else {
                known = false;
            }
        }
        // A live log may change between the total and record reads. Retry
        // later rather than combining tokens and cost from different snapshots.
        if check != u {
            return None;
        }
    }
    if readable == 0 {
        return None;
    }
    let cost_source = if any_cost && known {
        total.cost_usd = Some(cost);
        if priced {
            "prices"
        } else {
            "agent"
        }
    } else {
        "none"
    };
    Some(RoundUsage {
        usage: total,
        cost_source,
    })
}
