//! `profile usage` bar view (issue #1813): bar cells, reset times, colour,
//! the label column, the spanning cells of rows with no reading, the
//! footer lines, the 100-column budget and display sanitising.
//!
//! Every test uses fixed `Row` fixtures and `TimeView::Utc` or
//! `TimeView::Fixed`, so nothing depends on the host's time zone, terminal
//! or environment.

use super::usage_cmd::{Raw, Row, marks_parts, render_table};
use super::usage_parse::{UsageReport, UsageState, UsageWindow};
use super::usage_source::{Source, Spend};
use super::usage_view::{
    TimeView, bar_cell, bar_marks, colour_enabled, label, render_bars, reset_cell, reset_width,
    zone_label, zone_name,
};
use chrono::{FixedOffset, TimeZone, Utc};
use std::ffi::OsStr;

const ORDER: &str = "order: rotator pick (most room in the worse of session and week, then name)";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

/// 2026-11-05 23:30:00 UTC.
fn nov5_2330() -> i64 {
    Utc.with_ymd_and_hms(2026, 11, 5, 23, 30, 0)
        .single()
        .expect("valid time")
        .timestamp()
}

fn at(month: u32, day: u32, hour: u32, min: u32) -> i64 {
    Utc.with_ymd_and_hms(2026, month, day, hour, min, 0)
        .single()
        .expect("valid time")
        .timestamp()
}

fn plus2() -> TimeView {
    TimeView::Fixed(
        FixedOffset::east_opt(2 * 3600).expect("offset"),
        "UTC+02:00".to_string(),
    )
}

fn minus1() -> TimeView {
    TimeView::Fixed(
        FixedOffset::west_opt(3600).expect("offset"),
        "UTC-01:00".to_string(),
    )
}

fn win(label: &str, pct: f64, at: Option<i64>) -> UsageWindow {
    UsageWindow {
        label: label.to_string(),
        used_percentage: pct,
        resets_text: Some(format!("text-{label}-{pct}")),
        resets_at: at,
    }
}

fn report(
    state: UsageState,
    s: Option<(f64, Option<i64>)>,
    w: Option<(f64, Option<i64>)>,
) -> UsageReport {
    UsageReport {
        state,
        session: s.map(|(p, at)| win("session", p, at)),
        week_all: w.map(|(p, at)| win("all models", p, at)),
        week_models: Vec::new(),
        session_limit: true,
        observed_at: None,
    }
}

fn row(name: &str, rep: UsageReport) -> Row {
    Row {
        profile: name.to_string(),
        account: name.to_string(),
        default: false,
        leased_to: None,
        live_in: false,
        overflow: false,
        overflow_blocked_until: None,
        api_key: false,
        api_key_blocked: None,
        report: rep,
        error: None,
        raw: None,
        source: None,
        direct_failure: None,
        spend: Spend::NotApplicable,
        extra_shape: None,
    }
}

fn ok(name: &str, s: Option<(f64, Option<i64>)>, w: Option<(f64, Option<i64>)>) -> Row {
    row(name, report(UsageState::Ok, s, w))
}

fn weekly_only(name: &str, week: f64, at: Option<i64>) -> Row {
    let mut rep = report(UsageState::Ok, None, Some((week, at)));
    rep.session_limit = false;
    row(name, rep)
}

fn api_key_row(name: &str) -> Row {
    let mut r = row(name, UsageReport::empty(UsageState::Ok));
    r.api_key = true;
    r
}

/// The rows of the reference example, in the order the example shows.
fn example_rows() -> Vec<Row> {
    let (w, w2) = (Some(at(11, 5, 9, 0)), Some(at(11, 6, 2, 0)));
    let review = ok(
        "review",
        Some((5.0, Some(at(11, 1, 14, 0)))),
        Some((10.0, w)),
    );
    let mut work = ok(
        "work",
        Some((20.0, Some(at(11, 1, 12, 0)))),
        Some((30.0, w)),
    );
    work.live_in = true;
    work.default = true;
    work.account = "personal".into();
    let weekly = weekly_only("weekly", 47.0, w);
    let mut credit = ok(
        "credit",
        Some((40.0, Some(at(11, 1, 21, 0)))),
        Some((61.0, w2)),
    );
    credit.overflow = true;
    credit.source = Some(Source::Fallback);
    let team = row(
        "team",
        report(
            UsageState::SessionExhausted,
            Some((100.0, Some(at(11, 1, 23, 50)))),
            Some((55.0, w)),
        ),
    );
    let api = api_key_row("api");
    let mut personal = row("personal", report(UsageState::NotLoggedIn, None, None));
    personal.leased_to = Some("work".into());
    let mut spare = row("spare", report(UsageState::Timeout, None, None));
    spare.source = Some(Source::Fallback);
    vec![review, work, weekly, credit, team, api, personal, spare]
}

/// One line of the bar view, built from the documented widths: label
/// column, then WEEK (15), W-RESET, SESSION (16), S-RESET, STATE (17 here,
/// the longest state in the example), each followed by two spaces except
/// S-RESET, which is followed by one; trailing spaces are trimmed.
#[allow(clippy::too_many_arguments)]
fn line(
    lw: usize,
    rw: usize,
    name: &str,
    week: &str,
    wr: &str,
    sess: &str,
    sr: &str,
    state: &str,
    marks: &str,
) -> String {
    let s = format!(
        "{name:<lw$}  {week:<15}  {wr:<rw$}  {sess:<16}  {sr:<rw$} {state:<17}  {marks}",
        lw = lw,
        rw = rw
    );
    s.trim_end().to_string()
}

fn span_line(lw: usize, rw: usize, name: &str, msg: &str, state: &str, marks: &str) -> String {
    let span = 15 + 2 + rw + 2 + 16 + 2 + rw;
    let s = format!("{name:<lw$}  {msg:<span$} {state:<17}  {marks}");
    s.trim_end().to_string()
}

fn strip_sgr(s: &str) -> String {
    s.replace(GREEN, "")
        .replace(YELLOW, "")
        .replace(RED, "")
        .replace(RESET, "")
}

fn line_of<'a>(out: &'a str, name: &str) -> &'a str {
    out.lines()
        .find(|l| l.split_whitespace().next() == Some(name))
        .unwrap_or_else(|| panic!("no row for {name}: {out}"))
}

/// Column index (in characters) of `needle` in `line`.
fn col(line: &str, needle: &str) -> Option<usize> {
    line.find(needle).map(|b| line[..b].chars().count())
}

fn is_utc_reset(cell: &str) -> bool {
    regex::Regex::new(r"^\d{2}-\d{2} \d{2}:\d{2}Z$")
        .expect("regex")
        .is_match(cell)
}

// ---------------------------------------------------------------------------
// (d) Bar cells
// ---------------------------------------------------------------------------

#[test]
fn bar_cells_match_the_documented_table() {
    for (pct, want) in [
        (0.0, "░░░░░░░░░░   0%"),
        (13.0, "█░░░░░░░░░  13%"),
        (41.9, "████░░░░░░  41%"),
        (55.0, "█████░░░░░  55%"),
        (100.0, "██████████ 100%"),
        (130.0, "██████████ 130%"),
    ] {
        assert_eq!(bar_cell(pct, false), want, "{pct}%");
    }
}

#[test]
fn a_bar_cell_is_always_fifteen_characters() {
    for pct in [0.0, 0.4, 5.0, 9.9, 10.0, 41.2, 99.6, 100.0, 130.0, 999.0] {
        let cell = bar_cell(pct, false);
        assert_eq!(cell.chars().count(), 15, "{pct}%: {cell:?}");
        let bar: String = cell.chars().take(10).collect();
        assert!(
            bar.chars().all(|c| c == '█' || c == '░'),
            "{pct}%: {cell:?}"
        );
    }
}

#[test]
fn percentages_are_floored_so_100_appears_only_at_the_limit() {
    assert!(bar_cell(99.6, false).ends_with(" 99%"));
    assert!(bar_cell(41.2, false).ends_with(" 41%"));
    assert!(bar_cell(100.0, false).ends_with("100%"));
    assert!(bar_cell(12.5, false).ends_with(" 12%"));
}

#[test]
fn filled_cells_are_the_floored_tenth_clamped_to_ten() {
    let filled = |pct: f64| bar_cell(pct, false).chars().filter(|&c| c == '█').count();
    assert_eq!(filled(0.0), 0);
    assert_eq!(filled(9.9), 0);
    assert_eq!(filled(10.0), 1);
    assert_eq!(filled(55.0), 5);
    assert_eq!(filled(99.9), 9);
    assert_eq!(filled(100.0), 10);
    assert_eq!(filled(130.0), 10);
}

#[test]
fn odd_numbers_never_panic_and_keep_the_width() {
    for pct in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -5.0,
        -0.0,
        1e300,
    ] {
        let cell = bar_cell(pct, false);
        let bar: String = cell.chars().take(10).collect();
        assert!(bar.chars().all(|c| c == '█' || c == '░'), "{pct}: {cell:?}");
        for colour in [false, true] {
            let c = bar_cell(pct, colour);
            assert!(!c.contains('\n'), "{pct}: {c:?}");
        }
    }
    let negative = bar_cell(-5.0, false);
    assert!(negative.starts_with("░░░░░░░░░░"), "{negative:?}");
}

// ---------------------------------------------------------------------------
// (c) Colour
// ---------------------------------------------------------------------------

#[test]
fn colour_bands_use_the_raw_percentage() {
    for (pct, code) in [
        (0.0, GREEN),
        (69.9, GREEN),
        (70.0, YELLOW),
        (89.9, YELLOW),
        (90.0, RED),
        (100.0, RED),
        (130.0, RED),
    ] {
        let cell = bar_cell(pct, true);
        assert!(cell.starts_with(code), "{pct}%: {cell:?}");
        assert!(cell.ends_with(RESET), "{pct}%: {cell:?}");
        assert_eq!(cell.matches('\x1b').count(), 2, "one span: {cell:?}");
        assert_eq!(strip_sgr(&cell), bar_cell(pct, false), "{pct}%");
    }
}

#[test]
fn colour_off_emits_no_escape_sequences() {
    let out = render_bars(&example_rows(), &TimeView::Utc, false);
    assert!(!out.contains("\x1b["), "{out:?}");
    assert!(!out.contains('\x1b'), "{out:?}");
}

#[test]
fn colour_on_changes_nothing_but_the_bar_spans() {
    let rows = example_rows();
    let plain = render_bars(&rows, &TimeView::Utc, false);
    let coloured = render_bars(&rows, &TimeView::Utc, true);
    assert!(
        coloured.contains(GREEN) && coloured.contains(RED),
        "{coloured:?}"
    );
    assert_eq!(
        strip_sgr(&coloured),
        plain,
        "padding is computed on plain text"
    );
    let header = coloured.lines().next().expect("header");
    assert!(
        !header.contains('\x1b'),
        "header never coloured: {header:?}"
    );
    for footer in coloured
        .lines()
        .filter(|l| l.starts_with("times:") || l.starts_with("order:"))
    {
        assert!(
            !footer.contains('\x1b'),
            "footer never coloured: {footer:?}"
        );
    }
    let api = line_of(&coloured, "api");
    assert!(!api.contains('\x1b'), "no bars, no colour: {api:?}");
}

#[test]
fn colour_follows_the_terminal_and_no_color() {
    assert!(colour_enabled(true, None));
    assert!(
        colour_enabled(true, Some(OsStr::new(""))),
        "empty NO_COLOR is unset"
    );
    assert!(!colour_enabled(true, Some(OsStr::new("1"))));
    assert!(
        !colour_enabled(true, Some(OsStr::new("0"))),
        "any non-empty value"
    );
    assert!(
        !colour_enabled(false, None),
        "a pipe or file is never coloured"
    );
    assert!(!colour_enabled(false, Some(OsStr::new(""))));
    assert!(!colour_enabled(false, Some(OsStr::new("1"))));
}

// ---------------------------------------------------------------------------
// (b) Reset times and the zone line
// ---------------------------------------------------------------------------

#[test]
fn reset_cells_cross_the_day_boundary_in_each_zone() {
    let t = Some(nov5_2330());
    assert_eq!(reset_cell(t, &TimeView::Utc), "11-05 23:30Z");
    assert_eq!(reset_cell(t, &plus2()), "11-06 01:30");
    assert_eq!(reset_cell(t, &minus1()), "11-05 22:30");
}

#[test]
fn reset_widths_are_eleven_local_and_twelve_utc() {
    assert_eq!(reset_width(&TimeView::Utc), 12);
    assert_eq!(reset_width(&plus2()), 11);
    assert_eq!(reset_width(&TimeView::Local), 11);
    assert_eq!(reset_cell(Some(nov5_2330()), &plus2()).chars().count(), 11);
}

#[test]
fn a_missing_or_unrepresentable_reset_is_a_dash() {
    for view in [TimeView::Utc, plus2()] {
        assert_eq!(reset_cell(None, &view), "-");
        assert_eq!(reset_cell(Some(i64::MAX), &view), "-");
        assert_eq!(reset_cell(Some(i64::MIN), &view), "-");
    }
}

#[test]
fn zone_labels_name_the_view() {
    assert_eq!(zone_label(&TimeView::Utc), "UTC");
    assert_eq!(zone_label(&plus2()), "UTC+02:00");
    assert_eq!(zone_label(&minus1()), "UTC-01:00");
}

/// The `times:` zone: a known `TZ`, else the system zone only when `TZ` is
/// unset or empty, else the offset. A POSIX or path `TZ` drives chrono's
/// `Local`, so naming the system zone for it would contradict the times.
#[test]
fn the_zone_name_follows_tz_then_the_system_then_the_offset() {
    let sys = || Some("Europe/London".to_string());
    let off = || "UTC+09:00".to_string();
    assert_eq!(zone_name(Some("Asia/Tokyo"), sys, off), "Asia/Tokyo");
    assert_eq!(zone_name(Some(":Asia/Tokyo"), sys, off), "Asia/Tokyo");
    assert_eq!(zone_name(None, sys, off), "Europe/London");
    assert_eq!(zone_name(Some(""), sys, off), "Europe/London");
    assert_eq!(zone_name(Some(":"), sys, off), "Europe/London");
    assert_eq!(zone_name(Some("JST-9"), sys, off), "UTC+09:00");
    assert_eq!(zone_name(Some("/etc/localtime"), sys, off), "UTC+09:00");
    assert_eq!(zone_name(Some("\x1b[31mevil"), sys, off), "UTC+09:00");
    assert_eq!(zone_name(None, || Some("bogus".into()), off), "UTC+09:00");
    assert_eq!(zone_name(None, || None, off), "UTC+09:00");
}

#[test]
fn every_utc_reset_cell_is_a_z_time_or_a_dash() {
    let out = render_bars(&example_rows(), &TimeView::Utc, false);
    let header = out.lines().next().expect("header");
    let (wr, sr) = (
        col(header, "W-RESET").expect("W-RESET"),
        col(header, "S-RESET").expect("S-RESET"),
    );
    for name in ["review", "work", "weekly", "credit", "team"] {
        let l: Vec<char> = line_of(&out, name).chars().collect();
        for start in [wr, sr] {
            let cell: String = l[start..start + 12].iter().collect();
            let cell = cell.trim_end();
            assert!(
                cell == "-" || is_utc_reset(cell),
                "{name}: reset cell {cell:?} in {:?}",
                String::from_iter(&l)
            );
        }
    }
}

#[test]
fn the_footer_names_the_zone_then_the_order() {
    for (view, zone) in [(TimeView::Utc, "UTC"), (plus2(), "UTC+02:00")] {
        let out = render_bars(&example_rows(), &view, false);
        let tail: Vec<&str> = out.lines().rev().take(2).collect();
        assert_eq!(tail, [ORDER, &format!("times: {zone}")], "{out}");
        assert!(out.ends_with('\n'), "{out:?}");
        assert_eq!(out.matches("times:").count(), 1, "{out}");
    }
}

// ---------------------------------------------------------------------------
// Layout: the documented example, column by column
// ---------------------------------------------------------------------------

#[test]
fn utc_view_matches_the_documented_example() {
    let out = render_bars(&example_rows(), &TimeView::Utc, false);
    let (lw, rw) = (11, 12);
    let want = [
        format!(
            "{:<lw$}  {:<15}  {:<rw$}  {:<16}  {:<rw$} STATE",
            "PROFILE", "WEEK", "W-RESET", "SESSION", "S-RESET"
        ),
        line(
            lw,
            rw,
            "review",
            "█░░░░░░░░░  10%",
            "11-05 09:00Z",
            "░░░░░░░░░░   5%",
            "11-01 14:00Z",
            "ok",
            "",
        ),
        line(
            lw,
            rw,
            "work (live)",
            "███░░░░░░░  30%",
            "11-05 09:00Z",
            "██░░░░░░░░  20%",
            "11-01 12:00Z",
            "ok",
            "* hosts:personal",
        ),
        line(
            lw,
            rw,
            "weekly",
            "████░░░░░░  47%",
            "11-05 09:00Z",
            "no session limit",
            "-",
            "ok",
            "",
        ),
        line(
            lw,
            rw,
            "credit",
            "██████░░░░  61%",
            "11-06 02:00Z",
            "████░░░░░░  40%",
            "11-01 21:00Z",
            "ok",
            "overflow (fallback)",
        ),
        line(
            lw,
            rw,
            "team",
            "█████░░░░░  55%",
            "11-05 09:00Z",
            "██████████ 100%",
            "11-01 23:50Z",
            "session_exhausted",
            "",
        ),
        span_line(lw, rw, "api", "api-key (billed per token)", "api_key", ""),
        span_line(
            lw,
            rw,
            "personal",
            "no reading (not_logged_in)",
            "not_logged_in",
            "leased→work",
        ),
        span_line(
            lw,
            rw,
            "spare",
            "no reading (timeout)",
            "timeout",
            "(fallback)",
        ),
        "times: UTC".to_string(),
        ORDER.to_string(),
    ];
    let got: Vec<&str> = out.lines().collect();
    assert_eq!(got, want, "\n{out}");
}

#[test]
fn local_view_uses_eleven_character_reset_cells() {
    let view = TimeView::Fixed(
        FixedOffset::east_opt(0).expect("offset"),
        "Europe/London".into(),
    );
    let out = render_bars(&example_rows(), &view, false);
    let (lw, rw) = (11, 11);
    assert_eq!(
        line_of(&out, "review"),
        line(
            lw,
            rw,
            "review",
            "█░░░░░░░░░  10%",
            "11-05 09:00",
            "░░░░░░░░░░   5%",
            "11-01 14:00",
            "ok",
            ""
        )
    );
    assert_eq!(
        line_of(&out, "weekly"),
        line(
            lw,
            rw,
            "weekly",
            "████░░░░░░  47%",
            "11-05 09:00",
            "no session limit",
            "-",
            "ok",
            ""
        )
    );
    assert_eq!(
        line_of(&out, "api"),
        span_line(lw, rw, "api", "api-key (billed per token)", "api_key", "")
    );
    assert!(out.lines().any(|l| l == "times: Europe/London"), "{out}");
    assert!(!out.contains('Z'), "no UTC suffix in local mode: {out}");
}

#[test]
fn the_state_column_lines_up_on_every_row() {
    for view in [TimeView::Utc, plus2()] {
        let out = render_bars(&example_rows(), &view, false);
        let header = out.lines().next().expect("header");
        let state_at = col(header, "STATE").expect("STATE");
        for (name, state) in [
            ("review", "ok"),
            ("weekly", "ok"),
            ("team", "session_exhausted"),
            ("api", "api_key"),
            ("personal", "not_logged_in"),
            ("spare", "timeout"),
        ] {
            let l: String = line_of(&out, name).chars().skip(state_at).collect();
            assert!(l.starts_with(state), "{name}: {l:?}\n{out}");
        }
    }
}

#[test]
fn a_missing_window_in_a_measured_row_is_a_dash_in_bar_and_reset() {
    let r = ok("solo", Some((20.0, Some(nov5_2330()))), None);
    let out = render_bars(&[r], &TimeView::Utc, false);
    let lw = 7; // "PROFILE"
    assert_eq!(
        line_of(&out, "solo"),
        line(
            lw,
            12,
            "solo",
            "-",
            "-",
            "██░░░░░░░░  20%",
            "11-05 23:30Z",
            "ok",
            ""
        )
    );
}

#[test]
fn a_spend_limit_row_shows_bars_not_a_message() {
    let r = row(
        "spent",
        report(
            UsageState::SpendLimit,
            Some((10.0, Some(nov5_2330()))),
            Some((20.0, Some(nov5_2330()))),
        ),
    );
    let out = render_bars(&[r], &TimeView::Utc, false);
    let l = line_of(&out, "spent");
    assert!(l.contains("██░░░░░░░░  20%"), "{l}");
    assert!(l.contains("spend_limit"), "{l}");
    assert!(!l.contains("no reading"), "{l}");
}

#[test]
fn every_unavailable_state_says_no_reading_with_its_state() {
    for (state, text) in [
        (UsageState::NotLoggedIn, "not_logged_in"),
        (UsageState::Timeout, "timeout"),
        (UsageState::Unparsed, "unparsed"),
        (UsageState::Error, "error"),
    ] {
        let out = render_bars(
            &[row("x", report(state, None, None))],
            &TimeView::Utc,
            false,
        );
        let l = line_of(&out, "x");
        assert!(l.contains(&format!("no reading ({text})")), "{l}");
        assert!(!l.contains('%') && !l.contains('█'), "{l}");
    }
}

#[test]
fn a_weekly_exhausted_row_shows_a_full_week_and_no_session_limit() {
    let mut rep = report(
        UsageState::WeeklyExhausted,
        None,
        Some((100.0, Some(nov5_2330()))),
    );
    rep.session_limit = false;
    let out = render_bars(&[row("wk", rep)], &TimeView::Utc, true);
    let l = line_of(&out, "wk");
    assert!(l.contains(&format!("{RED}██████████ 100%{RESET}")), "{l:?}");
    assert!(l.contains("no session limit"), "{l}");
    assert!(l.contains("weekly_exhausted"), "{l}");
}

// ---------------------------------------------------------------------------
// Labels, marks and the 100-column budget
// ---------------------------------------------------------------------------

#[test]
fn the_live_label_follows_the_live_in_mark() {
    let mut r = ok("work", None, Some((1.0, None)));
    assert_eq!(label(&r), "work");
    r.live_in = true;
    assert_eq!(label(&r), "work (live)");
    let out = render_bars(&[r], &TimeView::Utc, false);
    assert!(
        !out.contains("live in ~/.claude"),
        "the mark moves to the label: {out}"
    );
}

#[test]
fn long_names_are_clipped_to_23_characters_and_an_ellipsis() {
    let name24 = "abcdefghijklmnopqrstuvwx";
    assert_eq!(label(&ok(name24, None, None)), name24, "24 fits");
    let name25 = "abcdefghijklmnopqrstuvwxy";
    assert_eq!(label(&ok(name25, None, None)), "abcdefghijklmnopqrstuvw…");
    let mut live = ok(name25, None, None);
    live.live_in = true;
    assert_eq!(label(&live), "abcdefghijklmnopqrstuvw… (live)");
    let multi = "é".repeat(25);
    assert_eq!(
        label(&ok(&multi, None, None)),
        format!("{}…", "é".repeat(23))
    );
    let wide = "日本語".repeat(9);
    assert_eq!(label(&ok(&wide, None, None)).chars().count(), 24);
}

#[test]
fn a_24_character_live_name_fits_in_100_columns_in_both_time_modes() {
    let name = "abcdefghijklmnopqrstuvwx";
    let t = Some(nov5_2330());
    let mut r = ok(name, Some((99.6, t)), Some((99.6, t)));
    r.live_in = true;
    for view in [TimeView::Utc, plus2()] {
        for colour in [false, true] {
            let out = render_bars(std::slice::from_ref(&r), &view, colour);
            let header = out.lines().next().expect("header");
            assert!(header.starts_with("PROFILE"), "{header}");
            assert!(header.ends_with("STATE"), "{header}");
            assert!(
                header.chars().count() <= 100,
                "header is {} columns: {header:?}",
                header.chars().count()
            );
            let l = strip_sgr(line_of(&out, name));
            assert!(l.starts_with("abcdefghijklmnopqrstuvwx (live)"), "{l}");
            assert!(
                l.chars().count() <= 100,
                "{} columns: {l:?}",
                l.chars().count()
            );
            assert!(l.ends_with("ok"), "no stray mark or dash: {l:?}");
        }
    }
}

#[test]
fn marks_without_live_are_empty_for_a_plain_row() {
    let r = ok("plain", None, None);
    assert!(marks_parts(&r, false).is_empty());
    assert!(marks_parts(&r, true).is_empty());
    let mut live = r.clone();
    live.live_in = true;
    assert!(
        marks_parts(&live, false).is_empty(),
        "{:?}",
        marks_parts(&live, false)
    );
    assert_eq!(marks_parts(&live, true), ["live in ~/.claude"]);
}

#[test]
fn marks_keep_their_order_and_text() {
    let mut r = ok("w", None, None);
    r.default = true;
    r.leased_to = Some("d".into());
    r.account = "acct".into();
    r.live_in = true;
    r.overflow = true;
    r.source = Some(Source::Fallback);
    assert_eq!(
        marks_parts(&r, true),
        [
            "*",
            "leased→d",
            "hosts:acct",
            "live in ~/.claude",
            "overflow",
            "(fallback)"
        ]
    );
    assert_eq!(
        marks_parts(&r, false),
        ["*", "leased→d", "hosts:acct", "overflow", "(fallback)"]
    );
}

// ---------------------------------------------------------------------------
// Raw blocks and the existing footers
// ---------------------------------------------------------------------------

#[test]
fn raw_blocks_and_old_footers_come_before_times_and_order() {
    let mut a = ok("a", Some((10.0, None)), Some((10.0, None)));
    a.raw = Some(Raw {
        stdout: "hello".into(),
        stderr: String::new(),
    });
    a.source = Some(Source::Fallback);
    let mut over = row(
        "over",
        report(
            UsageState::SessionExhausted,
            Some((100.0, None)),
            Some((5.0, None)),
        ),
    );
    over.live_in = true;
    over.overflow = true;
    let out = render_bars(&[a, over], &TimeView::Utc, false);
    let lines: Vec<&str> = out.lines().collect();
    let pos = |p: &str| {
        lines
            .iter()
            .position(|l| l.starts_with(p))
            .unwrap_or_else(|| panic!("no {p:?}: {out}"))
    };
    assert!(pos("a ") < pos("  raw stdout:"), "{out}");
    assert!(
        pos("    hello") < pos("over"),
        "raw follows its own row: {out}"
    );
    assert!(pos("over") < pos("running on overflow: over"), "{out}");
    assert!(pos("running on overflow:") < pos("times:"), "{out}");
    assert!(pos("times:") < pos("order:"), "{out}");
}

#[test]
fn the_api_key_live_and_blocked_footers_are_kept() {
    let mut api = api_key_row("console");
    api.live_in = true;
    api.api_key_blocked = Some(("spend limit".into(), nov5_2330()));
    let out = render_bars(&[api], &TimeView::Utc, false);
    assert!(
        out.contains("live: work is running on the API key (billed per token)\n"),
        "{out}"
    );
    assert!(
        out.contains("blocked: the API key until 2026-11-05T23:30:00Z (spend limit)\n"),
        "{out}"
    );
    let l = line_of(&out, "console");
    assert!(l.starts_with("console (live)"), "{l}");
    assert!(
        l.contains("blocked until 2026-11-05T23:30:00Z (spend limit)"),
        "{l}"
    );
}

// ---------------------------------------------------------------------------
// Security: untrusted text cannot drive the terminal
// ---------------------------------------------------------------------------

#[test]
fn hostile_names_and_accounts_cannot_inject_escapes() {
    let evil = "ev\u{1b}]0;x\u{7}il\u{202e}\u{9b}31m";
    let mut r = ok(evil, Some((95.0, Some(nov5_2330()))), Some((10.0, None)));
    r.account = format!("acct{evil}");
    r.leased_to = Some(format!("dir{evil}"));
    r.raw = Some(Raw {
        stdout: format!("out{evil}\nline2"),
        stderr: format!("err{evil}"),
    });
    // The block reason comes from the rotate ledger on disk.
    r.api_key_blocked = Some((format!("why{evil}"), nov5_2330()));
    for colour in [false, true] {
        let out = render_bars(std::slice::from_ref(&r), &TimeView::Utc, colour);
        let without = strip_sgr(&out);
        let footer = without
            .lines()
            .find(|l| l.starts_with("blocked: the API key until"))
            .unwrap_or_else(|| panic!("no blocked footer: {out:?}"));
        assert!(footer.contains("why"), "{footer:?}");
        assert!(!footer.chars().any(char::is_control), "{footer:?}");
        assert!(
            !without.contains('\x1b'),
            "escape outside the fixed codes: {out:?}"
        );
        assert!(!out.contains('\u{202e}'), "{out:?}");
        assert!(!out.contains('\u{9b}'), "{out:?}");
        assert!(!out.contains('\u{7}'), "{out:?}");
        assert!(
            !without.chars().any(|c| c.is_control() && c != '\n'),
            "{out:?}"
        );
    }
}

#[test]
fn a_hostile_name_is_cleaned_before_it_is_clipped() {
    // 30 characters, with controls near the cut: the label is still 24
    // characters and contains no control character.
    let name = format!("{}\u{1b}\u{9b}{}", "a".repeat(22), "b".repeat(6));
    let l = label(&ok(&name, None, None));
    assert_eq!(l.chars().count(), 24, "{l:?}");
    assert!(!l.chars().any(char::is_control), "{l:?}");
    assert!(l.ends_with('…'), "{l:?}");
}

// ---------------------------------------------------------------------------
// (e) --table keeps the table's columns, `SPEND` (#1856) last, and ends
// with the order line
// ---------------------------------------------------------------------------

#[test]
fn the_table_keeps_its_header_and_ends_with_the_order_line() {
    let out = render_table(&example_rows());
    let header: Vec<&str> = out
        .lines()
        .next()
        .expect("header")
        .split("  ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(
        header,
        [
            "PROFILE",
            "MARKS",
            "STATE",
            "SESSION",
            "SESSION RESETS",
            "WEEK",
            "WEEK RESETS",
            "SPEND"
        ]
    );
    assert_eq!(out.lines().last(), Some(ORDER), "{out}");
    assert!(!out.contains("times:"), "{out}");
    assert!(!out.contains('█'), "{out}");
    assert!(
        line_of(&out, "work").contains("* hosts:personal live in ~/.claude"),
        "the table keeps the live mark: {out}"
    );
}

/// Issue #1856 in the bar view: every spend other than `n/a` follows the
/// table's marks as `spend:<value>`; `n/a` adds nothing, so a credits-off
/// row is as wide as before.
#[test]
fn bar_marks_add_the_spend_unless_it_is_n_a() {
    let mut r = ok("mistt", Some((35.0, None)), Some((52.0, None)));
    assert!(bar_marks(&r).is_empty());
    r.default = true;
    for (spend, mark) in [
        (Spend::Ok, "spend:ok"),
        (Spend::LimitReached, "spend:limit_reached"),
        (Spend::Unknown, "spend:unknown"),
        (Spend::NotProbed, "spend:not_probed"),
    ] {
        r.spend = spend;
        assert_eq!(bar_marks(&r), ["*", mark]);
        let out = render_bars(std::slice::from_ref(&r), &TimeView::Utc, false);
        assert!(
            line_of(&out, "mistt").ends_with(&format!("* {mark}")),
            "{out}"
        );
    }
    r.spend = Spend::NotApplicable;
    assert_eq!(bar_marks(&r), ["*"]);
}

// ---------------------------------------------------------------------------
// Issue #2271: a login with no token in the bar view
//
// Contract:
// - A row whose error is `ErrorReason::NotSignedIn(why)` shows
//   `not signed in (<why>)` in place of the bars, not
//   `no reading (not_logged_in)`.
// - `bar_marks` leaves the text out, so the line holds it once; the marks
//   column keeps the row's other marks and `spend:unknown`, with no
//   `(fallback)`.
// - A `not_logged_in` row from `/usage` text keeps `no reading (...)`.
// ---------------------------------------------------------------------------

use super::usage_cmd::{ErrorReason, Probe};

fn not_signed_in_row(name: &str, why: &'static str) -> Row {
    let p = Probe::not_signed_in(why);
    let mut r = row(name, p.report);
    r.error = p.error;
    r.raw = p.raw;
    r.source = p.source;
    r.direct_failure = p.direct_failure;
    r.spend = p.spend;
    r.extra_shape = p.extra_shape;
    r
}

#[test]
fn a_not_signed_in_row_shows_the_reason_in_place_of_the_bars() {
    let rows = vec![
        ok("work", Some((30.0, None)), Some((20.0, None))),
        not_signed_in_row("scratch", "no credentials file"),
    ];
    let out = render_bars(&rows, &TimeView::Utc, false);
    let l = line_of(&out, "scratch");
    assert!(l.contains("not signed in (no credentials file)"), "{l}");
    assert!(!l.contains("no reading"), "{l}");
    assert!(!l.contains('%') && !l.contains('█'), "{l}");
    assert!(l.contains("not_logged_in"), "the state column: {l}");
    assert_eq!(l.matches("not signed in").count(), 1, "once: {l}");
    assert!(l.ends_with("spend:unknown"), "{l}");
    assert!(!l.contains("fallback"), "{l}");
}

#[test]
fn bar_marks_leave_out_the_not_signed_in_text() {
    let mut r = not_signed_in_row("scratch", "no oauth token");
    assert_eq!(bar_marks(&r), ["spend:unknown"]);
    r.default = true;
    assert_eq!(bar_marks(&r), ["*", "spend:unknown"]);
}

#[test]
fn every_final_reason_has_its_own_span() {
    for why in ["no oauth token", "no credentials file", "no refresh token"] {
        let out = render_bars(&[not_signed_in_row("x", why)], &TimeView::Utc, false);
        let l = line_of(&out, "x");
        assert!(l.contains(&format!("not signed in ({why})")), "{l}");
    }
}

#[test]
fn a_not_logged_in_row_from_usage_text_still_says_no_reading() {
    let mut r = row("x", report(UsageState::NotLoggedIn, None, None));
    r.source = Some(Source::Fallback);
    let out = render_bars(&[r], &TimeView::Utc, false);
    let l = line_of(&out, "x");
    assert!(l.contains("no reading (not_logged_in)"), "{l}");
    assert!(!l.contains("not signed in"), "{l}");
}

#[test]
fn other_error_rows_keep_no_reading() {
    let mut r = row("x", report(UsageState::Error, None, None));
    r.error = Some(ErrorReason::Direct("http 5xx"));
    let out = render_bars(&[r], &TimeView::Utc, false);
    assert!(line_of(&out, "x").contains("no reading (error)"), "{out}");
}
