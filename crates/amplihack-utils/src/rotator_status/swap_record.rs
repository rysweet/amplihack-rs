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
    /// The record is the one seen when the hook started: no new swap.
    Same,
    /// The swap, or its measurement, is not later than the stop (D5).
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

/// Whether `record` resumes a session stopped at `start` whose hook saw
/// `snapshot` when it started. Checked in the order Same, Stale, NoRoom,
/// then Resume. `seq` is never compared for order: a record that differs
/// from the snapshot and is newer than the stop is a new swap, so a reset
/// `seq` still resumes and a replayed old record is stale.
pub fn judge(snapshot: Option<&SwapRecord>, record: &SwapRecord, start: i64) -> Judgement {
    if snapshot == Some(record) {
        Judgement::Same
    } else if record.swapped_at <= start || record.reading.measured_at <= start {
        Judgement::Stale
    } else if record.reading.session >= 100 || record.reading.week >= 100 {
        Judgement::NoRoom
    } else {
        Judgement::Resume
    }
}
