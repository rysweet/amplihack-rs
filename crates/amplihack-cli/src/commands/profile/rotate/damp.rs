//! Failed-probe damping: a pure rule applied after `decide`. A failed live
//! probe counts as headroom 0, so `decide` alone would swap every cycle
//! while the live probe keeps failing. A swap made only because the live
//! probe failed needs `FAILED_PROBE_LIMIT` failures in a row; the post-swap
//! hold does not apply to it (#1880): a login that cannot be read gains
//! nothing by being kept. The count is stored in the ledger
//! (`failed_store`), so it survives a restart. See "Rotate failed live
//! probes" in docs/reference/profile-command.md.
//!
//! A key in the `$HOME` files that fails the live probe fails it whichever
//! login is live (`key_check` gives the settings that do). Candidates read
//! normally, since each is probed through its own directory, but the one
//! swapped in would then fail as well. So [`keep_for_home_key`]
//! turns that swap into a stay: it would change neither the billing nor the
//! next live reading. The cycle gives it only a key that
//! [`reachable`](super::key_check::reachable) accepts: a stay on a dead login
//! costs more than a useless hourly swap, so a doubtful key does not stop one.

use super::decide::{Decision, Reading, Reason};
use super::log::msg;
use super::settings::Settings;

/// Consecutive failed live probes before a swap on that ground alone.
pub const FAILED_PROBE_LIMIT: u32 = 3;

/// The count after a cycle that read `live`, for the rule's own tests: the
/// cycle classifies the whole probe with `live_limit`.
#[cfg(test)]
pub fn next_failed(prev: u32, live: &Reading) -> u32 {
    use super::live_limit::{LiveOutcome, next_count};
    let o = match live {
        Reading::Failed => LiveOutcome::Unknown,
        _ => LiveOutcome::Fresh,
    };
    next_count(prev, o)
}

/// `d` after the failed-probe rules, with the log note for a damped stay.
/// Only a threshold or plan-room swap on a failed live probe is changed: it
/// stays below `FAILED_PROBE_LIMIT` and swaps as `failed-probe` at it.
/// `_swapped_at`, `_now` and `_s` are unused since #1880 removed the hold
/// from this rule. They remain only so existing call sites compile; see
/// #1894 to remove them.
pub fn damp(
    d: Decision,
    live: &Reading,
    consecutive_failed: u32,
    _swapped_at: Option<i64>,
    _now: i64,
    _s: &Settings,
) -> (Decision, Option<String>) {
    let to = match (live, d) {
        (
            Reading::Failed,
            Decision::Swap {
                to,
                reason: Reason::Threshold | Reason::PlanRoom,
            },
        ) => to,
        (_, d) => return (d, None),
    };
    if consecutive_failed < FAILED_PROBE_LIMIT {
        let note = msg::stay_failed_probe(consecutive_failed, FAILED_PROBE_LIMIT);
        return (Decision::Stay, Some(note));
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
