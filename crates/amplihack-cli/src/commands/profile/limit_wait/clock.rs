//! The usage-limit wait's clock (issue #1618).

use std::time::{Duration, Instant};

/// Wall time, or with the debug-only test seams a virtual clock that starts
/// at `TEST_NOW` and moves only while the wait blocks, `TEST_SCALE` times as
/// fast as real time.
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

    /// The real time that `d` on this clock stands for.
    pub(super) fn real(&self, d: Duration) -> Duration {
        d.div_f64(self.scale)
    }

    /// Runs `f`, which blocks for at most the real time it is given, with
    /// the real time `d` on this clock stands for, and moves the clock by
    /// however long `f` took, and never past `d`: a block that ran to its
    /// timeout ends exactly at the time it was asked for.
    ///
    /// Only a test's scale makes a block stand for less real time than the
    /// shortest wait `poll(2)` can make, a millisecond. Such a block checks
    /// without blocking and moves the clock by the whole of `d`, so a test
    /// clock reaches a reset days away without a millisecond per progress
    /// line.
    pub(super) fn block<T>(&mut self, d: Duration, f: impl FnOnce(Duration) -> T) -> T {
        let began = Instant::now();
        let real = self.real(d);
        if real < Duration::from_millis(1) {
            let out = f(Duration::ZERO);
            let took = began.elapsed();
            self.skew += if self.base.is_some() {
                d
            } else {
                d.saturating_sub(took)
            };
            return out;
        }
        let out = f(real);
        let took = began.elapsed();
        let seen = took.mul_f64(self.scale).min(d);
        self.skew += if self.base.is_some() {
            seen
        } else {
            seen.saturating_sub(took)
        };
        out
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
