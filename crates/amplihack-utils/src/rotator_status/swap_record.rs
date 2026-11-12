//! The swap record, `<root>/.auto-switch.d/rotate/swap.json` (issue #2085).
//!
//! The rotator (`amplihack profile rotate`) writes one after each swap it
//! makes, under its lock, with mode 0600 through a temp file and a rename.
//! The limit-resume hook waits for a new one and decides with [`judge`]
//! whether it resumes a session stopped by a usage limit.
//!
//! The record holds login names, percentages and epoch seconds only:
//!
//! ```json
//! {"seq":7,"live_login":"spare","from_login":"work","swapped_at":1792591460,
//!  "reading":{"session":12,"week":40,"measured_at":1792591430}}
//! ```
//!
//! Unknown fields are ignored, but every field above must be present and
//! well typed. Nothing here creates, changes or locks a file.

use super::ledger::MAX_SKEW_SECS;
use super::valid_live_name;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The record's file name, in the rotate directory.
pub const SWAP_FILE: &str = "swap.json";
/// A larger record is refused.
pub const MAX_SWAP_BYTES: u64 = 4096;

/// The usage of the login swapped to, as the rotator measured it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapReading {
    /// Percent used in the session window, rounded up.
    pub session: u8,
    /// Percent used in the weekly window, rounded up.
    pub week: u8,
    /// When the measurement started, epoch seconds.
    pub measured_at: i64,
}

/// One swap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affinity: Option<crate::organisation_affinity::Affinity>,
    /// The previous record's `seq + 1`, or 1. Never compared for order.
    pub seq: u64,
    /// The login made live.
    pub live_login: String,
    /// The login that was live before.
    pub from_login: String,
    /// When the swap was made, epoch seconds.
    pub swapped_at: i64,
    pub reading: SwapReading,
}

/// What a record means for a session stopped at `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judgement {
    /// The record is the one seen when the hook started, and it was swapped
    /// before the stop: no new swap.
    Same,
    /// The swap finished before the stop (D5). The measurement may be
    /// earlier still: the rotator measures before it swaps, and a session
    /// that stops in between stopped on the login swapped away from.
    Stale,
    /// The login swapped to has a window at or above 100%.
    NoRoom,
    /// A new swap after the stop, onto a login with room.
    Resume,
}

/// `<root>/.auto-switch.d/rotate`.
pub fn rotate_dir(root: &Path) -> PathBuf {
    root.join(".auto-switch.d").join("rotate")
}

/// `<root>/.auto-switch.d/rotate/swap.json`.
pub fn swap_path(root: &Path) -> PathBuf {
    rotate_dir(root).join(SWAP_FILE)
}

/// Parses record bytes read at `now`. `None` for anything over 4096 bytes,
/// anything that is not the schema, a login name that fails
/// [`valid_live_name`], or a time outside `0 < t <= now + 300`.
pub fn parse_swap(raw: &[u8], now: i64) -> Option<SwapRecord> {
    if raw.len() as u64 > MAX_SWAP_BYTES {
        return None;
    }
    let r: SwapRecord = serde_json::from_slice(raw).ok()?;
    use crate::organisation_affinity::{Affinity, valid_id};
    if ![&r.from_org, &r.to_org]
        .into_iter()
        .all(|o| o.as_deref().is_none_or(valid_id))
        || r.affinity
            .is_some_and(|a| a != Affinity::between(r.from_org.as_deref(), r.to_org.as_deref()))
    {
        return None;
    }
    let time_ok = |t: i64| t > 0 && t <= now.saturating_add(MAX_SKEW_SECS);
    let names_ok = valid_live_name(&r.live_login) && valid_live_name(&r.from_login);
    (names_ok && time_ok(r.swapped_at) && time_ok(r.reading.measured_at)).then_some(r)
}

/// Reads and parses the record under `root` at `now`. `None` when it is
/// missing, a symlink, not a regular file, not owned by the current user,
/// writable by the group or others, too large or not a valid record.
#[cfg(unix)]
pub fn read_swap(root: &Path, now: i64) -> Option<SwapRecord> {
    use crate::secure_files::{owned_by_euid, read_bounded_nofollow};
    use std::os::unix::fs::MetadataExt;
    match read_bounded_nofollow(&swap_path(root), MAX_SWAP_BYTES) {
        Ok(Some((raw, meta))) if owned_by_euid(&meta) && meta.mode() & 0o022 == 0 => {
            parse_swap(&raw, now)
        }
        _ => None,
    }
}

/// Off Unix there is no rotator.
#[cfg(not(unix))]
pub fn read_swap(_root: &Path, _now: i64) -> Option<SwapRecord> {
    None
}

/// How many whole seconds before the hook's start a swap may have finished
/// and still be after the stop.
///
/// The hook does not know when the session stopped, only when it started
/// (`start`), a moment later. Both `start` and `swapped_at` are whole epoch
/// seconds, truncated, and the rotator takes `swapped_at` after the swap
/// has finished. A swap that finished after the stop therefore has
/// `swapped_at >= floor(stop) >= start - 1` whenever the hook started less
/// than a second after the stop: the stop and the swap can share a second
/// with `start`, or fall in the second before it. So a record counts as
/// before the stop only when `swapped_at < start - STOP_SLACK_SECS`.
///
/// The error this allows goes the cheap way. A swap that finished in that
/// second but just before the stop is read as after it, and the session is
/// sent one `continue` it may not need. Reading a swap after the stop as
/// before it would leave the session waiting for the next swap, up to the
/// hook's 7-day limit, with nothing to show why.
pub const STOP_SLACK_SECS: i64 = 1;

/// Whether a swap at `swapped_at` may have finished after a stop whose hook
/// started at `start`, in a session last resumed at `resumed_at`.
///
/// A swap at or before the session's last resume is always before the
/// stop: the session ran again after that resume, so it stopped later. This
/// is what keeps the [`STOP_SLACK_SECS`] allowance from waking a session
/// twice on one swap when, once resumed, it hits a limit again within a
/// second. Otherwise see [`STOP_SLACK_SECS`].
pub fn after_stop(swapped_at: i64, start: i64, resumed_at: Option<i64>) -> bool {
    swapped_at >= start.saturating_sub(STOP_SLACK_SECS) && resumed_at.is_none_or(|r| swapped_at > r)
}

/// Whether `record` resumes a session stopped at `start` whose hook saw
/// `snapshot` when it started, for a session last resumed at `resumed_at`
/// (from the hook's guard; `None` when it was never resumed). Checked in
/// the order Same, Stale, NoRoom, then Resume. `seq` is never compared for
/// order: a record that differs from the snapshot and is newer than the
/// stop is a new swap, so a reset `seq` still resumes and a replayed old
/// record is stale.
///
/// A snapshot that may itself be after the stop ([`after_stop`]) and is not
/// later than `start` is not `Same`: a swap can finish between the stop and
/// the hook's start, and the hook then notes it at once. That record is
/// judged on room like any other, so the session stopped on the outgoing
/// login still resumes. A snapshot dated after `start` cannot be such a
/// swap (it was already on disk when the hook started), so a clock that
/// runs ahead never makes a noted record resume.
pub fn judge(
    snapshot: Option<&SwapRecord>,
    record: &SwapRecord,
    start: i64,
    resumed_at: Option<i64>,
) -> Judgement {
    let after = after_stop(record.swapped_at, start, resumed_at);
    if snapshot == Some(record) && !(after && record.swapped_at <= start) {
        Judgement::Same
    } else if !after {
        Judgement::Stale
    } else if record.reading.session >= 100 || record.reading.week >= 100 {
        Judgement::NoRoom
    } else {
        Judgement::Resume
    }
}
