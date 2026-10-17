//! Parser for `claude -p /usage` output (issue #1578). Pure and
//! platform-neutral: every time is computed from the caller's `now` and the
//! IANA zone Claude printed, never the host's zone. Total: no input panics,
//! and a value that cannot be read is left out rather than guessed.

use chrono::{Datelike, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use chrono_tz::Tz;
use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

/// Phrases meaning the profile is not signed in. Matched without regard to
/// case in stdout and stderr, and only when no usage line was read. Keep in
/// step with the phrase table in docs/reference/profile-command.md.
pub const NOT_LOGGED_IN: &[&str] = &[
    "not logged in",
    "run /login",
    "invalid api key",
    "login required",
    "authentication_error",
    "oauth token has expired",
];

/// Phrases meaning the organisation's spend limit was reached. Matched like
/// [`NOT_LOGGED_IN`], whatever else was read.
pub const SPEND_LIMIT: &[&str] = &[
    "spend limit",
    "spending limit",
    "usage limit for your organization",
    "organization has reached",
];

/// A percentage above this is not a reading.
const MAX_PCT: f64 = 1_000_000.0;
const DAY: i64 = 86_400;
/// How far in the past a time-only reset may be and still mean today.
/// Claude prints whole minutes, so a reset due in the current minute can
/// read as up to a minute ago; session windows last five hours, so a time
/// this recent never means tomorrow.
const TIME_ONLY_SLACK: i64 = 600;
const ALL_MODELS: &str = "all models";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageState {
    Ok,
    SessionExhausted,
    WeeklyExhausted,
    SpendLimit,
    NotLoggedIn,
    Timeout,
    Unparsed,
    Error,
}

impl UsageState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::SessionExhausted => "session_exhausted",
            Self::WeeklyExhausted => "weekly_exhausted",
            Self::SpendLimit => "spend_limit",
            Self::NotLoggedIn => "not_logged_in",
            Self::Timeout => "timeout",
            Self::Unparsed => "unparsed",
            Self::Error => "error",
        }
    }
}

/// One `Current …: N% used[ · resets …]` line. Claude leaves the reset
/// clause out when the window has not started (no session open).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageWindow {
    /// `session`, `all models`, or the model name as printed.
    pub label: String,
    /// As read, not clamped.
    pub used_percentage: f64,
    /// The reset text exactly as printed (trimmed), or `None` when the line
    /// has no reset clause.
    pub resets_text: Option<String>,
    /// `resets_text` as Unix epoch seconds, when it could be resolved.
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageReport {
    pub state: UsageState,
    pub session: Option<UsageWindow>,
    pub week_all: Option<UsageWindow>,
    pub week_models: Vec<UsageWindow>,
}

impl UsageReport {
    /// A report with no windows, for states decided outside the parser.
    pub fn empty(state: UsageState) -> Self {
        Self {
            state,
            session: None,
            week_all: None,
            week_models: Vec::new(),
        }
    }
}

static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*Current (session|week \(([^)]+)\)):\s*([0-9]+(?:\.[0-9]+)?)%\s*used(?:\s*[·•\-]?\s*resets\s+(.+?))?\s*$",
    )
    .expect("usage line regex")
});

/// `[Mon D[, YYYY], ]H[:MM]am|pm (Zone)`.
static RESET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:([a-z]{3})\s+([0-9]{1,2})\s*,\s*(?:([0-9]{4})\s*,\s*)?)?([0-9]{1,2})(?::([0-9]{2}))?\s*(am|pm)\s*\(([^()\s]+)\)\s*$",
    )
    .expect("reset regex")
});

/// Reads `/usage` output. Usage lines come from stdout only; the phrases are
/// looked for in both streams. The exit status is deliberately not an input:
/// output that parses counts even after a non-zero exit.
pub fn parse_usage(stdout: &str, stderr: &str, now: i64) -> UsageReport {
    let mut report = UsageReport::empty(UsageState::Unparsed);
    for line in stdout.lines() {
        let Some((week, w)) = parse_line(line, now) else {
            continue;
        };
        // The first session and all-models lines count; repeats are ignored.
        let slot = match (week, w.label.as_str()) {
            (false, _) => &mut report.session,
            (true, ALL_MODELS) => &mut report.week_all,
            (true, _) => {
                report.week_models.push(w);
                continue;
            }
        };
        if slot.is_none() {
            *slot = Some(w);
        }
    }
    let text = format!("{stdout}\n{stderr}").to_lowercase();
    let says = |phrases: &[&str]| phrases.iter().any(|p| text.contains(p));
    let exhausted =
        |w: &Option<UsageWindow>| w.as_ref().is_some_and(|w| w.used_percentage >= 100.0);
    let any_line =
        report.session.is_some() || report.week_all.is_some() || !report.week_models.is_empty();
    report.state = if says(SPEND_LIMIT) {
        UsageState::SpendLimit
    } else if exhausted(&report.session) {
        UsageState::SessionExhausted
    } else if exhausted(&report.week_all) {
        UsageState::WeeklyExhausted
    } else if report.session.is_some() || report.week_all.is_some() {
        UsageState::Ok
    } else if !any_line && says(NOT_LOGGED_IN) {
        UsageState::NotLoggedIn
    } else {
        UsageState::Unparsed
    };
    report
}

/// One usage line, and whether it is a week line.
fn parse_line(line: &str, now: i64) -> Option<(bool, UsageWindow)> {
    let caps = LINE.captures(line)?;
    let week = caps.get(2);
    let label = week.map_or("session", |m| m.as_str()).to_string();
    let pct: f64 = caps[3].parse().ok()?;
    if !pct.is_finite() || pct > MAX_PCT {
        return None;
    }
    let resets_text = caps.get(4).map(|m| m.as_str().to_string());
    let resets_at = resets_text.as_deref().and_then(|t| resolve_reset(t, now));
    let window = UsageWindow {
        label,
        used_percentage: pct,
        resets_text,
        resets_at,
    };
    Some((week.is_some(), window))
}

/// Turns Claude's reset text into Unix epoch seconds, or `None` for an
/// unknown zone, a time that does not exist there (a daylight-saving gap), an
/// invalid date or any other form. A repeated time resolves to the earlier.
///
/// Without a year, the year chosen puts the time in `[now − 1 day, now + 366
/// days)`, trying this year, then next, then last. A time alone is today in
/// that zone, or tomorrow if it passed more than [`TIME_ONLY_SLACK`] ago.
pub fn resolve_reset(text: &str, now: i64) -> Option<i64> {
    let caps = RESET.captures(text)?;
    let tz: Tz = caps[7].parse().ok()?;
    let hour12: u32 = caps[4].parse().ok()?;
    let minute: u32 = caps.get(5).map_or(Some(0), |m| m.as_str().parse().ok())?;
    if !(1..=12).contains(&hour12) {
        return None;
    }
    let hour = hour12 % 12
        + if caps[6].eq_ignore_ascii_case("pm") {
            12
        } else {
            0
        };
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    let today = chrono::DateTime::from_timestamp(now, 0)?
        .with_timezone(&tz)
        .date_naive();

    let Some(month) = caps.get(1) else {
        let at = instant(tz, today.and_time(time))?;
        if at >= now.saturating_sub(TIME_ONLY_SLACK) {
            return Some(at);
        }
        return instant(tz, today.succ_opt()?.and_time(time));
    };
    let month = month_number(month.as_str())?;
    let day: u32 = caps[2].parse().ok()?;
    if let Some(year) = caps.get(3) {
        let year: i32 = year.as_str().parse().ok()?;
        return instant(
            tz,
            NaiveDate::from_ymd_opt(year, month, day)?.and_time(time),
        );
    }
    let (earliest, latest) = (now.saturating_sub(DAY), now.saturating_add(366 * DAY));
    let year = today.year();
    [year, year.checked_add(1)?, year.checked_sub(1)?]
        .into_iter()
        .filter_map(|y| instant(tz, NaiveDate::from_ymd_opt(y, month, day)?.and_time(time)))
        .find(|at| (earliest..latest).contains(at))
}

fn instant(tz: Tz, local: NaiveDateTime) -> Option<i64> {
    match tz.from_local_datetime(&local) {
        LocalResult::Single(t) => Some(t.timestamp()),
        LocalResult::Ambiguous(early, _) => Some(early.timestamp()),
        LocalResult::None => None,
    }
}

fn month_number(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let name = name.to_ascii_lowercase();
    MONTHS.iter().position(|m| *m == name).map(|i| i as u32 + 1)
}
