//! Crusty round 7 on #1831: the governor uses a stored reading only while it
//! is no older than `governor.max_reading_age_secs`, the age test the
//! rotator's `fresh_reading` (#1844) applies before `usage::plausible`. An
//! older reading leaves its profile unknown, and status shows its age.

use super::budget::stale_reading;
use super::config::Settings;
use super::inputs::budgets;
use super::status::{render_table, view};
use super::tests_seed::{NOW, Root};
use crate::commands::profile::governor_view::ProfileSnapshot;
use serde_json::Value;

const DAY: i64 = 86_400;
const HOUR: i64 = 3600;
const MAX_AGE: u64 = 3600;

fn snap(name: &str, weekly: f64, weekly_reset: i64, observed_at: i64) -> ProfileSnapshot {
    ProfileSnapshot {
        name: name.to_string(),
        active: false,
        eligible: true,
        five_hour: None,
        seven_day: Some((weekly, weekly_reset)),
        observed_at,
        no_session_window: true,
    }
}

/// Crusty's example: read six days ago at 10% weekly use, with the weekly
/// reset one day away. The reset has not passed, so without an age limit the
/// profile would count as 90 points of room.
#[test]
fn a_six_day_old_reading_with_its_reset_ahead_is_left_out_of_the_pool() {
    let observed = NOW - 6 * DAY;
    let s = snap("beta", 10.0, NOW + DAY, observed);
    assert!(stale_reading(&s, MAX_AGE, NOW));
    assert!(budgets(&[s], NOW, MAX_AGE).is_empty());
}

#[test]
fn a_reading_just_inside_the_limit_is_kept_and_one_just_outside_is_not() {
    let reset = NOW + DAY;
    let inside = snap("alpha", 10.0, reset, NOW - 3600);
    let outside = snap("beta", 10.0, reset, NOW - 3601);
    assert!(!stale_reading(&inside, MAX_AGE, NOW));
    assert!(stale_reading(&outside, MAX_AGE, NOW));
    let b = budgets(&[inside, outside], NOW, MAX_AGE);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].name, "alpha");
    assert_eq!(b[0].weekly.used_pct, 10.0);
}

/// As in `fresh_reading`, a reading time after `now` is not trusted.
#[test]
fn a_reading_from_the_future_is_stale() {
    let s = snap("alpha", 10.0, NOW + DAY, NOW + 60);
    assert!(stale_reading(&s, MAX_AGE, NOW));
}

#[test]
fn a_profile_without_a_reading_is_not_stale() {
    let mut s = snap("alpha", 10.0, NOW + DAY, 0);
    s.seven_day = None;
    assert!(!stale_reading(&s, MAX_AGE, NOW));
}

/// Moves `name`'s stored reading `secs` seconds into the past, keeping its
/// windows' reset times.
fn age(r: &Root, name: &str, secs: i64) {
    let path = r.root.join(".auto-switch.d").join("usage.json");
    let mut usage: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    usage["accounts"][name]["observed_at"] = (NOW - secs).into();
    std::fs::write(&path, usage.to_string()).unwrap();
}

/// The reserve check uses only fresh readings: a stale 10% on beta does not
/// hide that alpha, the one fresh profile, is inside the reserve.
#[test]
fn a_stale_reading_cannot_release_the_reserve_and_status_shows_its_age() {
    let r = Root::new(&[("alpha", 0.0, 95.0), ("beta", -1.0, 10.0)]);
    age(&r, "beta", 2 * HOUR);
    let settings = Settings::default();
    let v = serde_json::to_value(view(Some(&r.root), &settings, None, NOW)).unwrap();
    let profiles = v["profiles"].as_array().unwrap();
    let alpha = profiles.iter().find(|p| p["name"] == "alpha").unwrap();
    let beta = profiles.iter().find(|p| p["name"] == "beta").unwrap();
    assert_eq!(alpha["stale_reading"], false, "{v}");
    assert_eq!(alpha["observed_at"], NOW);
    assert_eq!(beta["stale_reading"], true, "{v}");
    assert_eq!(beta["invalid_reading"], false);
    assert_eq!(beta["observed_at"], NOW - 2 * HOUR);
    assert_eq!(beta["weekly"], Value::Null);
    assert_eq!(v["reserve"]["best_weekly_remaining"], 5.0);
    assert_eq!(v["reserve"]["ordinary_held"], true);
    let table = render_table(&view(Some(&r.root), &settings, None, NOW));
    assert!(table.contains("stale reading"), "{table}");
    assert!(table.contains("reading: 2027-01-15 06:00 UTC"), "{table}");
}

/// A longer `governor.max_reading_age_secs` keeps the same reading.
#[test]
fn the_limit_follows_the_setting() {
    let r = Root::new(&[("alpha", 0.0, 95.0), ("beta", -1.0, 10.0)]);
    age(&r, "beta", 2 * HOUR);
    let settings = Settings {
        max_reading_age_secs: 3 * 3600,
        ..Settings::default()
    };
    let v = serde_json::to_value(view(Some(&r.root), &settings, None, NOW)).unwrap();
    assert_eq!(v["reserve"]["best_weekly_remaining"], 90.0, "{v}");
    assert_eq!(v["reserve"]["ordinary_held"], false);
}

#[test]
fn a_profile_without_a_reading_has_no_reading_time() {
    let r = Root::new(&[]);
    let v = serde_json::to_value(view(Some(&r.root), &Settings::default(), None, NOW)).unwrap();
    assert_eq!(v["profiles"][0]["observed_at"], Value::Null, "{v}");
    assert_eq!(v["profiles"][0]["stale_reading"], false);
}
