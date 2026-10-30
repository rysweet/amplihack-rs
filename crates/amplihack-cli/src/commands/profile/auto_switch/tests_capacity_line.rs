//! The login capacity line (issue #1745): the bar, the line format and the
//! spare count. All pure; nothing here touches the filesystem.

use super::capacity_line::{SPARE_FIVE_HOUR, SPARE_SEVEN_DAY, bar, render, spare};
use super::usage::{Reading, ReadingSource, Window};
use std::collections::BTreeMap;

const NOW: i64 = 1_800_000_000;
const HOUR: i64 = 3600;

fn filled(n: usize) -> String {
    format!("{}{}", "█".repeat(n), "░".repeat(10 - n))
}

// ---------------------------------------------------------------------------
// bar
// ---------------------------------------------------------------------------

#[test]
fn bar_fills_one_cell_per_whole_ten_percent() {
    for (pct, cells) in [
        (0.0, 0),
        (9.99, 0),
        (10.0, 1),
        (34.0, 3),
        (43.0, 4),
        (50.0, 5),
        (99.9, 9),
        (100.0, 10),
    ] {
        assert_eq!(bar(pct), filled(cells), "{pct}");
    }
}

#[test]
fn bar_is_full_only_at_or_above_one_hundred() {
    assert_eq!(bar(99.999), filled(9));
    assert_eq!(bar(100.0), filled(10));
    assert_eq!(bar(130.0), filled(10));
}

#[test]
fn bar_is_total_for_odd_values() {
    assert_eq!(bar(f64::NAN), filled(0), "NaN is an empty bar");
    assert_eq!(bar(-5.0), filled(0));
    assert_eq!(bar(f64::NEG_INFINITY), filled(0));
    assert_eq!(bar(f64::INFINITY), filled(10));
    for pct in [f64::NAN, -5.0, f64::INFINITY, f64::NEG_INFINITY, 1e300] {
        assert_eq!(bar(pct).chars().count(), 10, "{pct}");
    }
}

// ---------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------

#[test]
fn render_prints_both_windows_and_the_spare_count() {
    assert_eq!(
        render("work", Some(34.0), Some(43.0), Some(2)).as_deref(),
        Some("🔑 work  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%  · 2 spare")
    );
}

#[test]
fn render_prints_zero_spare() {
    assert_eq!(
        render("work", Some(34.0), Some(43.0), Some(0)).as_deref(),
        Some("🔑 work  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%  · 0 spare")
    );
}

#[test]
fn render_leaves_out_the_spare_segment_when_unknown() {
    assert_eq!(
        render("work", Some(34.0), Some(43.0), None).as_deref(),
        Some("🔑 work  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%")
    );
}

#[test]
fn render_with_five_hour_only_drops_the_seven_day_segment() {
    assert_eq!(
        render("work", Some(34.0), None, Some(1)).as_deref(),
        Some("🔑 work  5h ███░░░░░░░ 34%  · 1 spare")
    );
}

#[test]
fn render_with_seven_day_only_drops_the_five_hour_segment() {
    assert_eq!(
        render("work", None, Some(43.0), None).as_deref(),
        Some("🔑 work  7d ████░░░░░░ 43%")
    );
}

#[test]
fn render_with_no_windows_prints_nothing() {
    assert_eq!(render("work", None, None, Some(3)), None);
    assert_eq!(render("work", None, None, None), None);
}

#[test]
fn render_at_one_hundred_and_above() {
    assert_eq!(
        render("w", Some(100.0), Some(130.0), None).as_deref(),
        Some("🔑 w  5h ██████████ 100%  7d ██████████ 130%"),
        "the pure renderer prints the value it is given"
    );
}

#[test]
fn render_rounds_half_away_from_zero() {
    assert_eq!(
        render("w", Some(33.5), Some(42.5), None).as_deref(),
        Some("🔑 w  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%")
    );
    assert_eq!(
        render("w", Some(33.4), Some(0.0), None).as_deref(),
        Some("🔑 w  5h ███░░░░░░░ 33%  7d ░░░░░░░░░░ 0%")
    );
}

#[test]
fn render_is_total_for_odd_values() {
    for pct in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0] {
        let line = render("w", Some(pct), Some(pct), Some(0))
            .unwrap_or_else(|| panic!("{pct}: a present window is rendered"));
        assert!(line.starts_with("🔑 w  5h "), "{pct}: {line}");
        assert!(
            !line.contains("NaN") && !line.contains("inf"),
            "{pct}: {line}"
        );
        assert!(!line.chars().any(char::is_control), "{pct}: {line:?}");
    }
    let neg = render("w", Some(-5.0), None, None).unwrap();
    assert!(neg.contains(&filled(0)), "{neg}");
}

#[test]
fn render_has_no_newline_or_control_characters() {
    let line = render("work-2", Some(12.0), Some(99.0), Some(7)).unwrap();
    assert!(!line.chars().any(char::is_control), "{line:?}");
}

// ---------------------------------------------------------------------------
// spare
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

fn accounts(entries: &[(&str, Reading)]) -> BTreeMap<String, Reading> {
    entries
        .iter()
        .map(|(n, r)| (n.to_string(), r.clone()))
        .collect()
}

fn all_exist(_: &str) -> bool {
    true
}

#[test]
fn thresholds_are_fifty_and_ninety() {
    assert_eq!(SPARE_FIVE_HOUR, 50.0);
    assert_eq!(SPARE_SEVEN_DAY, 90.0);
}

#[test]
fn a_profile_under_both_thresholds_is_spare() {
    let a = accounts(&[("x", live(10.0, 10.0)), ("y", live(49.9, 89.9))]);
    assert_eq!(spare(&a, "x", NOW, all_exist), 1);
}

#[test]
fn the_thresholds_themselves_are_not_spare() {
    let a = accounts(&[
        ("x", live(0.0, 0.0)),
        ("at-five", live(50.0, 10.0)),
        ("at-seven", live(10.0, 90.0)),
        ("over", live(80.0, 95.0)),
    ]);
    assert_eq!(spare(&a, "x", NOW, all_exist), 0);
}

#[test]
fn the_live_login_is_never_spare() {
    let a = accounts(&[("x", live(1.0, 1.0)), ("y", live(1.0, 1.0))]);
    assert_eq!(spare(&a, "x", NOW, all_exist), 1);
    assert_eq!(spare(&a, "nobody", NOW, all_exist), 2);
}

#[test]
fn an_expired_window_counts_as_zero() {
    let a = accounts(&[
        ("x", live(1.0, 1.0)),
        ("y", reading(win(99.0, NOW - 1), win(95.0, NOW - HOUR))),
        ("z", reading(win(99.0, NOW - 1), win(95.0, NOW + HOUR))),
    ]);
    assert_eq!(
        spare(&a, "x", NOW, all_exist),
        1,
        "y has both windows reset; z's seven-day window still holds"
    );
}

#[test]
fn a_profile_with_a_missing_window_is_not_spare() {
    let a = accounts(&[
        ("x", live(1.0, 1.0)),
        ("five-only", reading(win(1.0, NOW + HOUR), None)),
        ("seven-only", reading(None, win(1.0, NOW + HOUR))),
        ("neither", reading(None, None)),
    ]);
    assert_eq!(spare(&a, "x", NOW, all_exist), 0);
}

#[test]
fn a_profile_whose_directory_is_gone_is_not_spare() {
    let a = accounts(&[
        ("x", live(1.0, 1.0)),
        ("y", live(1.0, 1.0)),
        ("gone", live(1.0, 1.0)),
    ]);
    assert_eq!(spare(&a, "x", NOW, |n: &str| n != "gone"), 1);
}

#[test]
fn spare_asks_only_about_candidates() {
    let a = accounts(&[("x", live(1.0, 1.0)), ("y", live(1.0, 1.0))]);
    let asked = std::cell::RefCell::new(Vec::new());
    let n = spare(&a, "x", NOW, |name: &str| {
        asked.borrow_mut().push(name.to_string());
        true
    });
    assert_eq!(n, 1);
    assert!(
        !asked.borrow().iter().any(|n| n == "x"),
        "the live login needs no stat: {:?}",
        asked.borrow()
    );
}

#[test]
fn no_readings_means_zero_spare() {
    assert_eq!(spare(&BTreeMap::new(), "x", NOW, all_exist), 0);
}

// ---------------------------------------------------------------------------
// Source guard (requirement 11)
// ---------------------------------------------------------------------------

#[test]
fn the_new_modules_never_name_credentials() {
    for (name, src) in [
        ("capacity_line.rs", include_str!("capacity_line.rs")),
        ("home_line.rs", include_str!("home_line.rs")),
    ] {
        assert!(!src.contains(".credentials.json"), "{name}");
        assert!(!src.contains("CREDS"), "{name}");
        for banned in ["TcpStream", "UdpSocket", "std::net", "reqwest", "ureq"] {
            assert!(!src.contains(banned), "{name} uses {banned}");
        }
    }
}
