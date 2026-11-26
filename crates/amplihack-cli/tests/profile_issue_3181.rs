//! Issue #3181 end to end, through the `amplihack` binary. Every run uses the
//! rotate `Fixture` (temporary HOME and profile root, a fake `claude` whose
//! `/usage` output each test sets). No real login is read.
//!
//! Checked:
//! - The rotator leaves `work` at 91% of its session. A later reading of
//!   `work` at 70% in the same window (same reset time) is stale: the next
//!   cycle does not move back onto `work`, and `profile usage --json` shows
//!   `work` at 91, not 70.
//! - Control: the same 70% reading in a later window is believed, and the
//!   cycle moves onto `work`. So the refusal above comes from the window's
//!   highest reading, not from damping or the hold.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;

fn stdout(out: &std::process::Output) -> String {
    text(&out.stdout)
}

fn session_used(f: &Fixture, name: &str) -> f64 {
    let out = f.ok(&["profile", "usage", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let row = v
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["profile"] == name)
        .unwrap_or_else(|| panic!("no row for {name}: {v}"))
        .clone();
    let found = ["session", "five_hour"]
        .iter()
        .find_map(|k| row[*k]["used_percentage"].as_f64());
    found.unwrap_or_else(|| panic!("no session figure for {name}: {row}"))
}

/// `work` is left at 91%; `spare` takes over; `extra` is over the limit, so
/// later the only login that could look as if it had room is `work`.
fn leave_work_at_91(f: &Fixture) {
    f.usage_resetting("work", 91, 14, 3);
    f.usage_resetting("spare", 10, 20, 3);
    f.usage_resetting("extra", 96, 50, 3);
    let s = stdout(&f.ok(&["profile", "rotate"]));
    assert!(s.contains("swap work->spare"), "{s}");
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
}

#[test]
fn a_stale_lower_reading_does_not_bring_back_a_login_left_near_its_limit() {
    let f = Fixture::new();
    leave_work_at_91(&f);

    // Same window: work now reads 70% (its figure before it was used), and
    // spare has risen to its swap point.
    f.usage_resetting("work", 70, 14, 3);
    f.usage_resetting("spare", 92, 25, 3);
    let s = stdout(&f.ok(&["profile", "rotate"]));
    assert!(!s.contains("->work"), "moved back onto work: {s}");
    assert_ne!(
        ino(&f.home_login()),
        Some(f.live_ino),
        "~/.claude holds work again"
    );
    assert_eq!(session_used(&f, "work"), 91.0, "the stale 70% is shown");
}

#[test]
fn the_same_lower_reading_in_a_later_window_is_believed() {
    let f = Fixture::new();
    leave_work_at_91(&f);

    // Later window: work has reset and reads 70%.
    f.usage_resetting("work", 70, 14, 8);
    f.usage_resetting("spare", 92, 25, 3);
    let s = stdout(&f.ok(&["profile", "rotate"]));
    assert!(s.contains("swap spare->work"), "{s}");
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    assert_eq!(session_used(&f, "work"), 70.0);
}
