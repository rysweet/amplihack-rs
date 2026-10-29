//! `profile usage` bar view (issue #1739): the default human output. Pure
//! tests: fixed `Row` values in, a `String` out; no filesystem, no `claude`.
//!
//! Layout under test (see docs/reference/profile-command.md, "Bar output"):
//! a two-space indent, the label padded to `clamp(widest, 5, 24)`, two
//! spaces, the session cell (20-cell bar, one space, percentage right-aligned
//! to `max(4, widest)`), one space, the week cell, then two spaces and a note.
//! Trailing spaces are trimmed and no line passes 100 characters.

use super::usage_cmd::{ErrorReason, Raw, Row, render_bars};
use super::usage_parse::{UsageReport, UsageState, UsageWindow};

const FULL: char = '█';
const EMPTY: char = '░';
const HEADER: &str = "  LOGIN  SESSION                   WEEK";

fn win(pct: f64, resets: Option<&str>) -> UsageWindow {
    UsageWindow {
        label: "w".to_string(),
        used_percentage: pct,
        resets_text: resets.map(str::to_string),
        resets_at: None,
    }
}

fn row(name: &str, state: UsageState, s: Option<UsageWindow>, w: Option<UsageWindow>) -> Row {
    Row {
        profile: name.to_string(),
        account: name.to_string(),
        default: false,
        leased_to: None,
        live_in: false,
        overflow: false,
        overflow_blocked_until: None,
        live_in_home: false,
        api_key: false,
        report: UsageReport {
            state,
            session: s,
            week_all: w,
            week_models: Vec::new(),
        },
        error: None,
        raw: None,
    }
}

fn ok(name: &str, s: f64, w: f64) -> Row {
    row(
        name,
        UsageState::Ok,
        Some(win(s, Some("Oct 29, 1:10pm (America/Los_Angeles)"))),
        Some(win(w, Some("Nov 2, 10am (America/Los_Angeles)"))),
    )
}

fn bar(filled: usize) -> String {
    let mut s: String = std::iter::repeat_n(FULL, filled).collect();
    s.extend(std::iter::repeat_n(EMPTY, 20 - filled));
    s
}

fn lines(out: &str) -> Vec<&str> {
    out.lines().collect()
}

/// The single data line of a one-row render.
fn only_row(out: &str) -> &str {
    let l = lines(out);
    assert_eq!(l.len(), 2, "header and one row: {out}");
    l[1]
}

fn filled(line: &str) -> Vec<usize> {
    // The run lengths of each bar on the line, in order.
    let mut bars = Vec::new();
    let mut in_bar = false;
    let mut n = 0;
    let mut cells = 0;
    for c in line.chars() {
        if c == FULL || c == EMPTY {
            if !in_bar {
                in_bar = true;
                n = 0;
                cells = 0;
            }
            cells += 1;
            if c == FULL {
                n += 1;
            }
        } else if in_bar {
            assert_eq!(cells, 20, "a bar has 20 cells: {line}");
            bars.push(n);
            in_bar = false;
        }
    }
    if in_bar {
        assert_eq!(cells, 20, "a bar has 20 cells: {line}");
        bars.push(n);
    }
    bars
}

fn assert_width(out: &str) {
    for l in out.lines() {
        assert!(l.chars().count() <= 100, "{} chars: {l}", l.chars().count());
        assert_eq!(l, l.trim_end(), "trailing space: {l:?}");
    }
}

// ---------------------------------------------------------------------------
// Bars and percentages
// ---------------------------------------------------------------------------

#[test]
fn zero_and_partial_render_exactly() {
    let out = render_bars(&[ok("x", 0.0, 86.0)]);
    assert_eq!(
        out,
        format!("{HEADER}\n  x      {}   0% {}  86%\n", bar(0), bar(17))
    );
}

#[test]
fn full_and_over_full_fill_all_cells_and_show_the_real_value() {
    let out = render_bars(&[ok("x", 100.0, 150.0)]);
    assert_eq!(
        only_row(&out),
        format!("  x      {} 100% {} 150%", bar(20), bar(20))
    );
}

#[test]
fn cells_are_the_percentage_over_five_rounded_down() {
    for (pct, want) in [
        (0.0, 0),
        (4.9, 0),
        (5.0, 1),
        (9.99, 1),
        (50.0, 10),
        (99.9, 19),
        (100.0, 20),
        (1000.0, 20),
    ] {
        let out = render_bars(&[ok("x", pct, 0.0)]);
        assert_eq!(filled(only_row(&out)), [want, 0], "{pct}: {out}");
    }
}

#[test]
fn nan_negative_and_infinite_values_do_not_panic() {
    let rows = [
        ok("nan", f64::NAN, -5.0),
        ok("neg", -0.0, f64::NEG_INFINITY),
        ok("inf", f64::INFINITY, 0.0),
    ];
    let out = render_bars(&rows);
    assert_eq!(lines(&out).len(), 4, "{out}");
    assert_eq!(filled(lines(&out)[1]), [0, 0], "NaN and negative: {out}");
    assert_eq!(filled(lines(&out)[2])[1], 0, "-inf: {out}");
    assert_width(&out);
}

#[test]
fn a_fractional_percentage_widens_the_column_for_every_row() {
    let out = render_bars(&[ok("a", 12.5, 3.0), ok("b", 100.0, 7.0)]);
    let l = lines(&out);
    assert_eq!(
        l[0], "  LOGIN  SESSION                    WEEK",
        "header follows the wider column: {out}"
    );
    assert_eq!(l[1], format!("  a      {} 12.5% {}    3%", bar(2), bar(0)));
    assert_eq!(l[2], format!("  b      {}  100% {}    7%", bar(20), bar(1)));
}

#[test]
fn an_unreported_window_on_an_ok_row_shows_a_dash() {
    let r = row("x", UsageState::Ok, None, Some(win(30.0, None)));
    let out = render_bars(&[r]);
    assert_eq!(
        only_row(&out),
        format!("  x      -                         {}  30%", bar(6))
    );
    let r = row("x", UsageState::Ok, Some(win(30.0, None)), None);
    assert_eq!(
        only_row(&render_bars(&[r])),
        format!("  x      {}  30% -", bar(6))
    );
}

// ---------------------------------------------------------------------------
// Exhausted rows and reset notes
// ---------------------------------------------------------------------------

#[test]
fn session_exhausted_shows_both_bars_and_when_it_resets() {
    let r = row(
        "aanderson",
        UsageState::SessionExhausted,
        Some(win(100.0, Some("Oct 29, 1:10pm (America/Los_Angeles)"))),
        Some(win(68.0, Some("Nov 2, 10am (America/Los_Angeles)"))),
    );
    let out = render_bars(&[r]);
    let line = only_row(&out);
    assert_eq!(
        line,
        format!(
            "  aanderson  {} 100% {}  68%  session out, resets Oct 29, 1:10pm",
            bar(20),
            bar(13)
        )
    );
    assert!(!out.contains("America"), "zone removed: {out}");
    assert_width(&out);
}

#[test]
fn weekly_exhausted_shows_a_dash_for_the_session_and_the_week_reset() {
    let r = row(
        "night",
        UsageState::WeeklyExhausted,
        Some(win(40.0, Some("Oct 29, 1:10pm (America/Los_Angeles)"))),
        Some(win(100.0, Some("Nov 2, 10am (America/Los_Angeles)"))),
    );
    let out = render_bars(&[r]);
    assert_eq!(
        only_row(&out),
        format!(
            "  night  -                         {} 100%  week out, resets Nov 2, 10am",
            bar(20)
        )
    );
    assert_eq!(filled(only_row(&out)), [20], "one bar only: {out}");
}

#[test]
fn exhausted_without_a_reset_time_has_no_resets_part() {
    for resets in [None, Some(""), Some("   ")] {
        let r = row(
            "x",
            UsageState::SessionExhausted,
            Some(win(100.0, resets)),
            Some(win(10.0, None)),
        );
        let out = render_bars(&[r]);
        assert!(
            only_row(&out).ends_with("  session out"),
            "{resets:?}: {out}"
        );
        let r = row(
            "x",
            UsageState::WeeklyExhausted,
            None,
            Some(win(100.0, resets)),
        );
        let out = render_bars(&[r]);
        assert!(only_row(&out).ends_with("  week out"), "{resets:?}: {out}");
    }
}

#[test]
fn only_a_trailing_bracketed_part_is_removed() {
    for (text, want) in [
        ("Oct 29, 1:10pm (UTC)", "Oct 29, 1:10pm"),
        ("Oct (x) 29, 1pm (UTC)", "Oct (x) 29, 1pm"),
        ("Oct 29 (Tue), 1pm", "Oct 29 (Tue), 1pm"),
        ("1pm(UTC)", "1pm(UTC)"),
        ("Oct 29, 1pm", "Oct 29, 1pm"),
    ] {
        let r = row(
            "x",
            UsageState::SessionExhausted,
            Some(win(100.0, Some(text))),
            Some(win(0.0, None)),
        );
        let out = render_bars(&[r]);
        assert!(
            only_row(&out).ends_with(&format!("  session out, resets {want}")),
            "{text:?}: {out}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rows without bars
// ---------------------------------------------------------------------------

#[test]
fn an_api_key_row_has_no_bars_whatever_its_state() {
    for state in [UsageState::Ok, UsageState::Timeout, UsageState::Error] {
        let mut r = ok("billing", 50.0, 50.0);
        r.report.state = state;
        r.api_key = true;
        let out = render_bars(&[r]);
        assert_eq!(
            only_row(&out),
            "  billing  API key, billed per token (no plan limits)",
            "{state:?}"
        );
        assert!(!out.contains(FULL) && !out.contains(EMPTY), "{out}");
    }
}

#[test]
fn unusable_states_say_no_reading_with_the_state() {
    for state in [
        UsageState::SpendLimit,
        UsageState::NotLoggedIn,
        UsageState::Timeout,
        UsageState::Unparsed,
        UsageState::Error,
    ] {
        let mut r = row("x", state, None, None);
        if state == UsageState::Error {
            r.error = Some(ErrorReason::StartFailed);
        }
        let out = render_bars(&[r]);
        assert_eq!(
            only_row(&out),
            format!("  x      no reading ({})", state.as_str())
        );
        assert!(
            !out.contains("could not start"),
            "error detail stays on stderr: {out}"
        );
    }
}

// ---------------------------------------------------------------------------
// Labels, header, order
// ---------------------------------------------------------------------------

#[test]
fn live_rows_are_marked_and_no_other_mark_is_shown() {
    let mut live = ok("work", 10.0, 10.0);
    live.live_in = true;
    live.default = true;
    let mut home = ok("~/.claude", 20.0, 20.0);
    home.live_in = true;
    home.live_in_home = true;
    let mut other = ok("spare", 30.0, 30.0);
    other.default = true;
    other.overflow = true;
    other.leased_to = Some("work".into());
    other.account = "someone".into();
    let out = render_bars(&[live, home, other]);
    let l = lines(&out);
    assert_eq!(&l[0][..18], "  LOGIN           ", "{out}");
    assert!(l[1].starts_with("  work (live)       "), "{out}");
    assert!(l[2].starts_with("  ~/.claude (live)  "), "{out}");
    assert!(l[3].starts_with("  spare             "), "{out}");
    for no in ["*", "leased", "hosts:", "overflow", "someone"] {
        assert!(!out.contains(no), "{no} is a table mark: {out}");
    }
}

#[test]
fn rows_keep_the_given_order() {
    let out = render_bars(&[ok("zed", 90.0, 90.0), ok("abe", 1.0, 1.0)]);
    let l = lines(&out);
    assert!(l[1].starts_with("  zed "), "{out}");
    assert!(l[2].starts_with("  abe "), "{out}");
}

#[test]
fn empty_rows_give_the_header_alone() {
    assert_eq!(render_bars(&[]), format!("{HEADER}\n"));
}

// ---------------------------------------------------------------------------
// Width
// ---------------------------------------------------------------------------

#[test]
fn a_long_name_is_cut_to_24_with_an_ellipsis() {
    let name = "n".repeat(64);
    let out = render_bars(&[ok(&name, 10.0, 10.0)]);
    let want: String = format!("{}…", "n".repeat(23));
    assert!(
        only_row(&out).starts_with(&format!("  {want}  {}", bar(2))),
        "{out}"
    );
    assert_width(&out);
}

#[test]
fn a_long_live_name_keeps_the_live_suffix() {
    let mut r = ok(&"n".repeat(64), 10.0, 10.0);
    r.live_in = true;
    let out = render_bars(&[r]);
    let want = format!("{}… (live)", "n".repeat(16));
    assert_eq!(want.chars().count(), 24);
    assert!(
        only_row(&out).starts_with(&format!("  {want}  {}", bar(2))),
        "{out}"
    );
}

#[test]
fn a_name_of_exactly_24_is_not_cut() {
    let name = "n".repeat(24);
    let out = render_bars(&[ok(&name, 10.0, 10.0)]);
    assert!(only_row(&out).starts_with(&format!("  {name}  ")), "{out}");
    assert!(!out.contains('…'), "{out}");
}

#[test]
fn a_non_ascii_name_is_cut_by_characters_not_bytes() {
    let name = "é".repeat(40);
    let out = render_bars(&[ok(&name, 10.0, 10.0)]);
    let want = format!("{}…", "é".repeat(23));
    assert!(only_row(&out).starts_with(&format!("  {want}  ")), "{out}");
    assert_width(&out);
}

#[test]
fn no_line_passes_100_characters() {
    let long_reset = format!("{} (America/Los_Angeles)", "x".repeat(200));
    let mut a = row(
        &"n".repeat(64),
        UsageState::SessionExhausted,
        Some(win(100.0, Some(&long_reset))),
        Some(win(12.5, Some(&long_reset))),
    );
    a.live_in = true;
    let b = row(
        &"m".repeat(64),
        UsageState::WeeklyExhausted,
        None,
        Some(win(100.0, Some(&long_reset))),
    );
    let out = render_bars(&[a, b]);
    assert_width(&out);
    let l = lines(&out);
    assert!(l[1].contains("(live)"), "{out}");
    // About 17 columns remain for the note after a 24-char label and two
    // 26-char cells; the note is cut, never the bars.
    assert!(l[1].contains("  session out"), "{out}");
    assert!(l[1].ends_with('…'), "a clipped note ends in …: {out}");
    assert_eq!(l[1].chars().count(), 100, "{out}");
    assert_eq!(filled(l[1]), [20, 2], "{out}");
    assert!(l[2].contains("  week out") && l[2].ends_with('…'), "{out}");
    assert_eq!(l[2].chars().count(), 100, "{out}");
    assert!(!out.contains("America"), "{out}");
}

// ---------------------------------------------------------------------------
// Sanitising
// ---------------------------------------------------------------------------

const HOSTILE: &str = "a\x1b[31mb\u{9b}c\u{202e}d\u{2066}e\rf\ng";

fn assert_clean(out: &str) {
    for bad in [
        '\x1b', '\u{9b}', '\r', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}',
        '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    ] {
        assert!(!out.contains(bad), "{bad:?} in {out:?}");
    }
}

#[test]
fn hostile_names_and_raw_output_are_cleaned() {
    let mut r = row(
        HOSTILE,
        UsageState::SessionExhausted,
        Some(win(100.0, None)),
        Some(win(10.0, None)),
    );
    r.live_in = true;
    r.overflow = true;
    r.raw = Some(Raw {
        stdout: format!("out {HOSTILE}"),
        stderr: format!("err {HOSTILE}"),
    });
    let out = render_bars(&[r]);
    assert_clean(&out);
    // 17 cleaned characters plus " (live)": exactly 24, so not cut.
    assert!(out.contains("  a?[31mb?c?d?e?f?g (live)  "), "{out}");
    assert!(
        out.ends_with("running on overflow: a?[31mb?c?d?e?f?g\n"),
        "{out}"
    );
}

#[test]
fn hostile_reset_text_is_cleaned_before_the_zone_is_removed() {
    let r = row(
        "z",
        UsageState::SessionExhausted,
        Some(win(
            100.0,
            Some("O\x1bc\u{9b}t\u{202e}2\u{2066}9\r! (America/Los_Angeles)"),
        )),
        Some(win(10.0, None)),
    );
    let out = render_bars(&[r]);
    assert_clean(&out);
    assert!(
        only_row(&out).ends_with("  session out, resets O?c?t?2?9?!"),
        "{out}"
    );
}

#[test]
fn a_newline_in_a_name_or_reset_cannot_add_a_line() {
    let rows = [
        row("x\ny", UsageState::Ok, Some(win(1.0, None)), None),
        row(
            "z",
            UsageState::SessionExhausted,
            Some(win(100.0, Some("Oct\n29\r\n(UTC)"))),
            None,
        ),
        row("w\r\nv", UsageState::Timeout, None, None),
    ];
    let out = render_bars(&rows);
    assert_eq!(lines(&out).len(), 1 + rows.len(), "{out}");
    assert!(out.contains("  x?y "), "{out}");
}

// ---------------------------------------------------------------------------
// --raw and the overflow footer
// ---------------------------------------------------------------------------

#[test]
fn raw_blocks_follow_their_row() {
    let mut a = ok("a", 1.0, 1.0);
    a.raw = Some(Raw {
        stdout: "hello\nworld".into(),
        stderr: String::new(),
    });
    let b = ok("b", 2.0, 2.0);
    let out = render_bars(&[a, b]);
    let l = lines(&out);
    assert!(l[1].starts_with("  a "), "{out}");
    assert_eq!(
        &l[2..7],
        [
            "  raw stdout:",
            "    hello",
            "    world",
            "  raw stderr:",
            "    (empty)"
        ],
        "{out}"
    );
    assert!(l[7].starts_with("  b "), "{out}");
    assert_eq!(l.len(), 8, "{out}");
}

#[test]
fn overflow_footer_needs_live_overflow_and_a_reached_limit() {
    let mut on = ok("extra", 100.0, 40.0);
    on.live_in = true;
    on.overflow = true;
    let out = render_bars(&[ok("a", 1.0, 1.0), on.clone()]);
    assert_eq!(
        lines(&out).last().copied(),
        Some("running on overflow: extra"),
        "{out}"
    );

    let mut exhausted = row(
        "extra",
        UsageState::WeeklyExhausted,
        None,
        Some(win(100.0, None)),
    );
    exhausted.live_in = true;
    exhausted.overflow = true;
    assert!(render_bars(&[exhausted]).ends_with("running on overflow: extra\n"));

    let mut capacity = on.clone();
    capacity.report.session = Some(win(50.0, None));
    assert!(!render_bars(&[capacity]).contains("running on overflow"));

    let mut not_live = on.clone();
    not_live.live_in = false;
    assert!(!render_bars(&[not_live]).contains("running on overflow"));

    let mut not_overflow = on;
    not_overflow.overflow = false;
    assert!(!render_bars(&[not_overflow]).contains("running on overflow"));

    let mut failed = row("extra", UsageState::Error, None, None);
    failed.live_in = true;
    failed.overflow = true;
    assert!(!render_bars(&[failed]).contains("running on overflow"));
}
