//! Crusty round 7 on #1831: the governor uses a stored reading only while it
//! is no older than `governor.max_reading_age_secs`, the age test the
//! rotator's `fresh_reading` (#1844) applies before `usage::plausible`. An
//! older reading leaves its profile unknown, and status shows its age.
//!
//! Crusty round 8: an older reading is a lower bound on use, so one that
//! shows a window exhausted with its reset still ahead keeps the profile in
//! the pool as exhausted. Dropping it emptied the pool an hour into every
//! exhaustion hold, and step 1 then admitted everything (`no-readings`).

use super::budget::stale_reading;
use super::config::Settings;
use super::inputs::budgets;
use super::model::{Class, DecisionKind, ReasonCode};
use super::policy::decide;
use super::status::{render_table, view};
use super::tests_policy::fixture::{check, inputs};
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
    let settings = Settings::on();
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
        ..Settings::on()
    };
    let v = serde_json::to_value(view(Some(&r.root), &settings, None, NOW)).unwrap();
    assert_eq!(v["reserve"]["best_weekly_remaining"], 90.0, "{v}");
    assert_eq!(v["reserve"]["ordinary_held"], false);
}

#[test]
fn a_profile_without_a_reading_has_no_reading_time() {
    let r = Root::new(&[]);
    let v = serde_json::to_value(view(Some(&r.root), &Settings::on(), None, NOW)).unwrap();
    assert_eq!(v["profiles"][0]["observed_at"], Value::Null, "{v}");
    assert_eq!(v["profiles"][0]["stale_reading"], false);
}

fn with_session(
    name: &str,
    session: (f64, i64),
    weekly: (f64, i64),
    observed_at: i64,
) -> ProfileSnapshot {
    ProfileSnapshot {
        five_hour: Some(session),
        seven_day: Some(weekly),
        no_session_window: false,
        ..snap(name, 0.0, 0, observed_at)
    }
}

fn decide_on(snaps: &[ProfileSnapshot]) -> super::model::Decision {
    decide(&inputs(budgets(snaps, NOW, MAX_AGE), Class::Ordinary), NOW)
}

/// Crusty round 8's case: the one eligible profile is at 100% weekly, read
/// two hours ago, with the reset three days ahead. It is still exhausted.
#[test]
fn a_stale_exhausted_weekly_reading_still_holds_at_step_2() {
    let s = snap("alpha", 100.0, NOW + 3 * DAY, NOW - 2 * HOUR);
    assert!(stale_reading(&s, MAX_AGE, NOW));
    let b = budgets(std::slice::from_ref(&s), NOW, MAX_AGE);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].weekly.used_pct, 100.0);
    let d = decide_on(&[s]);
    check(&d, DecisionKind::Deny, ReasonCode::WeeklyExhausted);
    assert_eq!(d.retry_after, Some(NOW + 3 * DAY));
}

/// The session-window version: the session is at 100%, read two hours ago,
/// with its reset three hours ahead, and the week has room.
#[test]
fn a_stale_exhausted_session_reading_still_holds_at_step_3() {
    let s = with_session(
        "alpha",
        (100.0, NOW + 3 * HOUR),
        (40.0, NOW + 3 * DAY),
        NOW - 2 * HOUR,
    );
    assert!(stale_reading(&s, MAX_AGE, NOW));
    let d = decide_on(&[s]);
    check(&d, DecisionKind::Wait, ReasonCode::SessionExhausted);
    assert_eq!(d.retry_after, Some(NOW + 3 * HOUR));
}

/// Once the exhausted window's reset has passed, the stale reading says
/// nothing about the new window and the profile becomes unknown.
#[test]
fn a_stale_exhausted_reading_whose_reset_has_passed_is_unknown() {
    let snaps = || {
        [
            snap("alpha", 100.0, NOW - 60, NOW - 2 * DAY),
            with_session(
                "beta",
                (100.0, NOW - 60),
                (40.0, NOW + 3 * DAY),
                NOW - 2 * HOUR,
            ),
        ]
    };
    assert!(budgets(&snaps(), NOW, MAX_AGE).is_empty());
    check(
        &decide_on(&snaps()),
        DecisionKind::Admit,
        ReasonCode::NoReadings,
    );
}

/// A stale exhausted profile adds no room: a fresh profile with room is
/// chosen, and with no fresh profile the low stale reading does not count.
#[test]
fn a_stale_exhausted_profile_adds_no_room() {
    let stale = || snap("beta", 100.0, NOW + DAY, NOW - 2 * HOUR);
    let low = || snap("gamma", 10.0, NOW + DAY, NOW - 2 * HOUR);
    let d = decide_on(&[snap("alpha", 50.0, NOW + DAY, NOW), stale(), low()]);
    check(&d, DecisionKind::Admit, ReasonCode::Room);
    assert!(d.reason.message.contains("alpha"), "{d:?}");
    check(
        &decide_on(&[stale(), low()]),
        DecisionKind::Deny,
        ReasonCode::WeeklyExhausted,
    );
}

/// Status keeps such a profile's windows and marks the reading as stale.
#[test]
fn status_shows_a_kept_stale_exhausted_reading() {
    let r = Root::new(&[("alpha", -1.0, 100.0)]);
    age(&r, "alpha", 2 * HOUR);
    let v = view(Some(&r.root), &Settings::on(), None, NOW);
    let json = serde_json::to_value(&v).unwrap();
    assert_eq!(json["profiles"][0]["stale_reading"], true, "{json}");
    assert_eq!(json["profiles"][0]["weekly"]["used_pct"], 100.0, "{json}");
    let table = render_table(&v);
    assert!(table.contains("alpha [active, stale reading]"), "{table}");
}

/// Crusty round 9: a profile kept only because its stale session window is
/// exhausted adds no weekly room to the status reserve summary, as it adds
/// none at step 4. Session 100% and weekly 10%, read two hours ago.
#[test]
fn status_counts_no_weekly_room_from_a_stale_session_exhausted_reading() {
    let r = Root::new(&[("alpha", 100.0, 10.0)]);
    age(&r, "alpha", 2 * HOUR);
    let v = view(Some(&r.root), &Settings::on(), None, NOW);
    let json = serde_json::to_value(&v).unwrap();
    assert_eq!(json["profiles"][0]["stale_reading"], true, "{json}");
    assert_eq!(json["profiles"][0]["session"]["used_pct"], 100.0, "{json}");
    assert_ne!(json["reserve"]["best_weekly_remaining"], 90.0, "{json}");
    assert_eq!(
        json["reserve"]["best_weekly_remaining"],
        Value::Null,
        "{json}"
    );
    // Crusty round 10: the pool is empty, but `decide` waits at step 3
    // (session-exhausted), so status reports the hold rather than "admitted".
    assert_eq!(json["reserve"]["ordinary_held"], true, "{json}");
    assert_eq!(
        json["reserve"]["held_reason"], "session-exhausted",
        "{json}"
    );
    let table = render_table(&v);
    assert!(!table.contains("best weekly remainder 90.0"), "{table}");
    assert!(
        table.contains("ordinary work held: session limit used"),
        "{table}"
    );
}

/// A fresh profile beside it still sets the remainder from its own reading.
#[test]
fn status_reserve_uses_the_fresh_profile_beside_a_stale_session_exhausted_one() {
    let r = Root::new(&[("alpha", 100.0, 10.0), ("beta", 20.0, 70.0)]);
    age(&r, "alpha", 2 * HOUR);
    let v = serde_json::to_value(view(Some(&r.root), &Settings::on(), None, NOW)).unwrap();
    assert_eq!(v["reserve"]["best_weekly_remaining"], 30.0, "{v}");
    assert_eq!(v["reserve"]["ordinary_held"], false, "{v}");
}
