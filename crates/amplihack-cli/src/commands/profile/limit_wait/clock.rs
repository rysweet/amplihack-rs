//! The usage-limit wait's clock (issue #1618).

use std::time::{Duration, Instant};

/// The shortest real block `poll(2)` can make.
const MIN_REAL: Duration = Duration::from_millis(1);

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
    /// shortest wait `poll(2)` can make, a millisecond. Such a block moves
    /// the clock by the whole of `d`. On the way to a deadline it checks
    /// without blocking, so a test clock reaches a reset days away without a
    /// millisecond per progress line. A wait with no deadline (`deadline`
    /// false, issue #2911) has nothing far off to reach and ends only on an
    /// event, so its block still gives that event a millisecond of real time
    /// to arrive: a test's writer can make one, and a test wait that nothing
    /// ends is a slow loop rather than a hot one.
    pub(super) fn block<T>(
        &mut self,
        d: Duration,
        deadline: bool,
        f: impl FnOnce(Duration) -> T,
    ) -> T {
        let began = Instant::now();
        let real = self.real(d);
        if real < MIN_REAL {
            let out = f(if deadline { Duration::ZERO } else { MIN_REAL });
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
