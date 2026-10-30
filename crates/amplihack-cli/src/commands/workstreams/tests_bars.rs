//! Issue #1710: the bar renderer, checked against the target rows in the
//! 2026-10-29T19:09:55Z comment on #1710 (the rows #1739 also names).

use super::bars::{
    BAR_CELLS, LoginRow, MAX_WIDTH, Reading, bar, format_pct, render_login_lines, without_zone,
};

fn login(name: &str, live: bool, reading: Reading) -> LoginRow {
    LoginRow {
        profile: name.to_string(),
        live,
        reading,
    }
}

fn bars(session: Option<f64>, week: f64, state: &str) -> Reading {
    Reading::Bars {
        session,
        week: Some(week),
        state: state.to_string(),
        session_resets: Some("Oct 29, 1:10pm".into()),
        week_resets: Some("Nov 2, 10am".into()),
    }
}

/// The target rows, copied from #1739. Two columns there disagree with the
/// bar rows by one space: the `WEEK` heading and the week bar after `-` sit
/// one column right of the week bars in every other row. Here they are
/// aligned with the bar rows, so `WEEK` is over its bars.
const TARGET: [&str; 7] = [
    "  LOGIN                      SESSION                   WEEK",
    "  hharkness (live)           ░░░░░░░░░░░░░░░░░░░░   0% █████░░░░░░░░░░░░░░░  29%",
    "  bbuck                      █████████████████░░░  86% ████████░░░░░░░░░░░░  41%",
    "  aanderson                  ████████████████████ 100% ████████░░░░░░░░░░░░  43%  session out, resets Oct 29, 1:10pm",
    "  rssourcelabs               -                         ████████████████████ 100%  week out, resets Nov 2, 10am",
    "  api-key                    API key, billed per token (no plan limits)",
    "  somebody                   no reading (timeout)",
];

fn target_rows() -> Vec<LoginRow> {
    vec![
        login("hharkness", true, bars(Some(0.0), 29.0, "ok")),
        login("bbuck", false, bars(Some(86.0), 41.0, "ok")),
        login(
            "aanderson",
            false,
            bars(Some(100.0), 43.0, "session_exhausted"),
        ),
        login(
            "rssourcelabs",
            false,
            bars(Some(100.0), 100.0, "weekly_exhausted"),
        ),
        login("api-key", false, Reading::ApiKey),
        login(
            "somebody",
            false,
            Reading::NoReading {
                state: "timeout".into(),
            },
        ),
    ]
}

#[test]
fn the_rows_match_the_target_in_the_issue() {
    assert_eq!(render_login_lines(&target_rows()), TARGET);
}

#[test]
fn the_bar_rows_in_the_issue_are_reproduced_byte_for_byte() {
    // These four lines are exactly as written in #1739 and #1710.
    let out = render_login_lines(&target_rows());
    for verbatim in &TARGET[1..4] {
        assert!(out.contains(&verbatim.to_string()), "{verbatim}");
    }
    assert_eq!(out[5], TARGET[5]);
    assert_eq!(out[6], TARGET[6]);
}

#[test]
fn a_bar_has_twenty_cells_one_per_whole_five_percent() {
    assert_eq!(BAR_CELLS, 20);
    assert_eq!(bar(0.0).as_deref(), Some("░░░░░░░░░░░░░░░░░░░░"));
    assert_eq!(bar(100.0).as_deref(), Some("████████████████████"));
    assert_eq!(bar(15.0).as_deref(), Some("███░░░░░░░░░░░░░░░░░"));
    assert_eq!(bar(29.0).as_deref(), Some("█████░░░░░░░░░░░░░░░"));
    assert_eq!(bar(43.0).as_deref(), Some("████████░░░░░░░░░░░░"));
    assert_eq!(bar(99.9).as_deref(), Some("███████████████████░"));
    for p in [0.0, 1.0, 4.9, 30.0, 58.0, 99.9, 100.0] {
        assert_eq!(bar(p).unwrap().chars().count(), BAR_CELLS, "{p}");
    }
}

#[test]
fn values_over_100_and_below_0_clamp() {
    assert_eq!(bar(250.0), bar(100.0));
    assert_eq!(bar(-5.0), bar(0.0));
}

#[test]
fn a_non_finite_percentage_has_no_bar() {
    assert_eq!(bar(f64::NAN), None);
    assert_eq!(bar(f64::INFINITY), None);
    let out = render_login_lines(&[login(
        "odd",
        false,
        Reading::Bars {
            session: Some(f64::NAN),
            week: None,
            state: "ok".into(),
            session_resets: None,
            week_resets: None,
        },
    )]);
    // Neither window has a number to draw: `-` in both cells.
    let cells: Vec<&str> = out[1].split_whitespace().collect();
    assert_eq!(cells, ["odd", "-", "-"], "{out:?}");
}

#[test]
fn over_100_shows_the_number_read_with_a_full_bar() {
    let out = render_login_lines(&[login("big", false, bars(Some(130.0), 5.0, "ok"))]);
    assert!(out[1].contains("████████████████████ 130%"), "{out:?}");
}

#[test]
fn an_unparsed_reading_shows_its_state_and_no_bars() {
    let out = render_login_lines(&[login(
        "p",
        false,
        Reading::NoReading {
            state: "unparsed".into(),
        },
    )]);
    assert!(out[1].ends_with("no reading (unparsed)"), "{out:?}");
    assert!(!out[1].contains('█') && !out[1].contains('░'));
}

#[test]
fn an_overflow_profile_shows_no_bars() {
    let out = render_login_lines(&[login("spare", false, Reading::Overflow)]);
    assert!(
        out[1].ends_with("overflow, paid credits (no plan limits)"),
        "{out:?}"
    );
}

#[test]
fn another_state_with_numbers_is_named_after_the_bars() {
    let out = render_login_lines(&[login("org", false, bars(Some(10.0), 20.0, "spend_limit"))]);
    assert!(out[1].ends_with("  spend limit"), "{out:?}");
}

#[test]
fn hostile_text_is_cleaned_and_lines_fit() {
    let out = render_login_lines(&[login(
        &format!("evil\x1b[31m{}", "x".repeat(80)),
        true,
        Reading::Bars {
            session: Some(10.0),
            week: Some(10.0),
            state: "session_exhausted".into(),
            session_resets: Some(format!("soon\u{202e}{}", "r".repeat(200))),
            week_resets: None,
        },
    )]);
    for line in &out {
        assert!(line.chars().count() <= MAX_WIDTH, "{line}");
        assert!(
            !line.contains('\x1b') && !line.contains('\u{202e}'),
            "{line}"
        );
    }
    assert!(out[1].contains("(live)"), "the live mark survives a cut");
}

#[test]
fn without_zone_drops_only_a_trailing_zone() {
    assert_eq!(
        without_zone("Nov 2, 10am (America/Los_Angeles)"),
        "Nov 2, 10am"
    );
    assert_eq!(without_zone("Nov 2, 10am"), "Nov 2, 10am");
    assert_eq!(without_zone(" 4:50pm (x) later "), "4:50pm (x) later");
}

#[test]
fn a_percentage_is_whole_when_whole_else_one_decimal() {
    assert_eq!(format_pct(29.0), "29%");
    assert_eq!(format_pct(12.5), "12.5%");
}
