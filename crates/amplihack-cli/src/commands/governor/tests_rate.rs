//! Issue #1811 (TDD): burn rate, sustainable rate, forecast and the rate
//! signal (D4, D6).
//!
//! Wire with `#[cfg(test)] mod tests_rate;` in `governor/mod.rs`.
//!
//! Expected API in `rate`:
//! - `burn_rate(&[Sample]) -> Option<f64>`: points per hour between the
//!   oldest and newest sample in the newest sample's reset window; `None`
//!   with fewer than two samples in that window, no elapsed time or a
//!   falling value.
//! - `sustainable(remaining: f64, resets_at: Option<i64>, now) -> Option<f64>`
//! - `forecast(remaining: f64, burn: Option<f64>, now) -> Option<i64>`
//! - `WindowRate { burn, sustainable, forecast, resets_at }` and
//!   `signal(&[WindowRate], band_pct: u32) -> RateSignal`
//! - `next_limit(limit, RateSignal, step, max) -> u32`, always in `1..=max`.
//!
//! `model::Sample { observed_at, used_pct, resets_at: Option<i64> }`.

use super::model::Sample;
use super::rate::{RateSignal, WindowRate, burn_rate, forecast, next_limit, signal, sustainable};

const NOW: i64 = 1_800_000_000;
const HOUR: i64 = 3600;

fn s(ago_h: i64, used: f64) -> Sample {
    Sample {
        observed_at: NOW - ago_h * HOUR,
        used_pct: used,
        resets_at: Some(NOW + 48 * HOUR),
    }
}

fn w(burn: Option<f64>, sust: Option<f64>) -> WindowRate {
    WindowRate {
        burn,
        sustainable: sust,
        forecast: None,
        resets_at: Some(NOW + 10 * HOUR),
    }
}

#[test]
fn burn_rate_is_points_per_hour_between_oldest_and_newest() {
    assert_eq!(burn_rate(&[s(2, 10.0), s(1, 12.0), s(0, 14.0)]), Some(2.0));
    // Order of the slice does not matter.
    assert_eq!(burn_rate(&[s(0, 14.0), s(2, 10.0)]), Some(2.0));
}

#[test]
fn burn_rate_is_unknown_with_fewer_than_two_samples() {
    assert_eq!(burn_rate(&[]), None);
    assert_eq!(burn_rate(&[s(1, 10.0)]), None);
}

#[test]
fn burn_rate_is_unknown_without_elapsed_time() {
    assert_eq!(burn_rate(&[s(1, 10.0), s(1, 12.0)]), None);
}

#[test]
fn burn_rate_is_unknown_across_a_reset() {
    let mut later = s(0, 3.0);
    later.resets_at = Some(NOW + 200 * HOUR);
    assert_eq!(burn_rate(&[s(2, 1.0), later]), None);
}

#[test]
fn burn_rate_after_a_reset_uses_only_the_current_window() {
    // A long history from the previous window must not hide the rate of the
    // current one: two readings after the reset are enough.
    let new_reset = Some(NOW + 200 * HOUR);
    let mut history: Vec<Sample> = (3..60).map(|h| s(h, 90.0 - h as f64)).collect();
    let mut a = s(2, 1.0);
    a.resets_at = new_reset;
    let mut b = s(0, 5.0);
    b.resets_at = new_reset;
    history.push(b);
    history.insert(10, a);
    assert_eq!(burn_rate(&history), Some(2.0));
}

#[test]
fn burn_rate_is_unknown_when_usage_falls() {
    assert_eq!(burn_rate(&[s(2, 50.0), s(0, 10.0)]), None);
}

#[test]
fn burn_rate_ignores_non_finite_samples() {
    let r = burn_rate(&[s(2, f64::NAN), s(0, 10.0)]);
    assert!(r.is_none_or(f64::is_finite));
}

#[test]
fn sustainable_is_remaining_over_hours_to_reset() {
    assert_eq!(sustainable(20.0, Some(NOW + 10 * HOUR), NOW), Some(2.0));
    assert_eq!(sustainable(20.0, Some(NOW), NOW), None);
    assert_eq!(sustainable(20.0, Some(NOW - 1), NOW), None);
    assert_eq!(sustainable(20.0, None, NOW), None);
}

#[test]
fn forecast_is_now_plus_remaining_over_burn() {
    assert_eq!(forecast(10.0, Some(2.0), NOW), Some(NOW + 5 * HOUR));
    assert_eq!(forecast(10.0, Some(0.0), NOW), None);
    assert_eq!(forecast(10.0, None, NOW), None);
    assert_eq!(forecast(10.0, Some(f64::NAN), NOW), None);
}

#[test]
fn forecast_never_overflows() {
    let f = forecast(100.0, Some(f64::MIN_POSITIVE), i64::MAX - 10);
    assert!(f.is_none() || f.unwrap() >= i64::MAX - 10);
}

#[test]
fn signal_band_is_ten_percent_by_default() {
    assert_eq!(signal(&[w(Some(1.11), Some(1.0))], 10), RateSignal::Over);
    assert_eq!(signal(&[w(Some(1.10), Some(1.0))], 10), RateSignal::Steady);
    assert_eq!(signal(&[w(Some(0.90), Some(1.0))], 10), RateSignal::Steady);
    assert_eq!(signal(&[w(Some(0.89), Some(1.0))], 10), RateSignal::Under);
}

#[test]
fn signal_over_in_any_window_wins() {
    let ws = [w(Some(0.1), Some(1.0)), w(Some(5.0), Some(1.0))];
    assert_eq!(signal(&ws, 10), RateSignal::Over);
}

#[test]
fn signal_under_needs_every_window_under() {
    let ws = [w(Some(0.1), Some(1.0)), w(Some(1.0), Some(1.0))];
    assert_eq!(signal(&ws, 10), RateSignal::Steady);
}

#[test]
fn forecast_before_reset_is_over() {
    let mut win = w(Some(0.5), Some(1.0));
    win.forecast = Some(NOW + HOUR);
    assert_eq!(signal(&[win], 10), RateSignal::Over);
}

#[test]
fn no_burn_rate_is_unknown() {
    assert_eq!(signal(&[], 10), RateSignal::Unknown);
    assert_eq!(signal(&[w(None, Some(1.0))], 10), RateSignal::Unknown);
}

#[test]
fn nan_never_decides() {
    for win in [
        w(Some(f64::NAN), Some(1.0)),
        w(Some(1.0), Some(f64::NAN)),
        w(Some(f64::INFINITY), Some(f64::INFINITY)),
    ] {
        assert_eq!(signal(&[win], 10), RateSignal::Unknown);
    }
}

#[test]
fn next_limit_steps_and_clamps() {
    assert_eq!(next_limit(3, RateSignal::Over, 1, 4), 2);
    assert_eq!(next_limit(1, RateSignal::Over, 1, 4), 1);
    assert_eq!(next_limit(2, RateSignal::Over, 8, 4), 1);
    assert_eq!(next_limit(3, RateSignal::Under, 1, 4), 4);
    assert_eq!(next_limit(4, RateSignal::Under, 1, 4), 4);
    assert_eq!(next_limit(1, RateSignal::Under, 8, 4), 4);
    assert_eq!(next_limit(3, RateSignal::Steady, 1, 4), 3);
    assert_eq!(next_limit(3, RateSignal::Unknown, 1, 4), 3);
}

#[test]
fn next_limit_repairs_out_of_range_input() {
    assert_eq!(next_limit(0, RateSignal::Steady, 1, 4), 1);
    assert_eq!(next_limit(99, RateSignal::Steady, 1, 4), 4);
    assert_eq!(next_limit(u32::MAX, RateSignal::Under, u32::MAX, 64), 64);
}
