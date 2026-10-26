//! `exhausted_reset` (issue #1618): when a step is moved off its inherited
//! profile before the first attempt, the wait that may follow is for the
//! latest reset among the inherited account's exhausted windows, read from
//! the readings, so the retry under that profile is not started early.

use super::rerun::exhausted_reset;
use super::usage::{Reading, ReadingSource, Window};

const NOW: i64 = 1_792_335_600;
const THRESHOLD: u8 = 90;

fn reading(five: Option<(f64, i64)>, seven: Option<(f64, i64)>) -> Reading {
    let w = |(used_percentage, resets_at)| Window {
        used_percentage,
        resets_at,
    };
    Reading {
        five_hour: five.map(w),
        seven_day: seven.map(w),
        observed_at: NOW,
        source: ReadingSource::Statusline,
    }
}

#[test]
fn only_exhausted_windows_count() {
    let r = reading(Some((95.0, NOW + 1800)), Some((40.0, NOW + 86_400)));
    assert_eq!(exhausted_reset(&r, THRESHOLD, NOW), Some(NOW + 1800));
}

#[test]
fn the_latest_exhausted_window_is_the_one_waited_for() {
    let r = reading(Some((95.0, NOW + 1800)), Some((99.0, NOW + 86_400)));
    assert_eq!(exhausted_reset(&r, THRESHOLD, NOW), Some(NOW + 86_400));
}

#[test]
fn no_exhausted_window_or_one_already_reset_gives_none() {
    let below = reading(Some((50.0, NOW + 1800)), None);
    assert_eq!(exhausted_reset(&below, THRESHOLD, NOW), None);
    let past = reading(Some((100.0, NOW - 1)), None);
    assert_eq!(exhausted_reset(&past, THRESHOLD, NOW), None);
}
