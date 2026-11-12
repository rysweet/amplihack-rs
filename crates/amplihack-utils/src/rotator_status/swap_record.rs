//! The swap record, `<root>/.auto-switch.d/rotate/swap.json` (issue #2085).
//!
//! The rotator (`amplihack profile rotate`) writes one after each swap it
//! makes, under its lock, with mode 0600 through a temp file and a rename.
//! The limit-resume hook waits for a new one and decides with [`judge_at`]
//! whether it resumes a session stopped by a usage limit. The stop time it
//! passes comes from [`read_stop_time`], the only transcript read here.
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
/// and still be after the stop, when the stop time is not known.
///
/// This is the fallback. When [`read_stop_time`] gives the stop time, the
/// threshold is the lower of the stop and `start - STOP_SLACK_SECS` (see
/// [`after_stop_at`]), so a known stop only ever widens the window.
///
/// Without a stop time the hook knows only when it started (`start`), a
/// moment after the stop. Both `start` and `swapped_at` are whole epoch
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

/// The most bytes [`read_stop_time`] reads from the end of a transcript.
pub const STOP_TAIL_BYTES: u64 = 65_536;

/// The only field read from a transcript entry.
#[derive(Deserialize)]
struct LastEntry {
    timestamp: String,
}

/// When the session stopped, in whole epoch seconds (rounded down): the
/// `timestamp` of the last complete entry in the session's transcript.
///
/// Only the last [`STOP_TAIL_BYTES`] of the file are read. A partial last
/// line (no newline yet) is ignored, and so is a partial first line when the
/// read starts inside the file. Only the last non-empty complete line is
/// looked at; there is no fallback to an earlier one.
///
/// `None` when the file is missing, a symlink, not a regular file, not
/// owned by the current user or writable by the group or others, when the
/// last entry is not JSON with an RFC 3339 `timestamp` string, or when the
/// time is not within `0 < t <= start`. Nothing is logged and no error is
/// returned, so no byte of the transcript can reach a log or a message.
/// Symlinks in parent directories are followed; the checks apply to the
/// opened file. A FIFO never blocks the open.
#[cfg(unix)]
pub fn read_stop_time(transcript: &Path, start: i64) -> Option<i64> {
    use crate::secure_files::owned_by_euid;
    use std::io::{Read, Seek, SeekFrom};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(transcript)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || !owned_by_euid(&meta) || meta.mode() & 0o022 != 0 {
        return None;
    }
    let off = meta.len().saturating_sub(STOP_TAIL_BYTES);
    file.seek(SeekFrom::Start(off)).ok()?;
    // Bounded even if the file has grown since the size was read.
    let mut raw = Vec::new();
    file.take(STOP_TAIL_BYTES).read_to_end(&mut raw).ok()?;
    // Everything after the last newline is a line still being written.
    let end = raw.iter().rposition(|&b| b == b'\n')?;
    let mut complete = raw.get(..end)?;
    if off > 0 {
        // The read began inside a line: drop it.
        let first = complete.iter().position(|&b| b == b'\n')?;
        complete = complete.get(first + 1..)?;
    }
    let last = complete
        .split(|&b| b == b'\n')
        .rev()
        .find(|l| !l.iter().all(u8::is_ascii_whitespace))?;
    let entry: LastEntry = serde_json::from_slice(last).ok()?;
    let t = chrono::DateTime::parse_from_rfc3339(&entry.timestamp)
        .ok()?
        .timestamp();
    (t > 0 && t <= start).then_some(t)
}

/// Off Unix there is no rotator, so no stop time is needed.
#[cfg(not(unix))]
pub fn read_stop_time(_transcript: &Path, _start: i64) -> Option<i64> {
    None
}

/// [`after_stop_at`] with no stop time.
pub fn after_stop(swapped_at: i64, start: i64, resumed_at: Option<i64>) -> bool {
    after_stop_at(swapped_at, start, None, resumed_at)
}

/// Whether a swap at `swapped_at` may have finished after a stop at `stop`
/// (from [`read_stop_time`]; `None` when unknown) whose hook started at
/// `start`, in a session last resumed at `resumed_at`.
///
/// The swap must be at or after `min(stop, start - STOP_SLACK_SECS)`, or
/// `start - STOP_SLACK_SECS` with no stop (see [`STOP_SLACK_SECS`]). Taking
/// the lower of the two means a known stop never makes the rule stricter
/// than the fallback; it only lets a hook that started late still see a
/// swap made between the stop and its start.
///
/// A swap at or before the session's last resume is always before the
/// stop: the session ran again after that resume, so it stopped later. This
/// is what keeps the allowance from waking a session twice on one swap
/// when, once resumed, it hits a limit again soon after.
pub fn after_stop_at(
    swapped_at: i64,
    start: i64,
    stop: Option<i64>,
    resumed_at: Option<i64>,
) -> bool {
    let fallback = start.saturating_sub(STOP_SLACK_SECS);
    let threshold = stop.map_or(fallback, |s| s.min(fallback));
    swapped_at >= threshold && resumed_at.is_none_or(|r| swapped_at > r)
}

/// Whether `record`, seen when the hook started at `start`, may be a swap
/// made between the stop and that start: dated no later than `start` and
/// after the stop by [`after_stop_at`]. Such a snapshot is not `Same`
/// ([`judge_at`]). Without a stop time, [`stopped_on_outgoing`] uses this
/// rule for the event key.
pub fn noted_after_stop(
    record: &SwapRecord,
    start: i64,
    stop: Option<i64>,
    resumed_at: Option<i64>,
) -> bool {
    record.swapped_at <= start && after_stop_at(record.swapped_at, start, stop, resumed_at)
}

/// Whether a session stopped at `stop` (`None` when unknown), whose hook
/// started at `start` and noted `record`, stopped on `record.from_login`
/// rather than on the live login (#2235). The hook's event key names that
/// login.
///
/// With a stop time the swap must fall between one second before the stop
/// and the hook's start: `stop - STOP_SLACK_SECS <= swapped_at <= start`.
/// The lower bound depends on the stop alone, never on `start` (when this
/// copy of the hook happened to run) or on the session's resumes, so every
/// delivery of one stop gets the same key, whenever it starts and before or
/// after the stop is resumed. A repeat delivery is therefore always a
/// `duplicate`; it never waits for a later swap and resumes the session a
/// second time.
///
/// The one-second allowance is the same one [`STOP_SLACK_SECS`] gives the
/// resume decision when the hook starts in the stop's second: the stop and
/// a swap that finished just after it can be truncated into adjacent whole
/// seconds. A session resumed on a swap one second before its stop, through
/// that allowance, therefore has its event named for `from_login`, and a
/// later stop on the new login with the same text is not a `duplicate`: its
/// own stop is later, so the old swap is below its bound and it keys on the
/// live login.
///
/// This is not the same predicate as the resume decision, and does not try
/// to be. [`after_stop_at`] uses `min(stop, start - STOP_SLACK_SECS)`, which
/// depends on `start`. They differ only when the hook starts two or more
/// seconds after the stop and the swap is in the second before the stop:
/// the key names `from_login`, while the resume decision reads that swap as
/// before the stop (`Same`) and waits for the next one. Naming the login
/// the swap left is the reading that keeps repeat deliveries together; it
/// costs nothing but the event's name, since the hook still resumes only on
/// a later swap. The same reading has one cost: a session resumed on that
/// very swap in the second before its next stop, which stops again with
/// the same text, and whose hook then starts two or more seconds late, gets
/// the earlier event's key and is a `duplicate`. That needs a resume, a
/// fresh limit and a late hook all within about two seconds; the session is
/// left stopped rather than sent keys twice.
///
/// Without a stop time this is [`noted_after_stop`], whose
/// [`STOP_SLACK_SECS`] window and last-resume bound are all the hook has.
pub fn stopped_on_outgoing(
    record: &SwapRecord,
    start: i64,
    stop: Option<i64>,
    resumed_at: Option<i64>,
) -> bool {
    match stop {
        Some(s) => {
            s.saturating_sub(STOP_SLACK_SECS) <= record.swapped_at && record.swapped_at <= start
        }
        None => noted_after_stop(record, start, None, resumed_at),
    }
}

/// [`judge_at`] with no stop time.
pub fn judge(
    snapshot: Option<&SwapRecord>,
    record: &SwapRecord,
    start: i64,
    resumed_at: Option<i64>,
) -> Judgement {
    judge_at(snapshot, record, start, None, resumed_at)
}

/// Whether `record` resumes a session stopped at `stop` (`None` when
/// unknown) whose hook started at `start` and saw `snapshot` then, for a
/// session last resumed at `resumed_at` (from the hook's guard; `None` when
/// it was never resumed). Checked in the order Same, Stale, NoRoom, then
/// Resume. `seq` is never compared for order: a record that differs from
/// the snapshot and is newer than the stop is a new swap, so a reset `seq`
/// still resumes and a replayed old record is stale.
///
/// A snapshot that may itself be after the stop and is not later than
/// `start` ([`noted_after_stop`]) is not `Same`: a swap can finish between the stop
/// and the hook's start, and the hook then notes it at once. That record is
/// judged on room like any other, so the session stopped on the outgoing
/// login still resumes. A snapshot dated after `start` cannot be such a
/// swap (it was already on disk when the hook started), so a clock that
/// runs ahead never makes a noted record resume.
pub fn judge_at(
    snapshot: Option<&SwapRecord>,
    record: &SwapRecord,
    start: i64,
    stop: Option<i64>,
    resumed_at: Option<i64>,
) -> Judgement {
    let after = after_stop_at(record.swapped_at, start, stop, resumed_at);
    if snapshot == Some(record) && !noted_after_stop(record, start, stop, resumed_at) {
        Judgement::Same
    } else if !after {
        Judgement::Stale
    } else if record.reading.session >= 100 || record.reading.week >= 100 {
        Judgement::NoRoom
    } else {
        Judgement::Resume
    }
}
