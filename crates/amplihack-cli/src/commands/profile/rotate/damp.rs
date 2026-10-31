//! Failed-probe damping: a pure rule applied after `decide`. A failed live
//! probe counts as headroom 0, so `decide` alone would swap every cycle
//! while the live probe keeps failing. A swap made only because the live
//! probe failed needs `FAILED_PROBE_LIMIT` failures in a row and is subject
//! to the hold. See "Rotate failed live probes" in
//! docs/reference/profile-command.md.
//!
//! A key in the `$HOME` files fails the live probe whichever login is live.
//! Candidates read normally, since each is probed through its own directory,
//! but the one swapped in would then fail as well. So [`keep_for_home_key`]
//! turns that swap into a stay: it would change neither the billing nor the
//! next live reading. The cycle gives it only a key that
//! [`reachable`](super::key_check::reachable) accepts: a stay on a dead login
//! costs more than a useless hourly swap, so a doubtful key does not stop one.

use super::decide::{Decision, Reading, Reason, held};
use super::log::msg;
use super::settings::Settings;

/// Consecutive failed live probes before a swap on that ground alone.
pub const FAILED_PROBE_LIMIT: u32 = 3;

/// The consecutive failed live probe count after a cycle that read `live`.
pub fn next_failed(prev: u32, live: &Reading) -> u32 {
    match live {
        Reading::Failed => prev.saturating_add(1),
        _ => 0,
    }
}

/// `d` after the failed-probe rules, with the log note for a damped stay.
/// Only a threshold swap on a failed live probe is changed.
pub fn damp(
    d: Decision,
    live: &Reading,
    consecutive_failed: u32,
    swapped_at: Option<i64>,
    now: i64,
    s: &Settings,
) -> (Decision, Option<String>) {
    let to = match (live, d) {
        (
            Reading::Failed,
            Decision::Swap {
                to,
                reason: Reason::Threshold,
            },
        ) => to,
        (_, d) => return (d, None),
    };
    if consecutive_failed < FAILED_PROBE_LIMIT {
        let note = msg::stay_failed_probe(consecutive_failed, FAILED_PROBE_LIMIT);
        return (Decision::Stay, Some(note));
    }
    if let Some(t) = swapped_at
        && held(s.hold, swapped_at, now)
    {
        // A future swap time holds for the whole hold.
        let elapsed = u64::try_from(now.saturating_sub(t)).unwrap_or(0);
        let left = s.hold.saturating_sub(elapsed);
        return (Decision::Stay, Some(msg::stay_held_failed(left)));
    }
    (
        Decision::Swap {
            to,
            reason: Reason::FailedProbe,
        },
        None,
    )
}

/// True when `d` is a swap made only because the live probe kept failing;
/// the cycle looks for a home key only then.
pub fn is_failed_probe_swap(d: &Decision) -> bool {
    matches!(
        d,
        Decision::Swap {
            reason: Reason::FailedProbe,
            ..
        }
    )
}

/// `d` given the `$HOME` key `setting`, with the log note for the stay. A
/// failed-probe swap becomes a stay; anything else passes unchanged.
pub fn keep_for_home_key(
    d: Decision,
    note: Option<String>,
    setting: Option<&str>,
) -> (Decision, Option<String>) {
    match setting {
        Some(setting) if is_failed_probe_swap(&d) => {
            (Decision::Stay, Some(msg::stay_home_key(setting)))
        }
        _ => (d, note),
    }
}
