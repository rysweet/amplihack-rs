//! Issue #1797: `<root>/.auto-switch.d/usage-history.jsonl`, one line per
//! measured cycle, read by the capacity summary for its rate and by `rotate
//! status` for its `capacity:` line.
//!
//! A line holds the time, profile names, whole-number session and weekly
//! percentages and the weekly reset time. Nothing else from a probe is kept:
//! no identity, no plan, no error text and nothing Claude printed.
//!
//! [`entry_of`] is the one validity rule. Readers take the last 4 MiB and
//! skip whatever they cannot trust. Writers lock `usage-history.lock`, append
//! one line in one write, trim entries older than eight days, and never
//! create `.auto-switch.d`. No failure here ever fails a command.

use super::name::ProfileName;
use super::usage_cmd::Row;
use super::usage_parse::{UsageReport, UsageState, UsageWindow};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// The history file, inside `<root>/.auto-switch.d`.
pub(super) const FILE: &str = "usage-history.jsonl";
/// The writers' lock file, beside the history.
pub(super) const LOCK: &str = "usage-history.lock";
const STATE_DIR: &str = ".auto-switch.d";
/// Readers take at most this many bytes from the end of the file.
#[cfg_attr(not(unix), allow(dead_code))]
const TAIL: u64 = 4 * 1024 * 1024;
/// A rewrite keeps the file under this size.
#[cfg_attr(not(unix), allow(dead_code))]
const REWRITE_MAX: usize = 3 * 1024 * 1024;
/// Longer lines are skipped.
const LINE_MAX: usize = 64 * 1024;
/// Entries older than this are dropped by a rewrite.
#[cfg_attr(not(unix), allow(dead_code))]
const RETENTION: i64 = 8 * 86_400;
/// Times further than this from now are not believed.
const MAX_SKEW: i64 = 100 * 31_557_600;
#[cfg(unix)]
const LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(50);
#[cfg(unix)]
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// One profile's valid reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Point {
    /// Session percentage used, 0-100; `None` when not read.
    pub(super) s: Option<u8>,
    /// All-models week percentage used, 0-100.
    pub(super) w: u8,
    /// When the week resets, epoch seconds.
    pub(super) w_reset: Option<i64>,
    /// When the session resets, epoch seconds. Used by the summary of the
    /// current cycle only; never written to the history.
    pub(super) s_reset: Option<i64>,
}

/// One history line: the valid readings of one cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) at: i64,
    pub(super) profiles: BTreeMap<String, Point>,
}

/// One cycle as the summary sees it: the valid readings, and the logins
/// whose reading is unknown (sorted by name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) at: i64,
    pub(super) points: BTreeMap<String, Point>,
    pub(super) unknown: Vec<String>,
}

impl Snapshot {
    /// The history entry of this cycle: its valid readings only.
    pub(super) fn entry(&self) -> Entry {
        Entry {
            at: self.at,
            profiles: self.points.clone(),
        }
    }
}

/// Why a line was not written. Fixed reasons: never a path or system text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HistoryError {
    /// `.auto-switch.d` is missing or not private: recording is off.
    NoStateDir,
    LockBusy,
    UnsafeFile,
    WriteFailed,
    TooLarge,
}

impl HistoryError {
    /// The one stderr or rotate-log line, or `None` when recording is off.
    pub(super) fn warning(self) -> Option<String> {
        let reason = match self {
            Self::NoStateDir => return None,
            Self::LockBusy => "lock busy",
            Self::UnsafeFile => "unsafe file",
            Self::WriteFailed => "write failed",
            Self::TooLarge => "too large",
        };
        Some(format!("warning: usage history not updated ({reason})"))
    }
}

// ---------------------------------------------------------------------------
// The validity rule
// ---------------------------------------------------------------------------

/// A percentage clamped to 0-100 and rounded half away from zero; `None`
/// for NaN.
fn whole_pct(p: f64) -> Option<u8> {
    if p.is_nan() {
        return None;
    }
    // Clamped to 0-100 first, so the cast cannot truncate.
    Some(p.clamp(0.0, 100.0).round() as u8)
}

fn pct_of(w: Option<&UsageWindow>) -> Option<u8> {
    w.and_then(|w| whole_pct(w.used_percentage))
}

/// The one validity rule. `ok` and `session_exhausted` are valid with a
/// readable all-models week; `weekly_exhausted` is a known full week even
/// without one. Every other state, and every reading without a week, is
/// unknown: `None`, never 0 or 100.
pub(super) fn entry_of(r: &UsageReport) -> Option<Point> {
    let session = r.session.as_ref();
    let week = r.week_all.as_ref();
    let s_reset = session.and_then(|w| w.resets_at);
    let w_reset = week.and_then(|w| w.resets_at);
    match r.state {
        UsageState::Ok => Some(Point {
            s: pct_of(session),
            w: pct_of(week)?,
            w_reset,
            s_reset,
        }),
        UsageState::SessionExhausted => Some(Point {
            s: Some(100),
            w: pct_of(week)?,
            w_reset,
            s_reset,
        }),
        UsageState::WeeklyExhausted => Some(Point {
            s: pct_of(session),
            w: 100,
            w_reset,
            s_reset,
        }),
        _ => None,
    }
}

/// The snapshot of a `profile usage` run. API-key rows and the `~/.claude`
/// row are not logins and are left out; a live profile whose login was
/// swapped during the probe is unknown, since its numbers may be another
/// login's.
pub(super) fn from_rows(at: i64, rows: &[Row], swapped: Option<&ProfileName>) -> Snapshot {
    let mut points = BTreeMap::new();
    let mut unknown = Vec::new();
    for r in rows.iter().filter(|r| !r.api_key && !r.live_in_home) {
        let moved = swapped.is_some_and(|s| s.as_str() == r.profile);
        match entry_of(&r.report).filter(|_| !moved) {
            Some(p) => {
                points.insert(r.profile.clone(), p);
            }
            None => unknown.push(r.profile.clone()),
        }
    }
    unknown.sort();
    Snapshot {
        at,
        points,
        unknown,
    }
}

/// The snapshot of a rotator cycle. The live probe is recorded under
/// `live`, the live login's profile, and dropped when the login belongs to
/// no profile. The API-key profile `platform` is not a login and is never
/// recorded or counted as unknown.
#[cfg(unix)]
pub(super) fn from_probes(
    at: i64,
    live: Option<&ProfileName>,
    probes: &super::rotate::Probes,
    platform: Option<&ProfileName>,
) -> Snapshot {
    let (live_probe, others) = probes;
    let mut points = BTreeMap::new();
    let mut unknown = Vec::new();
    let named = live
        .map(|n| (n, live_probe))
        .into_iter()
        .chain(others.iter().map(|(n, p)| (n, p)));
    for (name, probe) in named.filter(|(n, _)| platform != Some(*n)) {
        match point_of_probe(probe) {
            Some(p) => {
                points.insert(name.as_str().to_string(), p);
            }
            None => unknown.push(name.as_str().to_string()),
        }
    }
    unknown.sort();
    unknown.dedup();
    Snapshot {
        at,
        points,
        unknown,
    }
}

/// The validity rule for the rotator's kept probe. An exhausted reading is
/// a full week when its week reads 100%, or when only the session window
/// was read and it is not full; otherwise it is a full session with the
/// week it read. A failed probe is unknown.
#[cfg(unix)]
fn point_of_probe(p: &super::rotate::Probed) -> Option<Point> {
    use super::rotate::Reading;
    let session = p.windows.session.as_ref();
    let week = p.windows.week.as_ref();
    let s = session.and_then(|w| whole_pct(w.used));
    let s_reset = session.and_then(|w| w.resets_at);
    let w_reset = week.and_then(|w| w.resets_at);
    match p.reading {
        Reading::Failed => None,
        Reading::Measured { week: used, .. } => Some(Point {
            s,
            w: whole_pct(used)?,
            w_reset,
            s_reset,
        }),
        Reading::Exhausted => match week.map(|w| whole_pct(w.used)) {
            Some(Some(100)) => Some(Point {
                s,
                w: 100,
                w_reset,
                s_reset,
            }),
            Some(Some(w)) => Some(Point {
                s: Some(100),
                w,
                w_reset,
                s_reset,
            }),
            Some(None) => None,
            None => s.filter(|s| *s < 100).map(|s| Point {
                s: Some(s),
                w: 100,
                w_reset: None,
                s_reset,
            }),
        },
    }
}

// ---------------------------------------------------------------------------
// The line format
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct LineOut<'a> {
    at: String,
    profiles: BTreeMap<&'a str, PointOut>,
}

#[derive(Serialize)]
struct PointOut {
    s: Option<u8>,
    w: u8,
    w_reset: Option<String>,
}

/// UTC time as `YYYY-MM-DDTHH:MM:SSZ`, the rotate log's format.
pub(super) fn iso(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(
        || "1970-01-01T00:00:00Z".to_string(),
        |d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}

/// One history line, without the newline. The session reset is not kept.
pub(super) fn to_line(e: &Entry) -> String {
    let out = LineOut {
        at: iso(e.at),
        profiles: e
            .profiles
            .iter()
            .map(|(n, p)| {
                (
                    n.as_str(),
                    PointOut {
                        s: p.s,
                        w: p.w,
                        w_reset: p.w_reset.map(iso),
                    },
                )
            })
            .collect(),
    };
    // Strings, small integers and options always serialise.
    serde_json::to_string(&out).expect("a history entry serialises to JSON")
}

/// An RFC 3339 time within 100 years of `now`, as epoch seconds.
fn time_of(v: &serde_json::Value, now: i64) -> Option<i64> {
    let t = chrono::DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()?
        .timestamp();
    (t.saturating_sub(now).abs() <= MAX_SKEW).then_some(t)
}

fn pct_in(v: &serde_json::Value) -> Option<u8> {
    v.as_u64().filter(|p| *p <= 100).map(|p| p as u8)
}

/// One profile of a line; `None` drops that profile only.
fn point_in(v: &serde_json::Value, now: i64) -> Option<Point> {
    let o = v.as_object()?;
    let s = match o.get("s") {
        None | Some(serde_json::Value::Null) => None,
        Some(s) => Some(pct_in(s)?),
    };
    let w_reset = match o.get("w_reset") {
        None | Some(serde_json::Value::Null) => None,
        Some(t) => Some(time_of(t, now)?),
    };
    Some(Point {
        s,
        w: pct_in(o.get("w")?)?,
        w_reset,
        s_reset: None,
    })
}

/// Reads one line. `None` for a line over 64 KiB, one that is not a JSON
/// object with a believable `at` and a `profiles` object. A profile whose
/// name fails the name rules or whose numbers are out of range is dropped
/// and the rest of the line kept. Unknown keys are ignored.
pub(super) fn parse_line(line: &str, now: i64) -> Option<Entry> {
    if line.len() > LINE_MAX {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let o = v.as_object()?;
    let at = time_of(o.get("at")?, now)?;
    let profiles = o
        .get("profiles")?
        .as_object()?
        .iter()
        .filter(|(n, _)| ProfileName::parse(n).is_ok())
        .filter_map(|(n, p)| point_in(p, now).map(|p| (n.clone(), p)))
        .collect();
    Some(Entry { at, profiles })
}

/// The readable entries of `bytes`, in file order.
fn entries_in(bytes: &[u8], now: i64) -> Vec<Entry> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|l| l.len() <= LINE_MAX)
        .filter_map(|l| std::str::from_utf8(l).ok())
        .filter_map(|l| parse_line(l, now))
        .collect()
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Whether `profile usage` may record: `<root>/.auto-switch.d` is a private
/// directory of this user. Checking never creates it.
pub(super) fn recording_enabled(root: &Path) -> bool {
    #[cfg(unix)]
    return amplihack_utils::secure_files::check_private_dir(&root.join(STATE_DIR)).is_ok();
    #[cfg(not(unix))]
    {
        let _ = (root, STATE_DIR);
        false
    }
}

/// The last 4 MiB of `file`, from the first whole line when cut.
#[cfg(unix)]
fn read_tail(file: &std::fs::File, len: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let start = len.saturating_sub(TAIL);
    let mut f = file;
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.take(TAIL).read_to_end(&mut buf)?;
    if start > 0 {
        // The first line was cut: start after it.
        let cut = buf
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        buf.drain(..cut);
    }
    Ok(buf)
}

/// The tail of the history, or `None` when it is missing or cannot be
/// trusted: a symlink, not a regular file (a FIFO never blocks the open),
/// another owner, or writable by group or others.
#[cfg(unix)]
fn read_history(root: &Path) -> Option<Vec<u8>> {
    use amplihack_utils::secure_files::owned_by_euid;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let path = root.join(STATE_DIR).join(FILE);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || !owned_by_euid(&meta) || meta.mode() & 0o022 != 0 {
        return None;
    }
    read_tail(&file, meta.len()).ok()
}

#[cfg(not(unix))]
fn read_history(_root: &Path) -> Option<Vec<u8>> {
    None
}

/// The entries with `now - window <= at < now`, oldest first. Empty when
/// the history is missing or cannot be read.
pub(super) fn read_recent(root: &Path, now: i64, window: i64) -> Vec<Entry> {
    let Some(bytes) = read_history(root) else {
        return Vec::new();
    };
    let from = now.saturating_sub(window);
    let mut out: Vec<Entry> = entries_in(&bytes, now)
        .into_iter()
        .filter(|e| e.at >= from && e.at < now)
        .collect();
    out.sort_by_key(|e| e.at);
    out
}

/// The newest readable entry; `None` when there is none.
pub(super) fn last(root: &Path, now: i64) -> Option<Entry> {
    entries_in(&read_history(root)?, now)
        .into_iter()
        .max_by_key(|e| e.at)
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn open_err(e: &std::io::Error) -> HistoryError {
    if e.kind() == std::io::ErrorKind::InvalidData {
        HistoryError::UnsafeFile
    } else {
        HistoryError::WriteFailed
    }
}

/// Takes the writers' lock, trying every 50 ms for up to one second.
#[cfg(unix)]
fn lock(dir: &Path) -> Result<std::fs::File, HistoryError> {
    use amplihack_utils::secure_files::open_private_nofollow;
    let file = open_private_nofollow(&dir.join(LOCK)).map_err(|e| open_err(&e))?;
    let start = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                if start.elapsed() >= LOCK_WAIT {
                    return Err(HistoryError::LockBusy);
                }
                std::thread::sleep(LOCK_RETRY);
            }
            Err(std::fs::TryLockError::Error(_)) => return Err(HistoryError::WriteFailed),
        }
    }
}

/// Whether the history needs a rewrite: over 4 MiB, or a first line that
/// cannot be read or is older than eight days.
#[cfg(unix)]
fn needs_rewrite(file: &std::fs::File, len: u64, now: i64) -> Result<bool, HistoryError> {
    use std::io::{Read, Seek, SeekFrom};
    if len == 0 {
        return Ok(false);
    }
    if len > TAIL {
        return Ok(true);
    }
    let mut f = file;
    f.seek(SeekFrom::Start(0))
        .map_err(|_| HistoryError::WriteFailed)?;
    let mut head = Vec::new();
    f.take(LINE_MAX as u64 + 1)
        .read_to_end(&mut head)
        .map_err(|_| HistoryError::WriteFailed)?;
    let Some(end) = head.iter().position(|b| *b == b'\n') else {
        return Ok(true);
    };
    let first = std::str::from_utf8(&head[..end])
        .ok()
        .and_then(|l| parse_line(l, now));
    Ok(first.is_none_or(|e| e.at < now.saturating_sub(RETENTION)))
}

/// The rewritten history: the readable entries of the tail no older than
/// eight days, as they were written, then `line`, dropping the oldest until
/// the whole is under 3 MiB.
#[cfg(unix)]
fn rewritten(tail: &[u8], line: &str, now: i64) -> Vec<u8> {
    let oldest = now.saturating_sub(RETENTION);
    let mut kept: Vec<&str> = tail
        .split(|b| *b == b'\n')
        .filter(|l| l.len() <= LINE_MAX)
        .filter_map(|l| std::str::from_utf8(l).ok())
        .filter(|l| parse_line(l, now).is_some_and(|e| e.at >= oldest))
        .collect();
    kept.push(line);
    let mut total: usize = kept.iter().map(|l| l.len() + 1).sum();
    let mut skip = 0;
    while total >= REWRITE_MAX && skip + 1 < kept.len() {
        total -= kept[skip].len() + 1;
        skip += 1;
    }
    let mut body = Vec::with_capacity(total);
    for l in &kept[skip..] {
        body.extend_from_slice(l.as_bytes());
        body.push(b'\n');
    }
    body
}

/// Appends `entry` as one line under the writers' lock, trimming first when
/// needed. Never creates `.auto-switch.d`: without a private one it returns
/// [`HistoryError::NoStateDir`], which prints nothing.
#[cfg(unix)]
pub(super) fn append(root: &Path, entry: &Entry, now: i64) -> Result<(), HistoryError> {
    use amplihack_utils::secure_files::{
        check_private_dir, open_private_nofollow, write_private_atomic,
    };
    use std::io::Write;

    let dir = root.join(STATE_DIR);
    check_private_dir(&dir).map_err(|_| HistoryError::NoStateDir)?;
    let _lock = lock(&dir)?;
    // Opened by path after the lock, so a rewrite's rename is never missed.
    let file = open_private_nofollow(&dir.join(FILE)).map_err(|e| open_err(&e))?;
    let len = file
        .metadata()
        .map_err(|_| HistoryError::WriteFailed)?
        .len();
    let line = to_line(entry);
    if needs_rewrite(&file, len, now)? {
        let tail = read_tail(&file, len).map_err(|_| HistoryError::WriteFailed)?;
        match write_private_atomic(&dir, FILE, &rewritten(&tail, &line, now)) {
            Ok(()) => return Ok(()),
            // Too large to grow any further: nothing is appended.
            Err(_) if len > TAIL => return Err(HistoryError::TooLarge),
            // Small enough to append to; the trim is tried again next time.
            Err(_) => {}
        }
    }
    let mut f = &file;
    f.write_all(format!("{line}\n").as_bytes())
        .map_err(|_| HistoryError::WriteFailed)
}

#[cfg(not(unix))]
pub(super) fn append(_root: &Path, _entry: &Entry, _now: i64) -> Result<(), HistoryError> {
    Err(HistoryError::NoStateDir)
}
