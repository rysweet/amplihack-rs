//! The rotate ledger for an older amplihack (#1659): which fields a rotator
//! from another binary cannot read, and the note printed when one runs, and
//! where it is printed. Kept
//! apart from `ledger.rs` to hold that file within its line budget.

use super::ledger::{RotateLedger, RotateState};

/// The ledger as written for its readers. With `older` (a rotator from
/// another binary runs), fields added after the ledger first shipped are
/// left out, because amplihack before #1659 rejects unknown ledger fields:
/// - `last_cycle.measured_at`, without which the limit-resume hook falls
///   back to its 60-second margin;
/// - `last_cycle.overflow` and `overflow_blocked` (issue #1606). An older
///   rotator never moves onto an overflow profile, so it has no use for
///   them. A spend block dropped this way is not a cost: when this binary
///   runs the rotator again, the blocked profile's probe still reports the
///   spend limit, and a profile reporting one is blocked again and never
///   moved onto;
/// - `guidance`, `home_unreadable_since` and `lost_login` (issue #1693),
///   which only `rotate status` and this binary's hand-change recovery read;
/// - `no_room` (issue #1804), set on every measuring cycle. Only this
///   binary's auto-switch reads it, as advice on whether a plan login was
///   measured. Without it auto-switch counts the rotator's live login as
///   unread and holds the API key back, which fails safe. Unlike
///   `platform_block` and `live_kind`, nothing is gained by keeping it for a
///   rotator that would refuse the whole ledger;
/// - `advisory_alert` (issue #1907). Without it an older rotator logs the
///   advisory ALERT once more; `alert` stays empty, so no hook is blocked;
/// - `alert_log` (issue #2013), the hourly alert limit's state. Without it
///   this binary writes each limited line once more when it next runs.
pub fn for_older_readers(l: &RotateLedger, older: bool) -> RotateLedger {
    let mut out = l.clone();
    if older {
        if let Some(c) = &mut out.last_cycle {
            c.measured_at = None;
            c.overflow = None;
        }
        out.overflow_blocked.clear();
        out.guidance = None;
        out.home_unreadable_since = None;
        out.lost_login.clear();
        out.no_room = None;
        // Full identity provenance is optional; legacy strict readers reject it.
        out.login_identity = None;
        // Issue #1870: only this binary's freshness gate reads it.
        out.swapped_away.clear();
        out.advisory_alert = None;
        // Issue #2013: only this binary's alert limit reads it.
        out.alert_log.clear();
    }
    out
}

/// The one line printed when a rotator from another amplihack binary runs.
pub fn foreign_rotator_note(pid: u32) -> String {
    format!(
        "amplihack: the Claude login rotator (pid {pid}) runs another amplihack binary, so the \
         rotate ledger is written in the format it reads; restart it to finish the upgrade: \
         amplihack profile rotate stop && amplihack profile rotate start"
    )
}

/// `amplihack install` and one-shot `rotate` (#1659): one line on stderr when
/// the rotator for `root` (default: this environment's root) runs another
/// amplihack binary, which keeps reading the ledger in its own format until
/// it is restarted.
pub(crate) fn warn_foreign_rotator(root: Option<&std::path::Path>) {
    let root = match root {
        Some(r) => Some(r.to_path_buf()),
        None => amplihack_utils::rotator_status::resolve_root(),
    };
    if let Some(pid) = root.and_then(|r| RotateState::new(&r).foreign_rotator()) {
        eprintln!("{}", foreign_rotator_note(pid));
    }
}
