//! The login capacity line (issue #1745): the bar, the line format and the
//! spare rule. All pure; nothing here touches the filesystem.

use super::capacity_line::{SPARE_FIVE_HOUR, SPARE_SEVEN_DAY, is_spare, render};
use super::usage::{Reading, ReadingSource, Window};

const NOW: i64 = 1_800_000_000;
const HOUR: i64 = 3600;

fn filled(n: usize) -> String {
    format!("{}{}", "█".repeat(n), "░".repeat(10 - n))
}

/// The bar `render` draws for one five-hour window.
fn bar(pct: f64) -> String {
    let line = render(None, Some(pct), None, None).unwrap();
    line.chars().skip(3).take(10).collect()
}

// ---------------------------------------------------------------------------
// The bar (D6)
// ---------------------------------------------------------------------------

#[test]
fn the_bar_rounds_half_away_from_zero() {
    for (pct, cells) in [
        (0.0, 0),
        (4.9, 0),
        (5.0, 1),
        (10.0, 1),
        (34.0, 3),
        (35.0, 4),
        (42.5, 4),
        (43.0, 4),
        (50.0, 5),
        (94.9, 9),
        (95.0, 10),
        (96.0, 10),
        (100.0, 10),
    ] {
        assert_eq!(bar(pct), filled(cells), "{pct}");
    }
}

#[test]
fn thirty_five_percent_fills_four_cells() {
    assert_eq!(bar(35.0), filled(4));
}

#[test]
fn the_bar_is_total_for_odd_values() {
    assert_eq!(bar(f64::NAN), filled(0), "NaN is an empty bar");
    assert_eq!(bar(-5.0), filled(0));
    assert_eq!(bar(f64::NEG_INFINITY), filled(0));
    assert_eq!(bar(f64::INFINITY), filled(10));
    assert_eq!(bar(130.0), filled(10));
    assert_eq!(bar(1e300), filled(10));
}

// ---------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------

#[test]
fn render_prints_both_windows_and_the_spare_count() {
    assert_eq!(
        render(Some("work"), Some(34.0), Some(43.0), Some(2)).as_deref(),
        Some("🔑 work  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%  · 2 spare")
    );
}

#[test]
fn render_prints_zero_spare() {
    assert_eq!(
        render(Some("work"), Some(34.0), Some(43.0), Some(0)).as_deref(),
        Some("🔑 work  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%  · 0 spare")
    );
}

#[test]
fn each_segment_is_left_out_on_its_own() {
    for (login, five, seven, spare, want) in [
        (
            Some("work"),
            Some(34.0),
            Some(43.0),
            None,
            "🔑 work  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%",
        ),
        (
            Some("work"),
            Some(34.0),
            None,
            Some(1),
            "🔑 work  5h ███░░░░░░░ 34%  · 1 spare",
        ),
        (
            Some("work"),
            None,
            Some(43.0),
            None,
            "🔑 work  7d ████░░░░░░ 43%",
        ),
        (
            None,
            Some(34.0),
            Some(43.0),
            None,
            "5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%",
        ),
        (Some("work"), None, None, Some(3), "🔑 work  · 3 spare"),
        (None, None, None, Some(0), "· 0 spare"),
        (Some("work"), None, None, None, "🔑 work"),
    ] {
        assert_eq!(render(login, five, seven, spare).as_deref(), Some(want));
    }
}

#[test]
fn render_with_no_segment_prints_nothing() {
    assert_eq!(render(None, None, None, None), None);
    assert_eq!(render(Some(""), None, None, None), None);
}

#[test]
fn values_over_one_hundred_are_printed_as_reported() {
    assert_eq!(
        render(Some("w"), Some(100.0), Some(130.0), None).as_deref(),
        Some("🔑 w  5h ██████████ 100%  7d ██████████ 130%")
    );
}

#[test]
fn render_rounds_the_percentage_half_away_from_zero() {
    assert_eq!(
        render(Some("w"), Some(33.5), Some(42.5), None).as_deref(),
        Some("🔑 w  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%")
    );
    assert_eq!(
        render(Some("w"), Some(33.4), Some(0.0), None).as_deref(),
        Some("🔑 w  5h ███░░░░░░░ 33%  7d ░░░░░░░░░░ 0%")
    );
}

#[test]
fn render_is_total_for_odd_values() {
    for pct in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0] {
        let line = render(Some("w"), Some(pct), Some(pct), Some(0))
            .unwrap_or_else(|| panic!("{pct}: a present window is rendered"));
        assert!(line.starts_with("🔑 w  5h "), "{pct}: {line}");
        assert!(
            !line.contains("NaN") && !line.contains("inf"),
            "{pct}: {line}"
        );
        assert!(!line.chars().any(char::is_control), "{pct}: {line:?}");
    }
}

#[test]
fn control_characters_are_removed_from_the_login() {
    // SR9: an ESC sequence in a name cannot reach the terminal.
    let line = render(Some("wo\u{1b}[2Jrk\n\u{7}"), Some(12.0), None, None).unwrap();
    // ESC, newline and BEL are removed; what is left is inert text.
    assert_eq!(line, "🔑 wo[2Jrk  5h █░░░░░░░░░ 12%");
    assert!(!line.contains('\u{1b}'), "{line:?}");
    assert_eq!(
        render(Some("\u{1b}\u{9b}"), None, None, None),
        None,
        "a login of control characters only is no login"
    );
}

#[test]
fn render_has_no_newline_or_control_characters() {
    let line = render(Some("work-2"), Some(12.0), Some(99.0), Some(7)).unwrap();
    assert!(!line.chars().any(char::is_control), "{line:?}");
}

// ---------------------------------------------------------------------------
// The spare rule (D7)
// ---------------------------------------------------------------------------

fn win(pct: f64, resets_at: i64) -> Option<Window> {
    Some(Window {
        used_percentage: pct,
        resets_at,
    })
}

fn reading(five: Option<Window>, seven: Option<Window>) -> Reading {
    Reading {
        five_hour: five,
        seven_day: seven,
        observed_at: NOW - 60,
        source: ReadingSource::Statusline,
    }
}

fn live(five: f64, seven: f64) -> Reading {
    reading(win(five, NOW + HOUR), win(seven, NOW + 3 * 86_400))
}

#[test]
fn thresholds_are_fifty_and_ninety() {
    assert_eq!(SPARE_FIVE_HOUR, 50.0);
    assert_eq!(SPARE_SEVEN_DAY, 90.0);
}

#[test]
fn a_reading_under_both_thresholds_is_spare() {
    assert!(is_spare(&live(20.0, 30.0), NOW));
    assert!(is_spare(&live(49.9, 89.9), NOW));
}

#[test]
fn the_thresholds_themselves_are_not_spare() {
    assert!(!is_spare(&live(50.0, 10.0), NOW));
    assert!(!is_spare(&live(10.0, 90.0), NOW));
}

#[test]
fn an_expired_window_counts_as_zero() {
    let r = reading(win(99.0, NOW - 1), win(95.0, NOW));
    assert!(is_spare(&r, NOW));
}

#[test]
fn a_reading_without_a_session_window_can_be_spare() {
    // A weekly-only plan (#1818): no five-hour window at all.
    assert!(is_spare(&reading(None, win(40.0, NOW + 86_400)), NOW));
    assert!(!is_spare(&reading(None, win(95.0, NOW + 86_400)), NOW));
}

#[test]
fn a_reading_without_a_seven_day_window_is_not_spare() {
    assert!(!is_spare(&reading(win(10.0, NOW + HOUR), None), NOW));
    assert!(!is_spare(&reading(None, None), NOW));
}

#[test]
fn an_exhausted_login_is_not_spare_even_below_the_thresholds() {
    // A usage-limit failure with a reset still ahead (#1846).
    let mut r = live(20.0, 30.0);
    r.source = ReadingSource::UsageLimitFailure;
    assert!(!is_spare(&r, NOW));
    // Once both windows have reset, it is spare again.
    let mut r = reading(win(20.0, NOW - 1), win(30.0, NOW - 1));
    r.source = ReadingSource::UsageLimitFailure;
    assert!(is_spare(&r, NOW));
}

// ---------------------------------------------------------------------------
// Source guard (AC11, SR1)
// ---------------------------------------------------------------------------

/// The text of the function in `src` that starts at `start`, through the
/// closing brace at the same indentation.
fn function<'a>(src: &'a str, start: &str) -> &'a str {
    let i = src
        .find(start)
        .unwrap_or_else(|| panic!("`{start}` not found"));
    let line = &src[src[..i].rfind('\n').map_or(0, |n| n + 1)..];
    let indent = &line[..line.len() - line.trim_start().len()];
    let close = format!("\n{indent}}}\n");
    let end = src[i..].find(&close).expect("the closing brace") + close.len();
    &src[i..i + end]
}

/// A text check only: it proves these sources do not name a credential
/// file or a call known to reach one. It cannot follow calls into other
/// modules (round 2 of PR #1857 found `capacity::refresh` reaching
/// `kind::Ctx::load` this way), so the property itself is checked at run
/// time by `a_recorded_home_tick_with_auto_switch_on_opens_no_credentials_file`
/// in `tests/issue_1745_home_statusline.rs`.
#[test]
fn capacity_line_and_home_line_read_no_credentials() {
    let default_live = include_str!("../default_live.rs");
    let mut sources = vec![
        ("capacity_line.rs", include_str!("capacity_line.rs")),
        ("home_line.rs", include_str!("home_line.rs")),
    ];
    for start in [
        "pub(super) fn ledger_live(",
        "fn newer(",
        "    pub(super) fn since(",
        "    pub(super) fn prototype(root: &Path) -> Ledger",
        "    pub(super) fn rotate(",
        "    fn read_ledger(",
    ] {
        sources.push((start, function(default_live, start)));
    }
    for (name, src) in sources {
        for banned in [
            "Account::of_dir",
            ".credentials.json",
            "CREDS",
            ".claude.json",
            "detect",
            "from_ledger",
        ] {
            assert!(!src.contains(banned), "{name} names {banned}");
        }
        for banned in ["TcpStream", "UdpSocket", "std::net", "reqwest", "ureq"] {
            assert!(!src.contains(banned), "{name} uses {banned}");
        }
    }
    // Calls known to resolve profile kinds, which opens every
    // `.credentials.json`.
    let code: Vec<&str> = include_str!("home_line.rs")
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect();
    let code = code.join("\n");
    for banned in [
        "capacity::refresh(",
        "OverflowInputs",
        "kind::",
        "identify::",
        "login_kind",
    ] {
        assert!(!code.contains(banned), "home_line.rs calls {banned}");
    }
}
