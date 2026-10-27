//! The usage-limit wait's clock (issue #1618).

use std::time::Duration;

/// Wall time, or with the debug-only test seams a virtual clock that starts
/// at `TEST_NOW` and moves only when the wait sleeps.
pub(super) struct Clock {
    base: Option<i64>,
    scale: f64,
    /// Virtual time slept minus real time slept.
    skew: Duration,
}

impl Clock {
    pub(super) fn new() -> Self {
        let (base, scale) = seams();
        Self {
            base,
            scale,
            skew: Duration::ZERO,
        }
    }

    pub(super) fn now(&self) -> i64 {
        let skew = i64::try_from(self.skew.as_secs()).unwrap_or(i64::MAX);
        self.base
            .unwrap_or_else(super::super::fsx::now)
            .saturating_add(skew)
    }

    pub(super) fn sleep(&mut self, d: Duration) {
        let real = d.div_f64(self.scale).min(Duration::from_secs(1));
        std::thread::sleep(real);
        self.skew += if self.base.is_some() { d } else { d - real };
    }
}

/// `AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW` and `_TEST_SCALE`, read only in
/// debug builds.
#[cfg(debug_assertions)]
fn seams() -> (Option<i64>, f64) {
    let var = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let base = var("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW")
        .filter(|v| v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse().ok());
    let scale = var("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| s.is_finite() && (1.0..=1_000_000.0).contains(s))
        .unwrap_or(1.0);
    (base, scale)
}

#[cfg(not(debug_assertions))]
fn seams() -> (Option<i64>, f64) {
    (None, 1.0)
}
