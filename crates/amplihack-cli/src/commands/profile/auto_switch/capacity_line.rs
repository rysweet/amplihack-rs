//! The login capacity line (issue #1745):
//! `🔑 <login>  5h <bar> <pct>%  7d <bar> <pct>%  · <n> spare`.
//!
//! Pure formatting and counting: no I/O. The percentages come from the
//! status-line input, the spare count from recorded readings. A private bar
//! helper for now; `profile usage` may later share one (#1739).

use super::usage::{Reading, Window};
use std::collections::BTreeMap;

/// A profile is spare while its five-hour window is below this...
pub const SPARE_FIVE_HOUR: f64 = 50.0;
/// ...and its seven-day window below this. Both comparisons are strict.
pub const SPARE_SEVEN_DAY: f64 = 90.0;
const CELLS: usize = 10;

/// Ten cells, one filled per whole ten percent. Total: NaN is empty, values
/// below 0 or above 100 are clamped.
pub fn bar(pct: f64) -> String {
    let filled = if pct.is_nan() {
        0
    } else {
        (pct.clamp(0.0, 100.0) / 10.0).floor() as usize
    };
    format!("{}{}", "█".repeat(filled), "░".repeat(CELLS - filled))
}

/// The percentage as text, rounded half away from zero. Values above 100 are
/// printed as given; NaN and negative values print as 0, infinity as 100.
fn pct_text(pct: f64) -> String {
    let shown = if pct.is_nan() {
        0.0
    } else if pct == f64::INFINITY {
        100.0
    } else {
        pct.max(0.0)
    };
    format!("{}%", shown.round() as i64)
}

/// The capacity line, or `None` when neither window is known. A missing
/// window drops its own segment; a `spare` of `None` drops the spare segment.
pub fn render(
    login: &str,
    five_hour: Option<f64>,
    seven_day: Option<f64>,
    spare: Option<usize>,
) -> Option<String> {
    if five_hour.is_none() && seven_day.is_none() {
        return None;
    }
    let mut line = format!("🔑 {login}");
    for (label, pct) in [("5h", five_hour), ("7d", seven_day)] {
        if let Some(pct) = pct {
            line.push_str(&format!("  {label} {} {}", bar(pct), pct_text(pct)));
        }
    }
    if let Some(n) = spare {
        line.push_str(&format!("  · {n} spare"));
    }
    Some(line)
}

/// How many recorded profiles other than `live` have room in both windows.
/// A window whose reset has passed counts as 0%; a profile with either window
/// missing is not spare. `exists` is asked only about profiles that would
/// otherwise count, so a removed profile costs one stat.
pub fn spare(
    accounts: &BTreeMap<String, Reading>,
    live: &str,
    now: i64,
    exists: impl Fn(&str) -> bool,
) -> usize {
    let used = |w: &Option<Window>| {
        w.map(|w| {
            if w.resets_at <= now {
                0.0
            } else {
                w.used_percentage
            }
        })
    };
    accounts
        .iter()
        .filter(|(name, _)| name.as_str() != live)
        .filter(|(_, r)| {
            matches!(
                (used(&r.five_hour), used(&r.seven_day)),
                (Some(five), Some(seven)) if five < SPARE_FIVE_HOUR && seven < SPARE_SEVEN_DAY
            )
        })
        .filter(|(name, _)| exists(name))
        .count()
}
