//! Rate feedback (D4, D6): burn rate from the governor's own history,
//! sustainable rate, forecast exhaustion, and the signal that moves the
//! concurrency limit. All arithmetic is checked: a non-finite number never
//! decides anything.

use super::model::Sample;

const HOUR: f64 = 3600.0;

/// What rate feedback says about the concurrency limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateSignal {
    /// Burning faster than sustainable: step down.
    Over,
    /// Burning slower than sustainable: step up.
    Under,
    /// Inside the band: hold.
    Steady,
    /// No burn rate: hold.
    Unknown,
}

/// One window's rates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowRate {
    /// Points per hour.
    pub burn: Option<f64>,
    /// Points per hour that would last until the reset.
    pub sustainable: Option<f64>,
    /// When the window runs out at the current burn rate.
    pub forecast: Option<i64>,
    pub resets_at: Option<i64>,
}

/// Points per hour from the oldest to the newest finite sample in the newest
/// sample's reset window. Samples from earlier windows are ignored, so a rate
/// is available again from the second reading after a reset. `None` with
/// fewer than two samples in that window, without elapsed time, or when
/// usage fell.
pub fn burn_rate(samples: &[Sample]) -> Option<f64> {
    let valid = || samples.iter().filter(|s| s.used_pct.is_finite());
    let newest = valid().max_by_key(|s| s.observed_at)?;
    let current = || valid().filter(|s| s.resets_at == newest.resets_at);
    if current().count() < 2 {
        return None;
    }
    let oldest = current().min_by_key(|s| s.observed_at)?;
    let elapsed = newest.observed_at.checked_sub(oldest.observed_at)?;
    if elapsed <= 0 {
        return None;
    }
    let delta = newest.used_pct - oldest.used_pct;
    if delta < 0.0 {
        return None;
    }
    let rate = delta / (elapsed as f64 / HOUR);
    rate.is_finite().then_some(rate)
}

/// `remaining` points spread over the hours until `resets_at`.
pub fn sustainable(remaining: f64, resets_at: Option<i64>, now: i64) -> Option<f64> {
    let left = resets_at?.checked_sub(now)?;
    if left <= 0 || !remaining.is_finite() {
        return None;
    }
    let rate = remaining / (left as f64 / HOUR);
    rate.is_finite().then_some(rate)
}

/// When `remaining` points run out at `burn` points per hour.
pub fn forecast(remaining: f64, burn: Option<f64>, now: i64) -> Option<i64> {
    let burn = burn.filter(|b| b.is_finite() && *b > 0.0)?;
    let secs = remaining / burn * HOUR;
    if !secs.is_finite() || secs < 0.0 || secs >= i64::MAX as f64 {
        return None;
    }
    now.checked_add(secs as i64)
}

/// The signal for a set of windows with a band of `band_pct` percent:
/// `Over` when any window burns above `sustainable × (1 + band)` or runs out
/// before its reset; `Under` when every window burns below
/// `sustainable × (1 − band)`; `Unknown` without a usable window.
pub fn signal(windows: &[WindowRate], band_pct: u32) -> RateSignal {
    let band = f64::from(band_pct.min(100)) / 100.0;
    let usable: Vec<(f64, f64, &WindowRate)> = windows
        .iter()
        .filter_map(|w| {
            let burn = w.burn.filter(|b| b.is_finite())?;
            let sust = w.sustainable.filter(|s| s.is_finite())?;
            Some((burn, sust, w))
        })
        .collect();
    if usable.is_empty() {
        return RateSignal::Unknown;
    }
    let over = usable.iter().any(|(burn, sust, w)| {
        let early = matches!((w.forecast, w.resets_at), (Some(f), Some(r)) if f < r);
        *burn > sust * (1.0 + band) || early
    });
    if over {
        return RateSignal::Over;
    }
    if usable
        .iter()
        .all(|(burn, sust, _)| *burn < sust * (1.0 - band))
    {
        return RateSignal::Under;
    }
    RateSignal::Steady
}

/// The next concurrency limit, always in `1..=max`.
pub fn next_limit(limit: u32, signal: RateSignal, step: u32, max: u32) -> u32 {
    let max = max.max(1);
    let limit = limit.clamp(1, max);
    match signal {
        RateSignal::Over => limit.saturating_sub(step).max(1),
        RateSignal::Under => limit.saturating_add(step).min(max),
        RateSignal::Steady | RateSignal::Unknown => limit,
    }
}
