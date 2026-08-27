//! Bounded mechanical retry for unambiguously transient transport faults
//! (issue #1267).
//!
//! # Scope, and what is deliberately out of it
//!
//! This module retries ONE thing: a recipe run that failed with an
//! unambiguously transient transport fault, as decided by
//! [`super::failure_class`]. That is a mechanical decision about a mechanical
//! fault, and a small bounded loop with exponential backoff is the right tool.
//!
//! It does NOT decide whether continuing is *worthwhile*. There is no
//! "give up after N rounds of no progress" integer anywhere in here, because
//! that is a judgement about progress and belongs to the agentic layer
//! (`loop-health-evaluator.yaml`, issue #1337), which looks at the evidence.
//! The bounds below exist only to stop a transport retry from becoming an
//! unbounded loop against a genuinely dead endpoint — they are a safety
//! backstop on a mechanical retry, not a substitute for judgement.
//!
//! # Testability
//!
//! [`run_with_transient_retry`] takes its sleep as a parameter, so the retry
//! behaviour is unit-testable without waiting out a real backoff. The time
//! budget is charged against the delays this loop hands to `sleep` and nothing
//! else, so no clock is consulted and the accounting is exact.

use super::failure_class::FailureVerdict;
use amplihack_utils::backoff::BackoffPolicy;
use std::time::Duration;

/// Total attempts (the first try plus retries) for a transient transport fault.
pub(crate) const MAX_ATTEMPTS_ENV: &str = "AMPLIHACK_RECIPE_TRANSIENT_MAX_ATTEMPTS";
/// Budget, in seconds, for the total time spent WAITING on backoff.
///
/// Deliberately not "time since the run started": the fault this exists for
/// arrives hours into a long run, and a budget measured from the start would
/// already be spent by the time the first 529 lands — the retry would never
/// happen at all. What is bounded here is how long the runner sits idle hoping
/// an endpoint recovers, not how long the work itself takes.
pub(crate) const BUDGET_ENV: &str = "AMPLIHACK_RECIPE_TRANSIENT_BUDGET_SECS";

/// Default total attempts. Small on purpose: an overloaded model endpoint
/// recovers in seconds or it is not the kind of fault this loop can fix.
const DEFAULT_MAX_ATTEMPTS: u32 = 3;
/// Default budget for time spent waiting on backoff, across all retries.
const DEFAULT_BUDGET: Duration = Duration::from_secs(300);
/// Delay before the first retry; doubles (with equal jitter) thereafter.
const INITIAL_DELAY: Duration = Duration::from_secs(10);
const MULTIPLIER: f64 = 2.0;
/// Ceiling on the configurable attempt count, so a typo in the environment
/// cannot turn the backstop off.
const MAX_ATTEMPTS_CEILING: u32 = 10;

/// Why the retry loop stopped retrying a still-transient failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopReason {
    /// The attempt count ran out.
    AttemptCap { max_attempts: u32 },
    /// The backoff-waiting budget ran out.
    TimeBudget { budget: Duration },
}

impl StopReason {
    pub(crate) fn describe(self) -> String {
        match self {
            Self::AttemptCap { max_attempts } => format!(
                "transient-retry attempt cap reached ({max_attempts} attempts; \
                 raise with ${MAX_ATTEMPTS_ENV})"
            ),
            Self::TimeBudget { budget } => format!(
                "transient-retry backoff-wait budget exhausted ({budget:?}; \
                 raise with ${BUDGET_ENV})"
            ),
        }
    }
}

/// The bounds on the mechanical transport retry.
#[derive(Debug, Clone)]
pub(crate) struct TransientRetryLimits {
    max_attempts: u32,
    policy: BackoffPolicy,
}

impl TransientRetryLimits {
    pub(crate) fn new(max_attempts: u32, policy: BackoffPolicy) -> Self {
        Self {
            max_attempts: max_attempts.max(1),
            policy,
        }
    }

    /// Read the operator-configured bounds, falling back to the defaults when
    /// unset or unparsable. `AMPLIHACK_RECIPE_TRANSIENT_MAX_ATTEMPTS=1`
    /// disables the retry entirely (one attempt, no retries).
    pub(crate) fn from_env() -> Self {
        let max_attempts = env_u64(MAX_ATTEMPTS_ENV)
            .map(|value| value.min(u64::from(MAX_ATTEMPTS_CEILING)) as u32)
            .unwrap_or(DEFAULT_MAX_ATTEMPTS)
            .max(1);
        let budget = env_u64(BUDGET_ENV)
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_BUDGET);
        Self::new(
            max_attempts,
            BackoffPolicy::new(INITIAL_DELAY, MULTIPLIER, budget),
        )
    }

    pub(crate) fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// How long to wait before attempt number `attempts_made + 1`, or why not
    /// to make it at all.
    ///
    /// `attempts_made` is the number of attempts already completed (>= 1 when
    /// this is consulted). `waited` is the time ALREADY spent on backoff — not
    /// the age of the run; see [`BUDGET_ENV`] for why that distinction is the
    /// whole point.
    pub(crate) fn plan_next(
        &self,
        attempts_made: u32,
        waited: Duration,
    ) -> Result<Duration, StopReason> {
        if attempts_made >= self.max_attempts {
            return Err(StopReason::AttemptCap {
                max_attempts: self.max_attempts,
            });
        }
        self.policy
            .next_backoff(attempts_made.saturating_sub(1), waited)
            .ok_or(StopReason::TimeBudget {
                budget: self.policy.budget(),
            })
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
}

/// What one attempt produced.
pub(crate) enum AttemptOutcome<T> {
    /// Success, or a failure that must NOT be retried mechanically. Returned
    /// to the caller as-is.
    Final(T),
    /// An unambiguously transient transport fault. Retried while the bounds
    /// allow; the value is kept as the answer if they do not.
    Transient(T, FailureVerdict),
}

/// What the retry loop did, for logging and for the terminal marker.
pub(crate) struct RetrySummary {
    /// Attempts actually made (always >= 1).
    pub(crate) attempts: u32,
    /// The delays waited between attempts, in order.
    pub(crate) waited: Vec<Duration>,
    /// Set only when the loop gave up on a still-transient failure.
    pub(crate) stop_reason: Option<StopReason>,
    /// The last transient verdict observed, if any.
    pub(crate) last_verdict: Option<FailureVerdict>,
}

/// Run `attempt` until it returns [`AttemptOutcome::Final`] or the transient
/// bounds are spent.
///
/// `attempt` receives the 1-based attempt number. `sleep` is injected so tests
/// exercise the real control flow without real time passing: production passes
/// `thread::sleep`, tests pass a closure that records the delay and returns.
/// The budget is charged against the delays this loop hands to `sleep`, so no
/// clock is consulted at all — the accounting is exact and deterministic.
///
/// `on_retry` is invoked with the verdict and the 1-based number of the attempt
/// that just failed, immediately before the wait — this is where the caller
/// emits its classification marker.
pub(crate) fn run_with_transient_retry<T>(
    limits: &TransientRetryLimits,
    mut attempt: impl FnMut(u32) -> AttemptOutcome<T>,
    mut on_retry: impl FnMut(&FailureVerdict, u32, Duration),
    mut sleep: impl FnMut(Duration),
) -> (T, RetrySummary) {
    let mut attempts_made = 0u32;
    let mut waited = Vec::new();

    loop {
        attempts_made += 1;
        match attempt(attempts_made) {
            AttemptOutcome::Final(value) => {
                return (
                    value,
                    RetrySummary {
                        attempts: attempts_made,
                        waited,
                        stop_reason: None,
                        last_verdict: None,
                    },
                );
            }
            AttemptOutcome::Transient(value, verdict) => {
                let waited_so_far: Duration = waited.iter().sum();
                match limits.plan_next(attempts_made, waited_so_far) {
                    Ok(delay) => {
                        on_retry(&verdict, attempts_made, delay);
                        waited.push(delay);
                        sleep(delay);
                    }
                    Err(stop_reason) => {
                        return (
                            value,
                            RetrySummary {
                                attempts: attempts_made,
                                waited,
                                stop_reason: Some(stop_reason),
                                last_verdict: Some(verdict),
                            },
                        );
                    }
                }
            }
        }
    }
}
