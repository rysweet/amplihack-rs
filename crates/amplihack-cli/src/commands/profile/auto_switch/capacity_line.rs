//! The login capacity line (issue #1745):
//! `🔑 <login>  5h <bar> <pct>%  7d <bar> <pct>%  · <n> spare`.
//!
//! Pure formatting and the spare rule: no I/O. The percentages come from the
//! status-line input, the spare count from recorded readings. A private bar
//! helper for now; `profile usage` may later share one (#1739).

use super::capacity::future;
use super::usage::{Reading, ReadingSource, Window};

/// A profile is spare while its five-hour window is below this...
pub const SPARE_FIVE_HOUR: f64 = 50.0;
/// ...and its seven-day window below this. Both comparisons are strict.
pub const SPARE_SEVEN_DAY: f64 = 90.0;
const CELLS: usize = 10;

/// Ten cells, filled to the percentage divided by ten and rounded half away
/// from zero (35% fills four). NaN is empty; values below 0 or above 100 are
/// clamped.
fn bar(pct: f64) -> String {
    let filled = if pct.is_nan() {
        0
    } else {
        (pct.clamp(0.0, 100.0) / 10.0).round() as usize
    };
    format!("{}{}", "█".repeat(filled), "░".repeat(CELLS - filled))
}

/// The percentage as text, rounded half away from zero. A value over 100 is
/// printed as reported; NaN and negative values print as 0, infinity as 100.
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

/// The capacity line. Each segment is left out on its own when its value is
/// unknown: the login when it is `None` or empty once control characters
/// are removed (SR9), a window when it is `None`, the spare count when it is
/// `None`. `None` only when no segment remains.
pub fn render(
    login: Option<&str>,
    five_hour: Option<f64>,
    seven_day: Option<f64>,
    spare: Option<usize>,
) -> Option<String> {
    let mut segments = Vec::new();
    let login: Option<String> = login.map(|l| l.chars().filter(|c| !c.is_control()).collect());
    if let Some(login) = login.filter(|l| !l.is_empty()) {
        segments.push(format!("🔑 {login}"));
    }
    for (label, pct) in [("5h", five_hour), ("7d", seven_day)] {
        if let Some(pct) = pct {
            segments.push(format!("{label} {} {}", bar(pct), pct_text(pct)));
        }
    }
    if let Some(n) = spare {
        segments.push(format!("· {n} spare"));
    }
    (!segments.is_empty()).then(|| segments.join("  "))
}

/// Whether `r` records a usage-limit failure with a window still to reset:
/// the login is out of capacity whatever its percentages say.
pub fn is_limited(r: &Reading, now: i64) -> bool {
    r.source == ReadingSource::UsageLimitFailure
        && (future(r.five_hour.as_ref(), now).is_some()
            || future(r.seven_day.as_ref(), now).is_some())
}

/// Whether a candidate with reading `r` is spare (D7): not [`is_limited`],
/// its five-hour window
/// under [`SPARE_FIVE_HOUR`] or absent (a plan with no session window,
/// #1818), and its seven-day window present and under [`SPARE_SEVEN_DAY`].
/// A window whose reset has passed counts as 0%.
pub fn is_spare(r: &Reading, now: i64) -> bool {
    let used = |w: &Window| {
        if w.resets_at <= now {
            0.0
        } else {
            w.used_percentage
        }
    };
    !is_limited(r, now)
        && r.five_hour
            .as_ref()
            .is_none_or(|w| used(w) < SPARE_FIVE_HOUR)
        && r.seven_day
            .as_ref()
            .is_some_and(|w| used(w) < SPARE_SEVEN_DAY)
}
