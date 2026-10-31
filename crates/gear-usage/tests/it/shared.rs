//! The fixtures of the four logs of §4.3: small hand-written files, never a
//! real session file.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The times the fixtures use. They are the epoch seconds of the RFC 3339
/// times of the same names.
pub const T12: i64 = 1_793_016_000; // 2026-10-26T12:00:00Z
pub const T13: i64 = 1_793_019_600; // 2026-10-26T13:00:00Z
pub const T14: i64 = 1_793_023_200; // 2026-10-26T14:00:00Z

/// The RFC 3339 time of an epoch second, for the fixtures that carry a time
/// as a string.
pub fn iso(epoch: i64) -> String {
    let (year, month, day) = civil(epoch.div_euclid(86_400));
    let seconds = epoch.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

/// The civil date of a day since the epoch.
fn civil(days: i64) -> (i64, u32, u32) {
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

/// Write the lines of a JSON file and give its path back.
pub fn write_lines(path: &Path, lines: &[Value]) -> PathBuf {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let text: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    std::fs::write(path, text.join("\n")).unwrap();
    path.to_path_buf()
}

/// Now, in epoch seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}
