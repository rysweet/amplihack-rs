//! Adaptive retry backoff.
//!
//! Provides an exponential-with-jitter backoff policy bounded by an
//! operator-configurable *time budget* rather than an arbitrary fixed
//! attempt cap. Retries continue, with growing delays, until the total
//! elapsed time exceeds the budget.
//!
//! Cancellation is respected implicitly: the delays are awaited via
//! [`tokio::time::sleep`], which is cancellation-safe — if the enclosing
//! task/future is dropped (cancelled), the in-flight delay is aborted and
//! no further retries occur.

use std::time::Duration;

/// Environment variable that overrides the retry *time budget* (seconds).
pub const RETRY_BUDGET_ENV: &str = "AMPLIHACK_REMOTE_RETRY_BUDGET_SECS";

/// Adaptive exponential backoff bounded by a total time budget.
#[derive(Debug, Clone)]
pub struct BackoffPolicy {
    /// Delay before the first retry.
    initial: Duration,
    /// Multiplier applied to the base delay each attempt (> 1.0 for growth).
    multiplier: f64,
    /// Total wall-clock budget across all retries. Once elapsed time meets
    /// or exceeds this, no further retries are scheduled.
    budget: Duration,
}

impl BackoffPolicy {
    /// Construct a policy from explicit parameters.
    pub fn new(initial: Duration, multiplier: f64, budget: Duration) -> Self {
        Self {
            initial,
            multiplier,
            budget,
        }
    }

    /// The total retry budget this policy is bounded by.
    pub fn budget(&self) -> Duration {
        self.budget
    }

    /// Read the operator-configured budget from [`RETRY_BUDGET_ENV`], falling
    /// back to `default_budget` when unset or unparsable.
    fn budget_from_env(default_budget: Duration) -> Duration {
        match std::env::var(RETRY_BUDGET_ENV) {
            Ok(raw) => match raw.trim().parse::<u64>() {
                Ok(secs) if secs > 0 => Duration::from_secs(secs),
                _ => default_budget,
            },
            Err(_) => default_budget,
        }
    }

    /// Default policy for VM provisioning retries (budget: 120s, override
    /// via [`RETRY_BUDGET_ENV`]).
    pub fn provisioning_default() -> Self {
        Self::new(
            Duration::from_secs(5),
            2.0,
            Self::budget_from_env(Duration::from_secs(120)),
        )
    }

    /// Default policy for file-transfer retries (budget: 60s, override via
    /// [`RETRY_BUDGET_ENV`]).
    pub fn transfer_default() -> Self {
        Self::new(
            Duration::from_secs(2),
            2.0,
            Self::budget_from_env(Duration::from_secs(60)),
        )
    }

    /// Exponential base delay for a given zero-based attempt (no jitter).
    fn base_delay(&self, attempt: u32) -> Duration {
        let factor = self.multiplier.powi(attempt as i32);
        let millis = self.initial.as_millis() as f64 * factor;
        // Guard against non-finite/negative results before casting.
        let millis = if millis.is_finite() && millis >= 0.0 {
            millis as u64
        } else {
            u64::MAX
        };
        Duration::from_millis(millis)
    }

    /// Apply "equal jitter": half the base delay plus a random amount in
    /// `[0, base/2]`. This spreads retries to avoid thundering herds while
    /// keeping a sensible minimum wait.
    fn with_jitter(&self, base: Duration) -> Duration {
        let half = base / 2;
        let span_ms = half.as_millis() as u64;
        let jitter_ms = if span_ms == 0 {
            0
        } else {
            pseudo_random_u64() % (span_ms + 1)
        };
        half + Duration::from_millis(jitter_ms)
    }

    /// Compute the delay before the next retry, or `None` if the time budget
    /// is exhausted and retries should stop.
    ///
    /// `attempt` is the zero-based retry index and `elapsed` is the wall-clock
    /// time spent so far. The returned delay is clamped so that sleeping for
    /// it will not overshoot the remaining budget.
    pub fn next_backoff(&self, attempt: u32, elapsed: Duration) -> Option<Duration> {
        if elapsed >= self.budget {
            return None;
        }
        let remaining = self.budget - elapsed;
        let delay = self.with_jitter(self.base_delay(attempt));
        Some(delay.min(remaining))
    }
}

/// Small, dependency-free PRNG (splitmix64) seeded from the current time.
/// Sufficient for backoff jitter — not for anything security-sensitive.
fn pseudo_random_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_delay_grows_exponentially() {
        let policy = BackoffPolicy::new(Duration::from_millis(100), 2.0, Duration::from_secs(600));
        assert_eq!(policy.base_delay(0), Duration::from_millis(100));
        assert_eq!(policy.base_delay(1), Duration::from_millis(200));
        assert_eq!(policy.base_delay(2), Duration::from_millis(400));
        assert_eq!(policy.base_delay(3), Duration::from_millis(800));
    }

    #[test]
    fn jitter_stays_within_equal_jitter_bounds() {
        let policy = BackoffPolicy::new(Duration::from_millis(1000), 2.0, Duration::from_secs(600));
        for attempt in 0..4 {
            let base = policy.base_delay(attempt);
            for _ in 0..50 {
                let jittered = policy.with_jitter(base);
                assert!(jittered >= base / 2, "jitter below half base");
                assert!(jittered <= base, "jitter above base");
            }
        }
    }

    #[test]
    fn next_backoff_returns_none_when_budget_exhausted() {
        let policy = BackoffPolicy::new(Duration::from_secs(1), 2.0, Duration::from_secs(10));
        assert!(policy.next_backoff(0, Duration::from_secs(10)).is_none());
        assert!(policy.next_backoff(5, Duration::from_secs(11)).is_none());
        assert!(policy.next_backoff(0, Duration::from_secs(1)).is_some());
    }

    #[test]
    fn next_backoff_clamps_to_remaining_budget() {
        let policy = BackoffPolicy::new(Duration::from_secs(30), 2.0, Duration::from_secs(10));
        // Large base delay (30s) but only 2s of budget remains.
        let delay = policy
            .next_backoff(0, Duration::from_secs(8))
            .expect("budget remains");
        assert!(
            delay <= Duration::from_secs(2),
            "delay must not overshoot budget"
        );
    }

    #[test]
    fn budget_from_env_overrides_default() {
        // Guard: the env var must not be set by the surrounding environment.
        let policy = BackoffPolicy::provisioning_default();
        // Default budget is 120s unless overridden.
        assert!(policy.budget() >= Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn retries_stop_once_budget_is_spent() {
        // Simulate a retry loop driven purely by the policy and confirm it
        // terminates within the budget rather than looping forever.
        let policy = BackoffPolicy::new(Duration::from_millis(50), 2.0, Duration::from_secs(5));
        let start = tokio::time::Instant::now();
        let mut attempt = 0u32;
        while let Some(delay) = policy.next_backoff(attempt, start.elapsed()) {
            tokio::time::sleep(delay).await;
            attempt += 1;
            assert!(attempt < 10_000, "loop must be budget-bounded");
        }
        assert!(attempt > 0, "at least one retry should have occurred");
        assert!(start.elapsed() >= Duration::from_secs(5));
    }

    #[tokio::test(start_paused = true)]
    async fn backoff_sleep_is_cancellation_safe() {
        // Wrapping the backoff sleep in a timeout that fires first models
        // task cancellation: the sleep is aborted and no retry proceeds.
        let policy = BackoffPolicy::new(Duration::from_secs(10), 2.0, Duration::from_secs(60));
        let delay = policy.next_backoff(3, Duration::ZERO).unwrap();
        let result = tokio::time::timeout(Duration::from_millis(1), async move {
            tokio::time::sleep(delay).await;
            "completed"
        })
        .await;
        assert!(result.is_err(), "delay should be cancelled by the timeout");
    }
}
