//! Issue #1797: `<root>/.auto-switch.d/usage-history.jsonl`, one line per
//! measured cycle, read by the capacity summary for its rate and by `rotate
//! status` for its `capacity:` line.
//!
//! A line holds the time, profile names, whole-number session and weekly
//! percentages and the weekly reset time. Nothing else from a probe is kept:
//! no identity, no plan, no error text and nothing Claude printed.
//!
//! [`entry_of`] is the one validity rule. Readers take the last 4 MiB and
//! skip whatever they cannot trust; a history that exists but is refused
//! gives one fixed warning. Writers check the profile root, refuse while a
//! credential variable is set, lock `usage-history.lock`, append one line in
//! one write, trim entries older than eight days, and never create
//! `.auto-switch.d`. No failure here ever fails a command.
//!
//! Issue #1970: before a line is written, each passive reading is checked
//! with [`stale`] against its login's newest reading in the history; a stale
//! one is replaced by that recorded reading, so the login stays in the line
//! ([`stale_replaced`]).

use super::name::ProfileName;
use super::usage_cmd::{Row, utc_time};
use super::usage_parse::{UsageReport, UsageState, UsageWindow};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// The history file, inside `<root>/.auto-switch.d`.
pub(super) const FILE: &str = "usage-history.jsonl";
/// The writers' lock file, beside the history.
pub(super) const LOCK: &str = "usage-history.lock";
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) const STATE_DIR: &str = ".auto-switch.d";
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
/// A rewrite starts only once the first line is this much older than
/// [`RETENTION`]. Without the slack, a history under 4 MiB would be
/// rewritten whole on every cycle once it held eight days; with it, at most
/// once an hour.
#[cfg(unix)]
const TRIM_SLACK: i64 = 3600;
/// Times further than this from now are not believed.
const MAX_SKEW: i64 = 100 * 31_557_600;
/// The oldest reading a cycle counts, in seconds before the cycle's time.
///
/// Every point of a line is taken as read at the line's `at`. A reading
/// from a direct read or `claude` is: it was taken during the cycle. A
/// passive reading (issue #1844) was stored by Claude's status line up to
/// `AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS` earlier, `300` by default
/// and up to `3600`. Up to 300 s, stamping it with the line's time moves it
/// by at most 300 s, under 2% of the shortest span a rate is taken over
/// (4.5 h). An older reading is unknown in the snapshot, so it is in no
/// figure, no rate and no history point; the line lists it as unknown.
pub(super) const READING_MAX_AGE: i64 = 300;
/// Issue #1970: two weekly reset times this close are the same week. A
/// reading's reset time can move by a few seconds between reads.
pub(super) const SAME_WEEK_SLACK: i64 = 3600;
/// Issue #1970: a writer compares passive readings with the newest readings
/// in at most this many bytes from the end of the history.
#[cfg(unix)]
pub(super) const STALE_LOOKBACK: u64 = 256 * 1024;
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
    /// Issue #1818: the reading showed no session window, so no session
    /// limit applies and the login is usable on its week alone. Only ever
    /// `true` with `s` `None`; a session that was simply not read is
    /// `false`. Written to the history as `"session_limit": false`.
    pub(super) no_session_limit: bool,
    /// Issue #1970: a passive reading, the figure Claude's status line
    /// stored rather than one read during the cycle. It may be a cached
    /// figure older than the history's newest, so [`append_with`] checks it
    /// with [`stale`] before writing it. Never written to the history: a
    /// parsed point is always `false`.
    pub(super) passive: bool,
}

/// Which command wrote a history line. `profile usage` reads every
/// subscription login; a rotate cycle records only the profiles it probed,
/// so the two give different `N` and `rotate status` names the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Writer {
    Usage,
    Rotate,
}

impl Writer {
    fn key(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Rotate => "rotate",
        }
    }

    fn of_key(k: &str) -> Option<Self> {
        match k {
            "usage" => Some(Self::Usage),
            "rotate" => Some(Self::Rotate),
            _ => None,
        }
    }
}

/// One history line: the valid readings of one cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) at: i64,
    pub(super) profiles: BTreeMap<String, Point>,
    /// Who wrote the line; `None` for a line written before `by` existed,
    /// or with a writer this version does not know.
    pub(super) by: Option<Writer>,
    /// The logins this writer tried and could not read, sorted by name.
    /// Only names: never counted in a figure. Empty for older lines.
    pub(super) unknown: Vec<String>,
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
    /// The history entry of this cycle, written by `by`: its valid
    /// readings only.
    pub(super) fn entry(&self, by: Writer) -> Entry {
        Entry {
            at: self.at,
            profiles: self.points.clone(),
            by: Some(by),
            unknown: self.unknown.clone(),
        }
    }
}

/// Why a line was not written, or was written without its trim. Fixed
/// reasons: never a path or system text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HistoryError {
    /// The profile root is not trusted, or `.auto-switch.d` is missing or
    /// not private: recording is off.
    NoStateDir,
    /// A credential variable is set, so the readings may all be one
    /// credential's. The caller already warns about the variable.
    CredentialVar,
    LockBusy,
    UnsafeFile,
    WriteFailed,
    TooLarge,
    /// The line was appended but the trim rewrite failed.
    TrimFailed,
}

/// The one warning when a history exists but is refused or unreadable.
pub(super) const READ_REFUSED: &str = "warning: usage history ignored (unsafe file)";

impl HistoryError {
    /// The one stderr or rotate-log line, or `None` when recording is off.
    pub(super) fn warning(self) -> Option<String> {
        let reason = match self {
            Self::NoStateDir | Self::CredentialVar => return None,
            Self::TrimFailed => return Some("warning: usage history not trimmed".to_string()),
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

/// Whether a reading taken at `observed_at` counts as of `at`: always for
/// one taken during the read (`None`), and for an earlier one only up to
/// [`READING_MAX_AGE`] old.
pub(super) fn recent_enough(observed_at: Option<i64>, at: i64) -> bool {
    observed_at.is_none_or(|o| at.saturating_sub(o) <= READING_MAX_AGE)
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
    // Issue #1818: positive evidence only; the parser never sets it with a
    // session window, and the check keeps that true here.
    let no_session_limit = !r.session_limit && session.is_none();
    // Issue #1970: a reading with a capture time was stored earlier by
    // Claude's status line, not read during this run.
    let passive = r.observed_at.is_some();
    match r.state {
        UsageState::Ok => Some(Point {
            s: pct_of(session),
            w: pct_of(week)?,
            w_reset,
            s_reset,
            no_session_limit,
            passive,
        }),
        UsageState::SessionExhausted => Some(Point {
            s: Some(100),
            w: pct_of(week)?,
            w_reset,
            s_reset,
            no_session_limit: false,
            passive,
        }),
        UsageState::WeeklyExhausted => Some(Point {
            s: pct_of(session),
            w: 100,
            w_reset,
            s_reset,
            no_session_limit,
            passive,
        }),
        _ => None,
    }
}

/// The snapshot of a `profile usage` run; `None` when no row is a
/// subscription login, which means neither a summary nor a history line.
/// API-key rows and the `~/.claude` row are not logins and are left out; a
/// live profile whose login was swapped during the probe is unknown, since
/// its numbers may be another login's. So is a passive reading older than
/// [`READING_MAX_AGE`], and so is every row that could not be read: an
/// `error` row, whether or not a direct read failed first, is never 0% or
/// 100% used.
///
/// Two times: `read_at` is when the run started reading, the `now` its
/// passive readings were accepted against; `at` is when the readings were
/// all in, the line's time. A reading's age is judged at `read_at`, the
/// same clock that accepted it, so a slow `claude` fallback elsewhere in
/// the run never turns a row the table shows as read into an unknown one.
pub(super) fn from_rows(
    read_at: i64,
    at: i64,
    rows: &[Row],
    swapped: Option<&ProfileName>,
) -> Option<Snapshot> {
    let mut points = BTreeMap::new();
    let mut unknown = Vec::new();
    let logins = rows.iter().filter(|r| !r.api_key);
    let mut any = false;
    for r in logins {
        any = true;
        let moved = swapped.is_some_and(|s| s.as_str() == r.profile);
        let counted = !moved && recent_enough(r.report.observed_at, read_at);
        match entry_of(&r.report).filter(|_| counted) {
            Some(p) => {
                points.insert(r.profile.clone(), p);
            }
            None => unknown.push(r.profile.clone()),
        }
    }
    unknown.sort();
    any.then_some(Snapshot {
        at,
        points,
        unknown,
    })
}

/// The snapshot of a rotator cycle. The live probe is recorded under
/// `live`, the live login's profile, and dropped when the login belongs to
/// no profile. The API-key profile `platform` is not a login and is never
/// recorded or counted as unknown. `unmeasured` are the plan logins the
/// cycle did not probe (`Snap::unmeasured_plan`: skipped as busy, or a
/// `Skip::NoAccount` profile): they were not read, so
/// they are listed as unknown and the line says how many it left out. A
/// passive reading older than [`READING_MAX_AGE`] is unknown too.
#[cfg(unix)]
pub(super) fn from_probes(
    at: i64,
    live: Option<&ProfileName>,
    probes: &super::rotate::Probes,
    platform: Option<&ProfileName>,
    unmeasured: &[ProfileName],
) -> Snapshot {
    let (live_probe, others) = probes;
    let mut points = BTreeMap::new();
    let mut unknown = Vec::new();
    let named = live
        .map(|n| (n, live_probe))
        .into_iter()
        .chain(others.iter().map(|(n, p)| (n, p)));
    for (name, probe) in named.filter(|(n, _)| platform != Some(*n)) {
        match point_of_probe(probe).filter(|_| recent_enough(probe.observed_at, at)) {
            Some(p) => {
                points.insert(name.as_str().to_string(), p);
            }
            None => unknown.push(name.as_str().to_string()),
        }
    }
    for name in unmeasured.iter().filter(|n| platform != Some(*n)) {
        if !points.contains_key(name.as_str()) {
            unknown.push(name.as_str().to_string());
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
/// a full week when its week reads 100%, and otherwise a full session with
/// the week it read. A probe without a readable week is unknown, as is a
/// failed probe: the week is never taken to be 0% or 100% unread.
#[cfg(unix)]
fn point_of_probe(p: &super::rotate::decide::Probed) -> Option<Point> {
    use super::rotate::decide::Reading;
    let session = p.windows.session.as_ref();
    let week = p.windows.week.as_ref();
    let s = session.and_then(|w| whole_pct(w.used));
    let s_reset = session.and_then(|w| w.resets_at);
    let w_reset = week.and_then(|w| w.resets_at);
    // Issue #1818: as in `entry_of`, positive evidence only. `Reading`
    // counts a missing session as 0% used; the point keeps it unread and
    // says why.
    let no_session_limit = p.windows.no_session_limit && session.is_none();
    // Issue #1970: the probe says whether it is a passive reading. Its
    // capture time cannot: a #1870 re-read is a direct read that has one.
    let passive = p.passive;
    match p.reading {
        Reading::Failed => None,
        Reading::Measured { week: used, .. } => Some(Point {
            s,
            w: whole_pct(used)?,
            w_reset,
            s_reset,
            no_session_limit,
            passive,
        }),
        Reading::Exhausted { .. } => match week.map(|w| whole_pct(w.used)) {
            Some(Some(100)) => Some(Point {
                s,
                w: 100,
                w_reset,
                s_reset,
                no_session_limit,
                passive,
            }),
            Some(Some(w)) => Some(Point {
                s: Some(100),
                w,
                w_reset,
                s_reset,
                no_session_limit: false,
                passive,
            }),
            Some(None) | None => None,
        },
    }
}

// ---------------------------------------------------------------------------
// Stale passive readings (issue #1970)
// ---------------------------------------------------------------------------

/// Whether `new`, a passive reading for a line taken at `at`, is stale
/// against `prev`, that login's newest reading in the history, if any.
///
/// Claude's status line can hand back a figure it cached earlier, so a
/// lower figure can reappear between rising ones. A passive reading is
/// stale when:
///
/// 1. its week has ended: its `w_reset` is at or before `at`, whatever the
///    history holds;
/// 2. its week is the recorded one (reset times within [`SAME_WEEK_SLACK`])
///    and its `w` is lower than the recorded `w`; or
/// 3. its week is more than [`SAME_WEEK_SLACK`] earlier than the recorded
///    one.
///
/// A reading whose own reset time is unknown is never stale, and with no
/// recorded reset time only rule 1 applies; the reset rule of the rate
/// covers those cases. The caller decides which readings are passive.
pub(super) fn stale(prev: Option<&Point>, new: &Point, at: i64) -> bool {
    let Some(reset) = new.w_reset else {
        return false;
    };
    if reset <= at {
        return true;
    }
    let Some((prev_w, prev_reset)) = prev.and_then(|p| p.w_reset.map(|r| (p.w, r))) else {
        return false;
    };
    if reset.abs_diff(prev_reset) <= SAME_WEEK_SLACK.unsigned_abs() {
        new.w < prev_w
    } else {
        reset < prev_reset
    }
}

/// Each login's newest reading in `bytes`, read only through
/// [`parse_line`]. Of two readings taken at the same time, the later line
/// wins.
#[cfg(unix)]
fn newest_points(bytes: &[u8], now: i64) -> BTreeMap<String, (i64, Point)> {
    let mut newest: BTreeMap<String, (i64, Point)> = BTreeMap::new();
    for e in lines(bytes).filter_map(|l| parse_line(l, now)) {
        for (name, p) in e.profiles {
            if newest.get(&name).is_none_or(|(at, _)| *at <= e.at) {
                newest.insert(name, (e.at, p));
            }
        }
    }
    newest
}

/// `entry` with each stale passive reading ([`stale`] against `newest`)
/// replaced by that login's newest recorded reading.
///
/// The login stays in the line. Passive readings come from Claude's status
/// line, so a stale one belongs to a login in use, and a line without it
/// would leave the busiest login out: a `profile usage` start would drop it
/// from the rate, and `rotate status` would report a rotator line as having
/// left a login out (`capacity_summary::status_line`). The recorded figure
/// adds nothing to the rate, because the week has not risen above it, and
/// it is a lower bound of the true figure, which only rises within a week,
/// so headroom is never overstated by the cached figure.
///
/// A recorded reading is used only while its own week has not ended at
/// `entry.at`. Without one (a reading from an ended week, with no newer
/// recorded figure of the current week) nothing known is left to write:
/// the login is listed as unknown, as a failed reading would be.
#[cfg(unix)]
fn stale_replaced(entry: &Entry, newest: &BTreeMap<String, (i64, Point)>) -> Entry {
    let mut out = entry.clone();
    for (name, p) in &entry.profiles {
        let recorded = newest.get(name).map(|(_, q)| q);
        if !p.passive || !stale(recorded, p, entry.at) {
            continue;
        }
        let current_week = recorded.filter(|q| q.w_reset.is_some_and(|r| r > entry.at));
        match current_week {
            Some(q) => {
                out.profiles.insert(name.clone(), q.clone());
            }
            None => {
                out.profiles.remove(name);
                out.unknown.push(name.clone());
            }
        }
    }
    out.unknown.sort();
    out.unknown.dedup();
    out
}

// ---------------------------------------------------------------------------
// The line format
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct LineOut<'a> {
    at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    by: Option<&'static str>,
    profiles: BTreeMap<&'a str, PointOut>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    unknown: &'a [String],
}

#[derive(Serialize)]
struct PointOut {
    s: Option<u8>,
    /// Written only as `false`, for a login with no session limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    session_limit: Option<bool>,
    w: u8,
    w_reset: Option<String>,
}

/// One history line, without the newline. The session reset is not kept.
pub(super) fn to_line(e: &Entry) -> String {
    let out = LineOut {
        at: utc_time(e.at),
        by: e.by.map(Writer::key),
        profiles: e
            .profiles
            .iter()
            .map(|(n, p)| {
                (
                    n.as_str(),
                    PointOut {
                        s: p.s,
                        session_limit: (p.no_session_limit && p.s.is_none()).then_some(false),
                        w: p.w,
                        w_reset: p.w_reset.map(utc_time),
                    },
                )
            })
            .collect(),
        unknown: &e.unknown,
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
    // Issue #1818: only an explicit `false` with no session figure means no
    // session limit; missing or `null` is the old meaning, "not read".
    let no_session_limit = match o.get("session_limit") {
        None | Some(serde_json::Value::Null) => false,
        Some(v) => !v.as_bool()? && s.is_none(),
    };
    Some(Point {
        s,
        w: pct_in(o.get("w")?)?,
        w_reset,
        s_reset: None,
        no_session_limit,
        // A line never says how a reading was taken.
        passive: false,
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
    let profiles: BTreeMap<String, Point> = o
        .get("profiles")?
        .as_object()?
        .iter()
        .filter(|(n, _)| ProfileName::parse(n).is_ok())
        .filter_map(|(n, p)| point_in(p, now).map(|p| (n.clone(), p)))
        .collect();
    let by = o
        .get("by")
        .and_then(serde_json::Value::as_str)
        .and_then(Writer::of_key);
    // Issue #1797 review: unknown logins by name. A name that fails the
    // name rules, or that also has a reading, is dropped; a value that is
    // not an array is ignored, as an unknown key would be.
    let mut unknown: Vec<String> = o
        .get("unknown")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|n| ProfileName::parse(n).is_ok() && !profiles.contains_key(*n))
        .map(str::to_string)
        .collect();
    unknown.sort();
    unknown.dedup();
    Some(Entry {
        at,
        profiles,
        by,
        unknown,
    })
}

/// The UTF-8 lines of `bytes`, in file order.
#[cfg(unix)]
fn lines(bytes: &[u8]) -> impl Iterator<Item = &str> {
    bytes
        .split(|b| *b == b'\n')
        .filter_map(|l| std::str::from_utf8(l).ok())
}

/// The readable entries of `bytes`, in file order, and how many non-empty
/// lines were not readable (not UTF-8, or refused by [`parse_line`]).
fn entries_in(bytes: &[u8], now: i64) -> (Vec<Entry>, usize) {
    let mut skipped = 0;
    let mut out = Vec::new();
    for l in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        match std::str::from_utf8(l).ok().and_then(|l| parse_line(l, now)) {
            Some(e) => out.push(e),
            None => skipped += 1,
        }
    }
    (out, skipped)
}

/// The one warning when lines of a readable history were ignored.
pub(super) fn skipped_warning(n: usize) -> String {
    format!("warning: {n} unreadable usage history lines ignored")
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// The last 4 MiB of `file`, from the first whole line when cut.
#[cfg(unix)]
fn read_tail(file: &std::fs::File, len: u64) -> std::io::Result<Vec<u8>> {
    read_last(file, len, TAIL)
}

/// The last `max` bytes of `file`, whose length is `len`, from the first
/// whole line when cut.
#[cfg(unix)]
fn read_last(file: &std::fs::File, len: u64, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let start = len.saturating_sub(max);
    let mut f = file;
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.take(max).read_to_end(&mut buf)?;
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

/// A history that exists but was not read: [`READ_REFUSED`] says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReadRefused;

/// The tail of the history; empty when it or `.auto-switch.d` is missing.
/// Refused when the profile root is not trusted, or the file cannot be
/// opened, is a symlink (a FIFO never blocks the open), fails the writers'
/// rule (`check_private_file`: a regular file of the current user with one
/// hard link and mode 0600), or cannot be read. Readers and writers
/// therefore accept exactly the same files.
#[cfg(unix)]
fn read_history(root: &Path) -> Result<Vec<u8>, ReadRefused> {
    use amplihack_utils::secure_files::check_private_file;
    use std::io::ErrorKind::NotFound;
    use std::os::unix::fs::OpenOptionsExt;
    match super::auto_switch::check_root_private(root) {
        Ok(()) => {}
        Err(e) if e.kind() == NotFound => return Ok(Vec::new()),
        Err(_) => return Err(ReadRefused),
    }
    let path = root.join(STATE_DIR).join(FILE);
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == NotFound => return Ok(Vec::new()),
        Err(_) => return Err(ReadRefused),
    };
    let meta = file.metadata().map_err(|_| ReadRefused)?;
    check_private_file(&meta).map_err(|_| ReadRefused)?;
    read_tail(&file, meta.len()).map_err(|_| ReadRefused)
}

#[cfg(not(unix))]
fn read_history(_root: &Path) -> Result<Vec<u8>, ReadRefused> {
    Ok(Vec::new())
}

/// Every readable entry, oldest first; entries with the same time stay in
/// file order. Empty when the history is missing or refused, with
/// [`READ_REFUSED`] as the warning when refused. Lines that cannot be read
/// are skipped and counted in one [`skipped_warning`], so a corrupted
/// history does not look like an empty one. One read parses the whole
/// tail, so a caller that needs both the newest entry and the ones before
/// it reads once.
pub(super) fn read_all(root: &Path, now: i64) -> (Vec<Entry>, Option<String>) {
    match read_history(root) {
        Ok(bytes) => {
            let (mut out, skipped) = entries_in(&bytes, now);
            out.sort_by_key(|e| e.at);
            (out, (skipped > 0).then(|| skipped_warning(skipped)))
        }
        Err(ReadRefused) => (Vec::new(), Some(READ_REFUSED.to_string())),
    }
}

/// The entries with `now - window <= at < now`, oldest first, and the
/// warning of [`read_all`].
pub(super) fn read_recent(root: &Path, now: i64, window: i64) -> (Vec<Entry>, Option<String>) {
    let from = now.saturating_sub(window);
    let (all, warning) = read_all(root, now);
    let recent = all
        .into_iter()
        .filter(|e| e.at >= from && e.at < now)
        .collect();
    (recent, warning)
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
/// cannot be read or is more than an hour older than eight days.
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
    let cutoff = now.saturating_sub(RETENTION).saturating_sub(TRIM_SLACK);
    Ok(first.is_none_or(|e| e.at < cutoff))
}

/// The rewritten history: the readable entries of the tail no older than
/// eight days, as they were written, then `line`, dropping the oldest until
/// the whole is under 3 MiB.
#[cfg(unix)]
fn rewritten(tail: &[u8], line: &str, now: i64) -> Vec<u8> {
    let oldest = now.saturating_sub(RETENTION);
    let mut kept: Vec<&str> = lines(tail)
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

/// Whether a non-empty history does not end in a newline.
#[cfg(unix)]
fn ends_cut(file: &std::fs::File, len: u64) -> Result<bool, HistoryError> {
    use std::os::unix::fs::FileExt;
    if len == 0 {
        return Ok(false);
    }
    let mut last = [0u8];
    file.read_exact_at(&mut last, len - 1)
        .map_err(|_| HistoryError::WriteFailed)?;
    Ok(last[0] != b'\n')
}

/// Appends `entry` as one line, applying every recording rule; see
/// [`append_with`]. The credential variables are read from the environment.
pub(super) fn append(root: &Path, entry: &Entry, now: i64) -> Result<(), HistoryError> {
    let cred = super::usage_cmd::env_credential_var(|k| std::env::var_os(k));
    append_with(root, entry, now, cred)
}

/// Appends `entry` as one line under the writers' lock, trimming first when
/// needed. Nothing is written while `cred`, a credential variable, is set
/// ([`HistoryError::CredentialVar`]), or when the profile root is not
/// trusted or `.auto-switch.d` is missing or not private
/// ([`HistoryError::NoStateDir`]); neither prints anything. Never creates
/// `.auto-switch.d`. A line appended after a failed trim gives
/// [`HistoryError::TrimFailed`]. A passive reading that is [`stale`]
/// against its login's newest reading in the last 256 KiB is replaced by
/// that reading ([`stale_replaced`], issue #1970).
#[cfg(unix)]
pub(super) fn append_with(
    root: &Path,
    entry: &Entry,
    now: i64,
    cred: Option<&'static str>,
) -> Result<(), HistoryError> {
    use amplihack_utils::secure_files::{
        check_private_dir, open_private_nofollow, write_private_atomic,
    };
    use std::io::Write;

    if cred.is_some() {
        return Err(HistoryError::CredentialVar);
    }
    super::auto_switch::check_root_private(root).map_err(|_| HistoryError::NoStateDir)?;
    let dir = root.join(STATE_DIR);
    check_private_dir(&dir).map_err(|_| HistoryError::NoStateDir)?;
    let _lock = lock(&dir)?;
    // Opened by path after the lock, so a rewrite's rename is never missed.
    let file = open_private_nofollow(&dir.join(FILE)).map_err(|e| open_err(&e))?;
    let len = file
        .metadata()
        .map_err(|_| HistoryError::WriteFailed)?
        .len();
    // Issue #1970: a passive reading lower than the recorded one for its
    // week, or from an ended week, is a stale cached figure and is not
    // written as new history; the recorded figure is written instead. Read
    // from the handle already checked, under the lock, and before the line
    // is made, so a rewrite writes the filtered line too. A failed read
    // writes nothing rather than the unfiltered line.
    let filtered;
    let entry = if entry.profiles.values().any(|p| p.passive) {
        let tail = read_last(&file, len, STALE_LOOKBACK).map_err(|_| HistoryError::WriteFailed)?;
        filtered = stale_replaced(entry, &newest_points(&tail, now));
        &filtered
    } else {
        entry
    };
    let line = to_line(entry);
    let mut trim_failed = false;
    if needs_rewrite(&file, len, now)? {
        let tail = read_tail(&file, len).map_err(|_| HistoryError::WriteFailed)?;
        match write_private_atomic(&dir, FILE, &rewritten(&tail, &line, now)) {
            Ok(()) => return Ok(()),
            // Too large to grow any further: nothing is appended.
            Err(_) if len > TAIL => return Err(HistoryError::TooLarge),
            // Small enough to append to; the trim is tried again next time.
            Err(_) => trim_failed = true,
        }
    }
    // A last line cut short (by a full disk, say) is ended first, so this
    // line is not joined onto it and lost with it.
    let lead = if ends_cut(&file, len)? { "\n" } else { "" };
    let mut f = &file;
    f.write_all(format!("{lead}{line}\n").as_bytes())
        .map_err(|_| HistoryError::WriteFailed)?;
    if trim_failed {
        return Err(HistoryError::TrimFailed);
    }
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn append_with(
    _root: &Path,
    _entry: &Entry,
    _now: i64,
    _cred: Option<&'static str>,
) -> Result<(), HistoryError> {
    Err(HistoryError::NoStateDir)
}
