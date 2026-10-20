//! Failed-probe damping: a pure rule applied after `decide`. A failed live
//! probe counts as headroom 0, so `decide` alone would swap every cycle
//! while the live probe keeps failing. A swap made only because the live
//! probe failed needs `FAILED_PROBE_LIMIT` failures in a row and is subject
//! to the hold. See "Rotate failed live probes" in
//! docs/reference/profile-command.md.

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
