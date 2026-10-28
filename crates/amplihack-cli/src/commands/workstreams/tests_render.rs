//! Issue #1710 (R5, D12): rendering is a pure function over a `Snapshot`.
//!
//! Contract (`render`):
//! - `bar(percent: f64) -> Option<String>`: 10 cells of `█` and `░`, the
//!   percentage clamped to 0..=100; `None` for NaN or infinity.
//! - `sanitize(&str) -> String`: every char outside printable ASCII
//!   (`' '..='~'`) becomes `?`.
//! - `clip(&str, max_chars) -> String`: unchanged when it fits, else the
//!   first `max_chars - 1` chars and `…`.
//! - `elapsed(Option<i64>) -> String`: `-` for `None`, `2h05m` under a day,
//!   `3d04h` from a day up; negative values count as 0.
//! - `render(&Snapshot) -> String`: the `WORKSTREAMS` section, a blank line,
//!   then the `LOGINS` section when `snapshot.logins` is `Some`. No line over
//!   100 chars, no ESC, and no non-ASCII char outside `█ ░ ▶ ⏸ …`.

use super::render::{
    LoginRow, LoginsSection, PrCell, Reading, Snapshot, WorkRow, bar, clip, elapsed, render,
    sanitize,
};

const ALLOWED_GLYPHS: [char; 5] = ['█', '░', '▶', '⏸', '…'];

fn work(label: &str) -> WorkRow {
    WorkRow {
        running: true,
        label: label.to_string(),
        issue: Some(1710),
        pr: PrCell::Known(1715),
        percent: 70,
        elapsed_secs: Some(7_500),
        stage: "merge-ready round 1".to_string(),
        concerns: None,
        conflicts: false,
        reason: None,
    }
}

fn login(name: &str, live: bool, reading: Reading) -> LoginRow {
    LoginRow {
        profile: name.to_string(),
        live,
        reading,
    }
}

fn bars(session: f64, week: f64) -> Reading {
    Reading::Bars {
        session: Some(session),
        week: Some(week),
        state: "ok".to_string(),
        resets: Some("Oct 28, 4:50pm".to_string()),
    }
}

fn full_snapshot() -> Snapshot {
    let mut review = work("rotator manual login");
    review.issue = Some(1693);
    review.pr = PrCell::Known(1702);
    review.percent = 45;
    review.stage = "review round 2".into();
    review.concerns = Some(3);

    let mut limited = work("limit-resume wake");
    limited.running = false;
    limited.pr = PrCell::None;
    limited.percent = 15;
    limited.stage = "building".into();
    limited.reason = Some("stopped on a usage limit".into());

    let mut queued = work("docs sweep");
    queued.running = false;
    queued.pr = PrCell::None;
    queued.percent = 0;
    queued.elapsed_secs = None;
    queued.stage = "queued".into();

    Snapshot {
        no_identity: false,
        workstreams: vec![work("workstream-status skill"), review, limited, queued],
        logins: Some(LoginsSection::Rows(vec![
            login("work", true, bars(31.0, 58.0)),
            login(
                "spare",
                false,
                Reading::NoReading {
                    state: "timeout".into(),
                },
            ),
            login("platform", false, Reading::ApiKey),
        ])),
    }
}

fn assert_display_safe(out: &str) {
    assert!(!out.contains('\x1b'), "ESC in output:\n{out}");
    for line in out.lines() {
        let width = line.chars().count();
        assert!(width <= 100, "{width} columns: {line:?}");
        for c in line.chars() {
            assert!(
                (' '..='~').contains(&c) || ALLOWED_GLYPHS.contains(&c),
                "unexpected char {c:?} in {line:?}"
            );
        }
    }
}

fn line_with<'a>(out: &'a str, needle: &str) -> &'a str {
    out.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in:\n{out}"))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[test]
fn bars_have_ten_cells_and_clamp() {
    assert_eq!(bar(0.0).as_deref(), Some("░░░░░░░░░░"));
    assert_eq!(bar(100.0).as_deref(), Some("██████████"));
    assert_eq!(bar(70.0).as_deref(), Some("███████░░░"));
    assert_eq!(bar(31.0).as_deref(), Some("███░░░░░░░"));
    assert_eq!(bar(92.0).as_deref(), Some("█████████░"));
    assert_eq!(bar(250.0).as_deref(), Some("██████████"));
    assert_eq!(bar(-5.0).as_deref(), Some("░░░░░░░░░░"));
    for p in [0.0, 1.0, 15.0, 45.0, 58.0, 99.9, 100.0] {
        assert_eq!(bar(p).unwrap().chars().count(), 10, "{p}");
    }
}

#[test]
fn a_non_finite_percentage_has_no_bar() {
    assert_eq!(bar(f64::NAN), None);
    assert_eq!(bar(f64::INFINITY), None);
    assert_eq!(bar(f64::NEG_INFINITY), None);
}

#[test]
fn sanitize_replaces_everything_outside_printable_ascii() {
    assert_eq!(sanitize("plain text #1"), "plain text #1");
    assert_eq!(sanitize("a\x1b[31mb"), "a?[31mb");
    assert_eq!(sanitize("tab\tnew\nline"), "tab?new?line");
    assert_eq!(sanitize("caf\u{e9}"), "caf?");
    assert_eq!(sanitize("\u{202e}rtl"), "?rtl");
    assert_eq!(sanitize("wide\u{4e2d}"), "wide?");
}

#[test]
fn clip_marks_a_cut_with_an_ellipsis() {
    assert_eq!(clip("short", 24), "short");
    assert_eq!(clip("exactly-ten", 11), "exactly-ten");
    let cut = clip("abcdefghijklmnopqrstuvwxyz0123", 24);
    assert_eq!(cut.chars().count(), 24);
    assert!(cut.ends_with('…'));
    assert!(cut.starts_with("abcdefghijklmnopqrstuvw"));
}

#[test]
fn elapsed_is_hours_and_minutes_under_a_day_then_days_and_hours() {
    assert_eq!(elapsed(None), "-");
    assert_eq!(elapsed(Some(0)), "0h00m");
    assert_eq!(elapsed(Some(7_500)), "2h05m");
    assert_eq!(elapsed(Some(86_399)), "23h59m");
    assert_eq!(elapsed(Some(273_600)), "3d04h");
    assert_eq!(elapsed(Some(-50)), "0h00m");
    assert!(elapsed(Some(i64::MAX)).chars().count() <= 12);
}

// ---------------------------------------------------------------------------
// Whole output
// ---------------------------------------------------------------------------

#[test]
fn the_full_table_is_plain_narrow_and_has_both_sections() {
    let out = render(&full_snapshot());
    assert_display_safe(&out);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "WORKSTREAMS");
    assert!(lines.contains(&"LOGINS"), "{out}");
    let ws = lines.iter().position(|l| *l == "WORKSTREAMS").unwrap();
    let lg = lines.iter().position(|l| *l == "LOGINS").unwrap();
    assert!(ws < lg);
}

#[test]
fn a_workstream_row_has_marker_numbers_bar_percent_elapsed_and_stage() {
    let out = render(&full_snapshot());
    let row = line_with(&out, "workstream-status skill");
    assert!(row.starts_with('▶'), "{row}");
    for part in [
        "#1710",
        "#1715",
        "███████░░░",
        "70%",
        "2h05m",
        "merge-ready round 1",
    ] {
        assert!(row.contains(part), "{part:?} missing from {row:?}");
    }
}

#[test]
fn concerns_conflicts_and_reason_follow_the_stage() {
    let mut row = work("x");
    row.running = false;
    row.stage = "review round 2".into();
    row.concerns = Some(3);
    row.conflicts = true;
    row.reason = Some("stopped on a usage limit".into());
    let out = render(&Snapshot {
        no_identity: false,
        workstreams: vec![row],
        logins: None,
    });
    let line = line_with(&out, "review round 2");
    assert!(line.starts_with('⏸'), "{line}");
    assert!(
        line.contains("review round 2, 3 concerns, conflicts, stopped on a usage limit"),
        "{line}"
    );
}

#[test]
fn missing_and_unknown_numbers_render_as_dash_and_question_mark() {
    let mut none = work("no-pr");
    none.issue = None;
    none.pr = PrCell::None;
    let mut unknown = work("gh-down");
    unknown.pr = PrCell::Unknown;
    let out = render(&Snapshot {
        no_identity: false,
        workstreams: vec![none, unknown],
        logins: None,
    });
    let a = line_with(&out, "no-pr");
    assert!(!a.contains('#'), "{a}");
    assert!(a.contains(" - "), "{a}");
    assert!(line_with(&out, "gh-down").contains("#?"));
}

#[test]
fn long_labels_are_cut_to_24_characters() {
    let label = "a-very-long-workstream-label-that-goes-on";
    let out = render(&Snapshot {
        no_identity: false,
        workstreams: vec![work(label)],
        logins: None,
    });
    assert!(!out.contains(label));
    let cut: String = label.chars().take(23).collect();
    assert!(out.contains(&format!("{cut}…")), "{out}");
}

#[test]
fn a_long_stage_is_cut_to_fit_100_columns() {
    let mut row = work("x");
    row.stage = "s".repeat(300);
    row.reason = Some("r".repeat(200));
    let out = render(&Snapshot {
        no_identity: false,
        workstreams: vec![row],
        logins: None,
    });
    assert_display_safe(&out);
    assert!(line_with(&out, "sss").ends_with('…'));
}

#[test]
fn hostile_text_is_sanitised_before_it_is_cut_or_padded() {
    let mut row = work("evil\x1b]0;title\x07label");
    row.stage = "stage\x1b[2J\u{202e}".into();
    row.reason = Some("why\r\nnext".into());
    let out = render(&Snapshot {
        no_identity: false,
        workstreams: vec![row],
        logins: Some(LoginsSection::Rows(vec![login(
            "prof\x1b[31m",
            false,
            bars(10.0, 10.0),
        )])),
    });
    assert_display_safe(&out);
    assert!(out.contains("evil?]0;title?label"), "{out}");
    assert_eq!(out.lines().filter(|l| l.contains("why")).count(), 1);
}

#[test]
fn empty_sections_print_none() {
    let out = render(&Snapshot {
        no_identity: false,
        workstreams: vec![],
        logins: Some(LoginsSection::Rows(vec![])),
    });
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    assert_eq!(lines.iter().filter(|l| **l == "none").count(), 2, "{out}");
}

#[test]
fn without_an_identity_the_hint_line_is_printed() {
    let out = render(&Snapshot {
        no_identity: true,
        workstreams: vec![],
        logins: None,
    });
    assert!(
        out.contains("no session id found; run with --all to see every run on this host"),
        "{out}"
    );
}

#[test]
fn no_logins_section_when_logins_were_not_asked_for() {
    let out = render(&Snapshot {
        no_identity: false,
        workstreams: vec![work("x")],
        logins: None,
    });
    assert!(!out.contains("LOGINS"), "{out}");
}

// ---------------------------------------------------------------------------
// Logins
// ---------------------------------------------------------------------------

#[test]
fn a_login_row_has_two_bars_percentages_state_and_reset() {
    let out = render(&full_snapshot());
    let row = line_with(&out, "work*");
    for part in [
        "███░░░░░░░",
        "31%",
        "██████░░░░",
        "58%",
        "ok",
        "Oct 28, 4:50pm",
    ] {
        assert!(row.contains(part), "{part:?} missing from {row:?}");
    }
}

#[test]
fn the_live_login_is_starred_and_explained() {
    let out = render(&full_snapshot());
    assert!(out.contains("work*"));
    assert!(
        out.lines().any(|l| l.trim() == "* live in ~/.claude"),
        "{out}"
    );

    let none_live = render(&Snapshot {
        no_identity: false,
        workstreams: vec![],
        logins: Some(LoginsSection::Rows(vec![login("a", false, bars(1.0, 1.0))])),
    });
    assert!(!none_live.contains("live in ~/.claude"), "{none_live}");
}

#[test]
fn a_profile_without_a_reading_shows_no_bar() {
    let out = render(&full_snapshot());
    let row = line_with(&out, "spare");
    assert!(row.contains("no reading"), "{row}");
    assert!(row.contains("timeout"), "{row}");
    assert!(!row.contains('█') && !row.contains('░'), "{row}");
}

#[test]
fn an_api_key_profile_says_it_is_billed_per_token() {
    let out = render(&full_snapshot());
    let row = line_with(&out, "platform");
    assert!(
        row.contains("API key: billed per token, no plan limits"),
        "{row}"
    );
    assert!(!row.contains('█') && !row.contains('░'), "{row}");
}

#[test]
fn a_reading_with_a_non_finite_percentage_shows_no_bar_for_it() {
    let out = render(&Snapshot {
        no_identity: false,
        workstreams: vec![],
        logins: Some(LoginsSection::Rows(vec![login(
            "odd",
            false,
            Reading::Bars {
                session: Some(f64::NAN),
                week: None,
                state: "ok".into(),
                resets: None,
            },
        )])),
    });
    assert_display_safe(&out);
    assert!(!out.contains("NaN"), "{out}");
    assert!(!out.contains("inf"), "{out}");
}

#[test]
fn a_refused_logins_section_prints_the_reason_in_place_of_rows() {
    let out = render(&Snapshot {
        no_identity: false,
        workstreams: vec![],
        logins: Some(LoginsSection::Refused(
            "profiles are not applied in Docker\x1b[0m".into(),
        )),
    });
    assert_display_safe(&out);
    assert!(out.contains("profiles are not applied in Docker"), "{out}");
}
