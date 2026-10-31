//! The price table (T-484): official prices in USD per million tokens, with
//! peak rates and peak windows, and the matching of a model id to an entry.
//!
//! GEAR ships a built-in table of the model makers' official prices, compiled
//! into the binary; a project's `[prices]` entries override it per model. A
//! model matches case-insensitively on its id without the provider prefix, so
//! `hk-dgx-1/DeepSeek-V4.1-Flash-1017` and
//! `tokenrhythm/deepseek-v4.1-flash-1017` both take the DeepSeek entry.
//!
//! Chinese public holidays (off-peak all day) are not modelled: a holiday
//! weekday is priced at its peak rate when the record falls in a peak window.

use gear_model::Ts;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The built-in table. Keep the prices the makers publish; the comment in the
/// file names the day of the reading and the holidays it does not model.
const BUILTIN: &str = include_str!("prices.toml");

/// The four rates of one price: USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// A weekday and UTC-hour window in which the peak rates apply. `days` has
/// bit 0 for Monday through bit 6 for Sunday; the minutes are minutes after
/// midnight UTC, `start_min <= minute < end_min`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeakWindow {
    pub days: u8,
    pub start_min: u32,
    pub end_min: u32,
}

/// The peak rates of one price and the windows they apply in.
#[derive(Debug, Clone, PartialEq)]
pub struct Peak {
    pub rates: Rates,
    pub windows: Vec<PeakWindow>,
}

/// One model's price: its off-peak (base) rates and, when the maker has a
/// peak, the peak rates and windows.
#[derive(Debug, Clone, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub peak: Option<Peak>,
}

/// The id a model is matched on: lower case, without the provider prefix.
pub fn model_id(model: &str) -> String {
    let id = model.rsplit_once('/').map_or(model, |(_, id)| id);
    id.trim().to_lowercase()
}

/// Parse one peak window: `Mon-Fri 01:00-04:00`, `Mon,Wed 06:00-10:00` or
/// `Sat 00:00-23:59`. Days are Mon..Sun; the hours are UTC.
pub fn parse_window(text: &str) -> Option<PeakWindow> {
    let (days, hours) = text.trim().split_once(char::is_whitespace)?;
    let days = parse_days(days)?;
    let (start, end) = hours.trim().split_once('-')?;
    let start_min = parse_clock(start)?;
    let end_min = parse_clock(end)?;
    if start_min >= end_min {
        return None;
    }
    Some(PeakWindow {
        days,
        start_min,
        end_min,
    })
}

fn parse_days(text: &str) -> Option<u8> {
    let mut mask = 0u8;
    for part in text.split(',') {
        let part = part.trim();
        match part.split_once('-') {
            Some((from, to)) => {
                let from = weekday(from)?;
                let to = weekday(to)?;
                if from > to {
                    return None;
                }
                for day in from..=to {
                    mask |= 1 << day;
                }
            }
            None => mask |= 1 << weekday(part)?,
        }
    }
    Some(mask)
}

fn weekday(text: &str) -> Option<u8> {
    Some(match text.trim().to_ascii_lowercase().as_str() {
        "mon" => 0,
        "tue" => 1,
        "wed" => 2,
        "thu" => 3,
        "fri" => 4,
        "sat" => 5,
        "sun" => 6,
        _ => return None,
    })
}

fn parse_clock(text: &str) -> Option<u32> {
    let (hour, minute) = text.trim().split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

/// Whether the time falls in one of the windows (UTC).
pub fn in_window(windows: &[PeakWindow], at: Ts) -> bool {
    let days = at.div_euclid(86_400);
    // 1970-01-01 was a Thursday: Monday is 3 days before the epoch day.
    let day = (days + 3).rem_euclid(7) as u8;
    let minute = (at.rem_euclid(86_400) / 60) as u32;
    windows
        .iter()
        .any(|w| w.days & (1 << day) != 0 && w.start_min <= minute && minute < w.end_min)
}

impl Price {
    /// The cost of `usage` at `at`, in USD: the peak rates inside a peak
    /// window, the base (off-peak) rates every other time.
    pub fn cost(&self, usage: &crate::Usage, at: Ts) -> f64 {
        let rates = match &self.peak {
            Some(peak) if in_window(&peak.windows, at) => peak.rates,
            _ => Rates {
                input: self.input,
                output: self.output,
                cache_read: self.cache_read,
                cache_write: self.cache_write,
            },
        };
        (usage.input as f64 * rates.input
            + usage.output as f64 * rates.output
            + usage.cache_read as f64 * rates.cache_read
            + usage.cache_write as f64 * rates.cache_write)
            / 1_000_000.0
    }
}

/// The price of a model: a project's override first (matched on the same id),
/// then the built-in table.
pub fn price_for(model: &str, overrides: &BTreeMap<String, Price>) -> Option<Price> {
    let id = model_id(model);
    let mut found = None;
    for (key, price) in overrides {
        if model_id(key) == id {
            found = Some(price.clone());
        }
    }
    if found.is_some() {
        return found;
    }
    builtin()
        .iter()
        .find(|(builtin_id, _)| *builtin_id == id)
        .map(|(_, price)| price.clone())
}

/// The built-in table, parsed once.
pub fn builtin() -> &'static [(String, Price)] {
    static TABLE: OnceLock<Vec<(String, Price)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let raw: RawBuiltin =
            toml::from_str(BUILTIN).expect("the built-in price table is valid TOML");
        raw.models
            .into_iter()
            .map(|model| {
                let windows = model
                    .peak_windows
                    .iter()
                    .map(|window| {
                        parse_window(window).unwrap_or_else(|| {
                            panic!("the built-in price table has an invalid window {window:?}")
                        })
                    })
                    .collect();
                (
                    model_id(&model.id),
                    Price {
                        input: model.off_peak.input,
                        output: model.off_peak.output,
                        cache_read: model.off_peak.cache_read,
                        cache_write: model.off_peak.cache_write,
                        peak: Some(Peak {
                            rates: Rates {
                                input: model.peak.input,
                                output: model.peak.output,
                                cache_read: model.peak.cache_read,
                                cache_write: model.peak.cache_write,
                            },
                            windows,
                        }),
                    },
                )
            })
            .collect()
    })
}

#[derive(Deserialize)]
struct RawBuiltin {
    models: Vec<RawModel>,
}

#[derive(Deserialize)]
struct RawModel {
    id: String,
    off_peak: RawRates,
    peak: RawRates,
    peak_windows: Vec<String>,
}

#[derive(Deserialize)]
struct RawRates {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}
