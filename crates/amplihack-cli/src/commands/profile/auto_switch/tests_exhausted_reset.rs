//! `exhausted_reset` (issue #1618): when a step is moved off its inherited
//! profile before the first attempt, the wait that may follow is for the
//! latest reset among the inherited account's used-up (100%) windows, read
//! from the readings, so the retry under that profile is not started early.
//! A window over the auto-switch threshold but under 100% does not count.

use super::rerun::exhausted_reset;
use super::usage::{Reading, ReadingSource, Window};

const NOW: i64 = 1_792_335_600;

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
fn only_used_up_windows_count() {
    let r = reading(Some((100.0, NOW + 1800)), Some((40.0, NOW + 86_400)));
    assert_eq!(exhausted_reset(&r, NOW), Some(NOW + 1800));
}

#[test]
fn the_latest_exhausted_window_is_the_one_waited_for() {
    let r = reading(Some((100.0, NOW + 1800)), Some((100.0, NOW + 86_400)));
    assert_eq!(exhausted_reset(&r, NOW), Some(NOW + 86_400));
}

#[test]
fn no_exhausted_window_or_one_already_reset_gives_none() {
    let below = reading(Some((50.0, NOW + 1800)), None);
    assert_eq!(exhausted_reset(&below, NOW), None);
    let past = reading(Some((100.0, NOW - 1)), None);
    assert_eq!(exhausted_reset(&past, NOW), None);
}

/// A weekly window at 92%, over a 90% threshold, with a reset days away:
/// the profile still has capacity, so it gives no reset at all, rather than
/// a `ResetTooFar` that would cancel the wait.
#[test]
fn a_window_over_the_threshold_but_not_used_up_does_not_count() {
    let r = reading(Some((30.0, NOW + 1800)), Some((92.0, NOW + 3 * 86_400)));
    assert_eq!(exhausted_reset(&r, NOW), None);
    let r = reading(Some((100.0, NOW + 1800)), Some((92.0, NOW + 3 * 86_400)));
    assert_eq!(exhausted_reset(&r, NOW), Some(NOW + 1800));
}
