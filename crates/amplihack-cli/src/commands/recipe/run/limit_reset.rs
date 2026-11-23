//! The text form of a usage-limit reset (issue #1618): the fragment after
//! `resets `, such as `8:30am (America/Los_Angeles)` or
//! `Oct 8, 12am (America/Los_Angeles)`.
//!
//! Grammar, after cutting the fragment to 128 bytes at a character boundary:
//! an optional date (`Mon D, ` with a three-letter English month in any case
//! and a 1- or 2-digit day), a clock time (`H[:MM]am`, `H[:MM]pm` with H from
//! 1 to 12 in any case, or 24-hour `HH:MM`), exactly one space, then `(`, an
//! IANA zone name, `)`. The zone is looked up only in `chrono-tz`'s
//! compiled-in table. No year, day or zone is ever guessed: anything else
//! gives no reset.
//!
//! Without a date the clock time is placed today in that zone; with one, in
//! this year. A time up to ten minutes in the past stays (already reached);
//! an older one moves to tomorrow, or to next year for a dated reset. A time
//! in a spring-forward gap becomes the first instant after the gap; one in a
//! fall-back overlap becomes the earlier instant. Never clamped.

use chrono::{Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use std::str::FromStr;

const MAX_FRAGMENT: usize = 128;
/// A clock time this far in the past is still today's.
const GRACE_SECS: i64 = 600;
/// Minutes walked forward through a daylight-saving gap at most.
const MAX_GAP_MINUTES: i64 = 180;
const MONTHS: [&[u8]; 12] = [
    b"jan", b"feb", b"mar", b"apr", b"may", b"jun", b"jul", b"aug", b"sep", b"oct", b"nov", b"dec",
];

/// The instant `fragment` names, relative to `now` (Unix seconds).
pub(super) fn parse(fragment: &str, now: i64) -> Option<i64> {
    let mut end = fragment.len().min(MAX_FRAGMENT);
    while !fragment.is_char_boundary(end) {
        end -= 1;
    }
    let bytes = &fragment.as_bytes()[..end];
    let (date, bytes) = match date(bytes) {
        Some((month, day, rest)) => (Some((month, day)), rest),
        None => (None, bytes),
    };
    let (hour, minute, rest) = clock(bytes)?;
    let tz = Tz::from_str(zone(rest)?).ok()?;
    let today = tz.timestamp_opt(now, 0).single()?.date_naive();
    let at = |date: NaiveDate| resolve(&tz, date.and_hms_opt(hour, minute, 0)?);
    let (first, later) = match date {
        None => (Some(today), today.succ_opt()),
        Some((month, day)) => (
            NaiveDate::from_ymd_opt(today.year(), month, day),
            NaiveDate::from_ymd_opt(today.year() + 1, month, day),
        ),
    };
    match first.and_then(at) {
        Some(t) if t >= now.saturating_sub(GRACE_SECS) => Some(t),
        _ => at(later?),
    }
}

/// `(month 1-12, day, the bytes after "Mon D, ")`.
fn date(b: &[u8]) -> Option<(u32, u32, &[u8])> {
    let mon = b.get(..3)?.to_ascii_lowercase();
    let month = MONTHS.iter().position(|m| *m == mon.as_slice())?;
    let rest = b[3..].strip_prefix(b" ")?;
    let d_len = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    if !(1..=2).contains(&d_len) {
        return None;
    }
    let after = rest[d_len..].strip_prefix(b", ")?;
    Some((month as u32 + 1, number(&rest[..d_len]), after))
}

/// `(hour 0-23, minute, the bytes after the clock time)`.
fn clock(b: &[u8]) -> Option<(u32, u32, &[u8])> {
    let digits = |s: &[u8]| s.iter().take_while(|c| c.is_ascii_digit()).count();
    let h_len = digits(b);
    if !(1..=2).contains(&h_len) {
        return None;
    }
    let hour = number(&b[..h_len]);
    let mut rest = &b[h_len..];
    let mut minute = None;
    if let Some(after) = rest.strip_prefix(b":") {
        if digits(after) != 2 {
            return None;
        }
        minute = Some(number(&after[..2]));
        rest = &after[2..];
    }
    let meridiem = match rest.get(..2).map(|m| m.to_ascii_lowercase()) {
        Some(m) if m == b"am" => Some(0),
        Some(m) if m == b"pm" => Some(12),
        _ => None,
    };
    match meridiem {
        Some(add) => {
            if !(1..=12).contains(&hour) || minute.is_some_and(|m| m > 59) {
                return None;
            }
            Some((hour % 12 + add, minute.unwrap_or(0), &rest[2..]))
        }
        // 24-hour form: two-digit hour and minutes both required.
        None => {
            let minute = minute?;
            (h_len == 2 && hour <= 23 && minute <= 59).then_some((hour, minute, rest))
        }
    }
}

fn number(d: &[u8]) -> u32 {
    d.iter().fold(0, |acc, c| acc * 10 + u32::from(c - b'0'))
}

/// The zone name in ` (<zone>)`, exactly one space first. Text after the
/// closing parenthesis is ignored.
fn zone(rest: &[u8]) -> Option<&str> {
    let inner = rest.strip_prefix(b" (")?;
    let close = inner.iter().position(|&c| c == b')')?;
    let name = std::str::from_utf8(&inner[..close]).ok()?;
    (!name.is_empty()).then_some(name)
}

/// The instant of local time `naive` in `tz`: the earlier one in an overlap,
/// the first valid one after a gap.
fn resolve(tz: &Tz, naive: NaiveDateTime) -> Option<i64> {
    (0..=MAX_GAP_MINUTES).find_map(|k| {
        let t = naive.checked_add_signed(Duration::minutes(k))?;
        match tz.from_local_datetime(&t) {
            LocalResult::Single(d) => Some(d.timestamp()),
            LocalResult::Ambiguous(a, b) => Some(a.timestamp().min(b.timestamp())),
            LocalResult::None => None,
        }
    })
}
