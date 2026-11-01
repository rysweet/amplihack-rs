//! Issue #1797 in the rotator: each measured cycle appends its kept probes
//! to `usage-history.jsonl`, and `rotate status` prints the `capacity:` line
//! from that history. Neither runs a probe, starts a process or holds the
//! rotate state lock.

use super::identify::LiveId;
use super::log::Sink;
use super::observe::{Probes, Snap};
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::{capacity_summary, capacity_weight, usage_history};
use std::path::Path;

#[cfg(test)]
#[path = "tests_capacity_history.rs"]
mod tests;

/// Appends the cycle's probes, measured at `at`, to the usage history. The
/// live probe is recorded only under a profile whose login is identified as
/// live; a hand-changed or unowned login is left out, and so is the API-key
/// profile. `usage_history::append` applies the recording rules (a trusted
/// root, a private `.auto-switch.d` and no credential variable), so nothing
/// is written while a credential variable is set. A line that cannot be
/// written puts one fixed line in the rotate log and never fails the cycle.
pub fn record(root: &Path, out: &mut Sink, first: &Snap, probes: &Probes, at: i64) {
    let live = match &first.live {
        LiveId::Known(n) => Some(n),
        _ => None,
    };
    let entry = usage_history::from_probes(at, live, probes, first.platform())
        .entry(usage_history::Writer::Rotate);
    if let Err(e) = usage_history::append(root, &entry, now())
        && let Some(w) = e.warning()
    {
        out.emit(now(), &w);
    }
}

/// The `capacity:` line and the threshold and weights warnings. The newest
/// history entry is "now" for the figures and the entries up to six hours
/// before it are the history; the threshold comes from the variable alone
/// and the weights from the weights file. The age is measured at `now`.
/// With no readable entry the line is `capacity: no recorded cycle`, with
/// the history's warning when it exists but was refused.
pub fn capacity(root: &Path, now: i64) -> (String, Vec<String>) {
    // One read: the newest entry is "now", the rest are the history.
    let (mut entries, refused) = usage_history::read_all(root, now);
    let Some(newest) = entries.pop() else {
        return (
            "capacity: no recorded cycle".to_string(),
            refused.into_iter().collect(),
        );
    };
    let env = std::env::var_os(capacity_weight::ENV);
    let (threshold, warning) = capacity_weight::usable_threshold(None, env.as_deref());
    let (weights, weight_warnings) = capacity_weight::load(root);
    let warnings = warning.into_iter().chain(weight_warnings).collect();
    let current = usage_history::Snapshot {
        at: newest.at,
        points: newest.profiles,
        unknown: newest.unknown,
    };
    let summary = capacity_summary::compute(&capacity_summary::Inputs {
        current: &current,
        weights: &weights,
        threshold,
        history: &entries,
        now: newest.at,
    });
    (summary.status_line(newest.at, now, newest.by), warnings)
}

/// The `capacity:` line of `rotate status` alone, without its warnings.
#[cfg(test)]
pub fn capacity_line(root: &Path, now: i64) -> String {
    capacity(root, now).0
}
