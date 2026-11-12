//! The `profile usage` bar view (issue #1813), the default human output.
//!
//! One row per profile: the label, then the week and session windows as
//! ten-cell bars with their reset times, then the state and the remaining
//! marks. Rows with no reading get one spanning cell instead of bars.
//!
//! Rendering has no side effects. The only reads of the environment or the
//! clock are in [`TimeView::Local`]'s conversions and its zone name.
//! Untrusted text goes through `sanitize_display`; the only escape bytes
//! written are the fixed colour codes in [`bar_cell`].

use super::usage_cmd::{
    API_KEY_CELL, EXCLUDED_CELL, NO_SESSION_LIMIT_CELL, ORDER_LINE, Row, footers, marks_parts,
    no_session_limit, raw_lines, sanitize_display, state_label,
};
use super::usage_parse::{UsageState, UsageWindow};
use super::usage_source::Spend;
use chrono::{DateTime, FixedOffset, Local, TimeZone};
use std::ffi::OsStr;

/// Cells in a bar.
const BAR_CELLS: usize = 10;
/// Characters of a profile name shown before it is clipped.
const NAME_MAX: usize = 24;
/// Added to the label of the profile whose login is live in `~/.claude`.
const LIVE_SUFFIX: &str = " (live)";
/// A bar cell: ten bar characters, a space and a 4-character percentage.
const BAR_W: usize = 15;
/// The SESSION column, wide enough for `no session limit`.
const SESSION_W: usize = 16;
/// Spaces between columns.
const GAP: usize = 2;
/// Spaces before the STATE column: one, so that the header, which ends in
/// the 5-character `STATE`, fits in 100 columns under `--utc` with a
/// 24-character live name.
const STATE_GAP: usize = 1;

const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

/// The zone reset times are shown in.
#[derive(Debug, Clone)]
pub(super) enum TimeView {
    /// `--utc`: times end in `Z`.
    Utc,
    /// A fixed offset and the name shown for it, so tests do not depend
    /// on the host's zone; the command itself never builds one.
    #[cfg_attr(not(test), allow(dead_code))]
    Fixed(FixedOffset, String),
    /// The host's local zone, converted per timestamp so daylight saving
    /// applies to each time on its own.
    Local,
}

/// Whether the bar view is coloured: only on a terminal, and only when
/// `NO_COLOR` is unset or empty (no-color.org).
pub(super) fn colour_enabled(is_tty: bool, no_color: Option<&OsStr>) -> bool {
    is_tty && no_color.is_none_or(OsStr::is_empty)
}

/// A bar cell, always 15 characters of plain text: the floored tenth of the
/// percentage filled (clamped to 0-10 cells), then the floored percentage
/// right-aligned in 4 characters. With `colour`, the whole cell is one span
/// coloured by the raw percentage: green below 70, yellow below 90, red at
/// 90 and above.
pub(super) fn bar_cell(pct: f64, colour: bool) -> String {
    let filled = if pct.is_nan() {
        0
    } else {
        // Clamped to 0-10 before the cast, so it cannot truncate.
        (pct.clamp(0.0, 100.0) / 10.0).floor() as usize
    };
    let mut cell: String = std::iter::repeat_n('█', filled)
        .chain(std::iter::repeat_n('░', BAR_CELLS - filled))
        .collect();
    cell.push(' ');
    cell.push_str(&pct_text(pct));
    if !colour {
        return cell;
    }
    let code = if pct >= 90.0 {
        RED
    } else if pct >= 70.0 {
        YELLOW
    } else {
        GREEN
    };
    format!("{code}{cell}{RESET}")
}

/// The floored percentage in exactly 4 characters; `?%` for a value that
/// cannot be a percentage, `>999` above the widest one that fits.
fn pct_text(pct: f64) -> String {
    if pct.is_nan() || pct < 0.0 {
        return "  ?%".to_string();
    }
    if pct >= 1000.0 {
        return ">999".to_string();
    }
    // 0 <= pct < 1000, so the cast is exact after the floor.
    format!("{:>3}%", pct.floor() as i64)
}

/// A reset time as `MM-DD HH:MM` in the view's zone, with `Z` in UTC; `-`
/// when there is none or it cannot be shown.
pub(super) fn reset_cell(at: Option<i64>, time: &TimeView) -> String {
    let Some(t) = at else {
        return "-".to_string();
    };
    let shown = match time {
        TimeView::Utc => {
            DateTime::from_timestamp(t, 0).map(|d| d.format("%m-%d %H:%MZ").to_string())
        }
        TimeView::Fixed(offset, _) => offset
            .timestamp_opt(t, 0)
            .single()
            .map(|d| d.format("%m-%d %H:%M").to_string()),
        TimeView::Local => Local
            .timestamp_opt(t, 0)
            .single()
            .map(|d| d.format("%m-%d %H:%M").to_string()),
    };
    shown.unwrap_or_else(|| "-".to_string())
}

/// Width of a reset column: 12 in UTC (`MM-DD HH:MMZ`), 11 otherwise.
pub(super) fn reset_width(time: &TimeView) -> usize {
    match time {
        TimeView::Utc => 12,
        TimeView::Fixed(..) | TimeView::Local => 11,
    }
}

/// The zone named on the `times:` line.
pub(super) fn zone_label(time: &TimeView) -> String {
    match time {
        TimeView::Utc => "UTC".to_string(),
        TimeView::Fixed(_, name) => name.clone(),
        TimeView::Local => local_zone_name(),
    }
}

/// The local zone's name, from the host's `TZ`, system zone and offset.
fn local_zone_name() -> String {
    zone_name(
        std::env::var("TZ").ok().as_deref(),
        || iana_time_zone::get_timezone().ok(),
        || format!("UTC{}", Local::now().offset()),
    )
}

/// The zone named on the `times:` line: `TZ` when it names a known zone;
/// the system zone only when `TZ` is unset or empty; else `offset`, the
/// current offset as `UTC±HH:MM`. chrono's `Local` honours any non-empty
/// `TZ`, including POSIX rules such as `JST-9` and file paths, so such a
/// `TZ` must not be named as the system zone, which the times do not use.
/// A name is printed only when it parses as a known zone, so nothing else
/// from the environment reaches the output.
pub(super) fn zone_name(
    tz: Option<&str>,
    system: impl FnOnce() -> Option<String>,
    offset: impl FnOnce() -> String,
) -> String {
    let known = |name: &str| {
        name.parse::<chrono_tz::Tz>()
            .ok()
            .map(|tz| tz.name().to_string())
    };
    match tz.map(|v| v.trim_start_matches(':')) {
        Some(v) if !v.is_empty() => known(v).unwrap_or_else(offset),
        _ => system().and_then(|v| known(&v)).unwrap_or_else(offset),
    }
}

/// The label column: the sanitised name, clipped to 23 characters and `…`
/// when longer than 24, then ` (live)` for the profile live in `~/.claude`.
pub(super) fn label(row: &Row) -> String {
    let clean = sanitize_display(&row.profile, false);
    let mut out = if clean.chars().count() > NAME_MAX {
        let mut s: String = clean.chars().take(NAME_MAX - 1).collect();
        s.push('…');
        s
    } else {
        clean
    };
    if row.live_in {
        out.push_str(LIVE_SUFFIX);
    }
    out
}

/// One cell: the plain text that sets its width, and what is printed.
struct Cell {
    plain: String,
    shown: String,
}

impl Cell {
    fn text(s: impl Into<String>) -> Self {
        let plain = s.into();
        Self {
            shown: plain.clone(),
            plain,
        }
    }

    fn bar(pct: f64, colour: bool) -> Self {
        Self {
            plain: bar_cell(pct, false),
            shown: bar_cell(pct, colour),
        }
    }
}

/// The cells between the label and STATE: four windows' cells, or one
/// spanning cell for a row with no bars.
enum Middle {
    Bars([Cell; 4]),
    Span(String),
}

fn middle(row: &Row, time: &TimeView, colour: bool) -> Middle {
    if row.is_excluded() {
        return Middle::Span(EXCLUDED_CELL.to_string());
    }
    if row.api_key {
        return Middle::Span(API_KEY_CELL.to_string());
    }
    let r = &row.report;
    match r.state {
        UsageState::NotLoggedIn
        | UsageState::Timeout
        | UsageState::Unparsed
        | UsageState::Error => Middle::Span(format!("no reading ({})", r.state.as_str())),
        UsageState::Ok
        | UsageState::SessionExhausted
        | UsageState::WeeklyExhausted
        | UsageState::SpendLimit => {
            let bar = |w: &Option<UsageWindow>| {
                w.as_ref()
                    .map_or_else(|| Cell::text("-"), |w| Cell::bar(w.used_percentage, colour))
            };
            let reset = |w: &Option<UsageWindow>| {
                Cell::text(reset_cell(w.as_ref().and_then(|w| w.resets_at), time))
            };
            let session = if no_session_limit(row) {
                Cell::text(NO_SESSION_LIMIT_CELL)
            } else {
                bar(&r.session)
            };
            Middle::Bars([
                bar(&r.week_all),
                reset(&r.week_all),
                session,
                reset(&r.session),
            ])
        }
    }
}

fn pad(out: &mut String, cell: &Cell, width: usize, gap: usize) {
    out.push_str(&cell.shown);
    let used = cell.plain.chars().count();
    out.extend(std::iter::repeat_n(' ', width.saturating_sub(used) + gap));
}

/// The gap after the `i`th of the four middle cells.
fn middle_gap(i: usize) -> usize {
    if i == 3 { STATE_GAP } else { GAP }
}

/// The marks column of the bar view: the table's marks, then
/// `spend:<value>` (issue #1856) for every row whose spend is not `n/a`, so
/// the spend status the table shows in its `SPEND` column is not lost.
pub(super) fn bar_marks(row: &Row) -> Vec<String> {
    let mut parts = marks_parts(row, false);
    if row.spend != Spend::NotApplicable {
        parts.push(format!("spend:{}", row.spend.as_str()));
    }
    parts
}

/// The bar view: header, rows (each followed by its `--raw` text), the
/// footers the table also prints, then `times: <zone>` and the order line.
/// Its layout is not a stable interface; `--json` is.
pub(super) fn render_bars(rows: &[Row], time: &TimeView, colour: bool) -> String {
    let rw = reset_width(time);
    let labels: Vec<String> = rows.iter().map(label).collect();
    let lw = labels
        .iter()
        .map(|l| l.chars().count())
        .chain(["PROFILE".len()])
        .max()
        .unwrap_or(0);
    let sw = rows
        .iter()
        .map(|r| state_label(r).len())
        .chain(["STATE".len()])
        .max()
        .unwrap_or(0);
    let widths = [BAR_W, rw, SESSION_W, rw];
    let span_w = widths.iter().sum::<usize>() + GAP * (widths.len() - 1);

    let finish = |s: String| format!("{}\n", s.trim_end());
    let mut out = String::new();
    {
        let mut h = String::new();
        pad(&mut h, &Cell::text("PROFILE"), lw, GAP);
        for (i, (name, w)) in ["WEEK", "W-RESET", "SESSION", "S-RESET"]
            .iter()
            .zip(widths)
            .enumerate()
        {
            pad(&mut h, &Cell::text(*name), w, middle_gap(i));
        }
        h.push_str("STATE");
        out.push_str(&finish(h));
    }
    for (row, lab) in rows.iter().zip(labels) {
        let mut l = String::new();
        pad(&mut l, &Cell::text(lab), lw, GAP);
        match middle(row, time, colour) {
            Middle::Bars(cells) => {
                for (i, (c, w)) in cells.iter().zip(widths).enumerate() {
                    pad(&mut l, c, w, middle_gap(i));
                }
            }
            Middle::Span(text) => pad(&mut l, &Cell::text(text), span_w, STATE_GAP),
        }
        pad(&mut l, &Cell::text(state_label(row)), sw, GAP);
        l.push_str(&bar_marks(row).join(" "));
        out.push_str(&finish(l));
        raw_lines(&mut out, row);
    }
    out.push_str(&footers(rows));
    out.push_str(&format!("times: {}\n", zone_label(time)));
    out.push_str(ORDER_LINE);
    out.push('\n');
    out
}
