//! Issue #1710 (R5, D12): rendering is a pure function over a `Snapshot`,
//! checked against the target output the owner gave in the comment of
//! 2026-10-29T19:09:55Z on #1710.
//!
//! Contract (`render`):
//! - `sanitize(&str) -> String`: every char outside printable ASCII
//!   (`' '..='~'`) becomes `?`.
//! - `clip(&str, max_chars) -> String`: unchanged when it fits, else the
//!   first `max_chars - 1` chars and `…`.
//! - `render(&Snapshot) -> String`: the dated title, a blank line, the
//!   column headings and one row per workstream, then a blank line and the
//!   logins section from the shared renderer (#1739) when `snapshot.logins`
//!   is `Some`. No line over 120 chars, no ESC, and no non-ASCII char
//!   outside `█ ░ ▶ ⏸ … —`.

use super::bars::Reading;
use super::render::{LoginRow, LoginsSection, PrCell, Snapshot, WorkRow, clip, render, sanitize};

const ALLOWED_GLYPHS: [char; 6] = ['█', '░', '▶', '⏸', '…', '—'];

/// The target output from the comment on #1710. Every data row is exactly
/// as written there. Two headings are moved to sit over their columns: in
/// the comment `STAGE` is two columns right of the stage text and `WEEK`
/// one column right of the week bars, and the week bar after a `-` session
/// is one column right of the week bars above it.
const TARGET: &str = "  amplihack-rs workstreams — Thu 11:56 PDT

  WORKSTREAM                     ISSUE  PR     PROGRESS                   STAGE
  Credit-backed overflow         #1694  #1695  ████████████████████ 100%  merged
  API-key profiles (last resort) #1700  #1727  ██████░░░░░░░░░░░░░░  30%  ▶ PR open, conflicts with main
  Usage probe timeouts           #1698  -      ███░░░░░░░░░░░░░░░░░  15%  ⏸ build stopped on a usage limit
  Rotator skips needed swaps     #1721  -      ░░░░░░░░░░░░░░░░░░░░   0%  ⏸ queued

  LOGIN                      SESSION                   WEEK
  hharkness (live)           ░░░░░░░░░░░░░░░░░░░░   0% █████░░░░░░░░░░░░░░░  29%
  aanderson                  ████████████████████ 100% ████████░░░░░░░░░░░░  43%  session out, resets Oct 29, 1:10pm
  rssourcelabs               -                         ████████████████████ 100%  week out, resets Nov 2, 10am
  api-key                    API key, billed per token (no plan limits)
";

/// The data rows of the target exactly as the comment on #1710 has them.
const VERBATIM: [&str; 8] = [
    "  amplihack-rs workstreams — Thu 11:56 PDT",
    "  Credit-backed overflow         #1694  #1695  ████████████████████ 100%  merged",
    "  API-key profiles (last resort) #1700  #1727  ██████░░░░░░░░░░░░░░  30%  ▶ PR open, conflicts with main",
    "  Usage probe timeouts           #1698  -      ███░░░░░░░░░░░░░░░░░  15%  ⏸ build stopped on a usage limit",
    "  Rotator skips needed swaps     #1721  -      ░░░░░░░░░░░░░░░░░░░░   0%  ⏸ queued",
    "  hharkness (live)           ░░░░░░░░░░░░░░░░░░░░   0% █████░░░░░░░░░░░░░░░  29%",
    "  aanderson                  ████████████████████ 100% ████████░░░░░░░░░░░░  43%  session out, resets Oct 29, 1:10pm",
    "  api-key                    API key, billed per token (no plan limits)",
];

fn work(label: &str, issue: u64, pr: PrCell, percent: u8, stage: &str) -> WorkRow {
    WorkRow {
        running: false,
        label: label.to_string(),
        issue: Some(issue),
        pr,
        percent,
        stage: stage.to_string(),
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

fn bars(session: f64, week: f64, state: &str) -> Reading {
    Reading::Bars {
        session: Some(session),
        week: Some(week),
        state: state.to_string(),
        session_resets: Some("Oct 29, 1:10pm".into()),
        week_resets: Some("Nov 2, 10am".into()),
    }
}

/// The state behind the target output.
fn target_snapshot() -> Snapshot {
    let mut open = work(
        "API-key profiles (last resort)",
        1700,
        PrCell::Known(1727),
        30,
        "PR open",
    );
    open.running = true;
    open.conflicts = true;
    let mut limited = work("Usage probe timeouts", 1698, PrCell::None, 15, "building");
    limited.reason = Some("stopped on a usage limit".into());
    Snapshot {
        repo: Some("amplihack-rs".into()),
        when: "Thu 11:56 PDT".into(),
        no_identity: false,
        workstreams: vec![
            work(
                "Credit-backed overflow",
                1694,
                PrCell::Known(1695),
                100,
                "merged",
            ),
            open,
            limited,
            work(
                "Rotator skips needed swaps",
                1721,
                PrCell::None,
                0,
                "queued",
            ),
        ],
        logins: Some(LoginsSection::Rows(vec![
            login("hharkness", true, bars(0.0, 29.0, "ok")),
            login("aanderson", false, bars(100.0, 43.0, "session_exhausted")),
            login(
                "rssourcelabs",
                false,
                bars(100.0, 100.0, "weekly_exhausted"),
            ),
            login("api-key", false, Reading::ApiKey),
        ])),
    }
}

fn small(workstreams: Vec<WorkRow>, logins: Option<LoginsSection>) -> Snapshot {
    Snapshot {
        repo: None,
        when: "Thu 11:56 PDT".into(),
        no_identity: false,
        workstreams,
        logins,
    }
}

fn assert_display_safe(out: &str) {
    assert!(!out.contains('\x1b'), "ESC in output:\n{out}");
    for line in out.lines() {
        let width = line.chars().count();
        assert!(width <= 120, "{width} columns: {line:?}");
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
// The target in the issue
// ---------------------------------------------------------------------------

#[test]
fn the_output_matches_the_target_in_the_issue() {
    assert_eq!(render(&target_snapshot()), TARGET);
}

#[test]
fn every_data_row_of_the_target_is_reproduced_byte_for_byte() {
    let out = render(&target_snapshot());
    let lines: Vec<&str> = out.lines().collect();
    for row in VERBATIM {
        assert!(lines.contains(&row), "missing {row:?} in:\n{out}");
    }
    assert_display_safe(&out);
}

#[test]
fn a_merged_row_has_no_marker_and_a_waiting_row_has_one() {
    let out = render(&target_snapshot());
    assert!(line_with(&out, "Credit-backed").ends_with("100%  merged"));
    assert!(!line_with(&out, "Credit-backed").contains('⏸'));
    assert!(line_with(&out, "Rotator").ends_with("⏸ queued"));
    assert!(line_with(&out, "API-key profiles").contains("▶ PR open"));

    // Done is done, even with a live session still attached.
    let mut merged = work("x", 1, PrCell::Known(2), 100, "merged");
    merged.running = true;
    let out = render(&small(vec![merged], None));
    assert!(!out.contains('▶') && !out.contains('⏸'), "{out}");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Workstream rows
// ---------------------------------------------------------------------------

#[test]
fn concerns_conflicts_and_reason_follow_the_stage() {
    let mut row = work("x", 1, PrCell::Known(2), 44, "review round 2");
    row.concerns = Some(3);
    row.conflicts = true;
    row.reason = Some("stopped".into());
    let out = render(&small(vec![row], None));
    assert!(
        line_with(&out, "review round 2")
            .ends_with("⏸ review round 2, 3 concerns, conflicts with main, stopped"),
        "{out}"
    );
}

#[test]
fn missing_and_unknown_numbers_render_as_dash_and_question_mark() {
    let mut none = work("no-pr", 1, PrCell::None, 15, "building");
    none.issue = None;
    let unknown = work("gh-down", 1, PrCell::Unknown, 30, "PR state unknown");
    let out = render(&small(vec![none, unknown], None));
    let a = line_with(&out, "no-pr");
    assert!(!a.contains('#'), "{a}");
    assert!(a.contains(" - "), "{a}");
    assert!(line_with(&out, "gh-down").contains("#?"));
}

#[test]
fn long_labels_are_cut_to_30_characters() {
    let label = "a-very-long-workstream-label-that-goes-on-and-on";
    let out = render(&small(
        vec![work(label, 1, PrCell::None, 0, "queued")],
        None,
    ));
    assert!(!out.contains(label));
    let cut: String = label.chars().take(29).collect();
    assert!(out.contains(&format!("{cut}…")), "{out}");
}

#[test]
fn a_long_stage_is_cut_to_fit_the_line() {
    let mut row = work("x", 1, PrCell::None, 15, "s".repeat(300).as_str());
    row.reason = Some("r".repeat(200));
    let out = render(&small(vec![row], None));
    assert_display_safe(&out);
    assert!(line_with(&out, "sss").ends_with('…'));
}

#[test]
fn hostile_text_is_sanitised_before_it_is_cut_or_padded() {
    let mut row = work("evil\x1b]0;title\x07label", 1, PrCell::None, 0, "x");
    row.stage = "stage\x1b[2J\u{202e}".into();
    row.reason = Some("why\r\nnext".into());
    let mut snap = small(
        vec![row],
        Some(LoginsSection::Rows(vec![login(
            "prof\x1b[31m",
            false,
            bars(10.0, 10.0, "ok"),
        )])),
    );
    snap.repo = Some("re\x1bpo".into());
    snap.when = "now\u{202e}".into();
    let out = render(&snap);
    assert_display_safe(&out);
    assert!(out.contains("evil?]0;title?label"), "{out}");
    assert!(out.starts_with("  re?po workstreams — now?\n"), "{out}");
    assert_eq!(out.lines().filter(|l| l.contains("why")).count(), 1);
}

#[test]
fn empty_sections_say_so() {
    let out = render(&small(vec![], Some(LoginsSection::Rows(vec![]))));
    assert!(out.contains("  no workstreams\n"), "{out}");
    assert!(out.contains("  logins: none\n"), "{out}");
    assert!(
        !out.contains("WORKSTREAM "),
        "no headings over nothing: {out}"
    );
}

#[test]
fn without_an_identity_the_hint_line_is_printed() {
    let mut snap = small(vec![], None);
    snap.no_identity = true;
    assert!(
        render(&snap).contains("no session id found; run with --all to see every run on this host"),
    );
}

#[test]
fn no_logins_section_when_logins_were_not_asked_for() {
    let out = render(&small(vec![work("x", 1, PrCell::None, 0, "queued")], None));
    assert!(!out.contains("LOGIN"), "{out}");
}

#[test]
fn a_refused_logins_section_prints_the_reason_in_place_of_rows() {
    let out = render(&small(
        vec![],
        Some(LoginsSection::Refused(
            "profiles are not applied in Docker\x1b[0m".into(),
        )),
    ));
    assert_display_safe(&out);
    assert!(
        out.contains("  logins: profiles are not applied in Docker?[0m"),
        "{out}"
    );
}

// ---------------------------------------------------------------------------
// The examples agents are shown
// ---------------------------------------------------------------------------

/// The skill tells agents to paste the output verbatim, so its example must
/// be output the renderer produces, and so must the reference page's.
#[test]
fn the_skill_and_reference_examples_are_the_rendered_target() {
    let skill = include_str!("../../../../../amplifier-bundle/skills/workstream-status/SKILL.md");
    let reference = include_str!("../../../../../docs/reference/workstreams-command.md");
    let rendered = render(&target_snapshot());
    for (name, doc) in [("SKILL.md", skill), ("workstreams-command.md", reference)] {
        assert!(
            doc.contains(&format!("```text\n{rendered}```")),
            "{name} does not show the rendered target output"
        );
        assert!(
            !doc.contains("ELAPSED"),
            "{name} shows a column that is not printed"
        );
    }
}
