//! The fair queue (D8). Waiting requests are ordered by class rank, then by
//! how many admissions their workstream had in the last 24 hours, then by
//! arrival. Two workstreams waiting in one class therefore alternate.

use super::model::{Admission, Class, QueueEntry};

/// The fairness window.
pub const GOVERNOR_WINDOW_SECS: i64 = 86_400;
/// The most admissions kept in the window.
pub const GOVERNOR_WINDOW_CAP: usize = 1024;

/// Admissions to `workstream` in `admissions`.
pub fn admissions_to(workstream: &str, admissions: &[Admission]) -> usize {
    admissions
        .iter()
        .filter(|a| a.workstream == workstream)
        .count()
}

/// The key for a request of `class` from `workstream` with ticket `seq`.
pub fn key_for(
    class: Class,
    workstream: &str,
    seq: u64,
    admissions: &[Admission],
) -> (u8, usize, u64) {
    (class.rank(), admissions_to(workstream, admissions), seq)
}

/// `(class rank, admissions to the workstream in the window, seq)`.
pub fn order_key(e: &QueueEntry, admissions: &[Admission]) -> (u8, usize, u64) {
    key_for(e.class, &e.workstream, e.seq, admissions)
}

/// The seq of the entry admitted next, if any.
pub fn head(entries: &[QueueEntry], admissions: &[Admission]) -> Option<u64> {
    entries
        .iter()
        .min_by_key(|e| order_key(e, admissions))
        .map(|e| e.seq)
}

/// Drops admissions older than the window and keeps the newest
/// [`GOVERNOR_WINDOW_CAP`], oldest first.
pub fn prune_window(admissions: &mut Vec<Admission>, now: i64) {
    let since = now.saturating_sub(GOVERNOR_WINDOW_SECS);
    admissions.retain(|a| a.at >= since);
    admissions.sort_by_key(|a| a.at);
    if admissions.len() > GOVERNOR_WINDOW_CAP {
        let excess = admissions.len() - GOVERNOR_WINDOW_CAP;
        admissions.drain(..excess);
    }
}
