//! Issue #1907: `rotate::capacity`, the rule for what a stay or an ALERT
//! records for the limit-resume hook, and the two alert fields.
//!
//! Contract (design for #1907):
//! - `Room::of(live: Option<&Probed>, reading: &Reading, platform_live: bool)
//!   -> Room`, with `Room::{Fresh, Platform, Unread}` (`Debug`, `Clone`,
//!   `Copy`, `PartialEq`, `Eq`). Pure: no file, process or clock.
//!   - `platform_live` gives `Platform`, whatever the probe says (#1700).
//!   - `Fresh` only for a `Reading::Measured` below 100.0 on both windows,
//!     not carried, unknown, still running or spend-limited. NaN and exactly
//!     100.0 are not below 100. Everything else is `Unread`.
//!   - With no probe (`None`), `reading` alone decides, by the same rule.
//! - `Room::live(probed, reading, platform, tag, spend_blocked)`: an active
//!   spend block on the live login is `Unread`, whatever else holds.
//! - `seen_at(probed, measured_at)`: the live reading's own time, never
//!   later than `measured_at`; what a `stayed` cycle records.
//! - `Room::stay(self, l: &mut RotateLedger) -> CycleOutcome`: `Fresh` and
//!   `Platform` clear `alert` and `advisory_alert` and give `Stayed`;
//!   `Unread` leaves both fields and gives `StayedUnread`.
//! - `Room::alert(self, out: &mut Sink, l: &mut RotateLedger, kind: &str,
//!   key: &str, t: i64, line: &str) -> CycleOutcome`: `Fresh` with
//!   `kind == alert::NO_CAPACITY` stores `key` in `advisory_alert`, sets
//!   `alert` to `None` and gives `Stayed`; every other case stores `key` in
//!   `alert`, clears `advisory_alert` and gives `Alerted`.
//! - `alert::NO_CAPACITY == "no-capacity"`. `alert::should_alert(l, key)`
//!   and `alert::should_alert_advisory(l, key)` return false when `key` is
//!   already stored in either field; each moves `key` into its own field and
//!   clears the other. `alert::clear_alert` clears both.
//! - `CycleOutcome::StayedUnread.as_str() == "stayed-unread"`.
//! - `load_ledger` rejects an invalid `advisory_alert`, and a ledger with
//!   both fields set, as `RotateInconsistent`.
//! - `for_older_readers(l, true)` strips `advisory_alert`.

use super::super::alert::{NO_CAPACITY, clear_alert, should_alert, should_alert_advisory};
use super::super::cycle::CycleOutcome;
use super::super::decide::{Probed, Reading};
use super::super::ledger::{RotateLedger, RotateState, for_older_readers};
use super::super::log::Sink;
use super::super::overflow::Tag;
use super::super::tests_issue_1870::{exh, rd};
use super::{Room, seen_at};
use crate::commands::profile::ProfileError;
use std::fs;
use std::os::unix::fs::PermissionsExt;

const KEY: &str = "no-capacity:work";

fn sink() -> Sink {
    Sink::new(None, false, false)
}

fn room(p: &Probed) -> Room {
    Room::of(Some(p), &p.reading, false)
}

// --- Room::of --------------------------------------------------------------

#[test]
fn a_measured_reading_below_100_on_both_windows_is_fresh() {
    for (s, w) in [(0.0, 0.0), (40.0, 30.0), (92.0, 60.0), (99.9, 99.9)] {
        assert_eq!(room(&rd(s, w, None)), Room::Fresh, "s={s} w={w}");
    }
}

/// `Room::of` judges what the reading says, not when it was taken: a
/// passive reading below 100% is room. Its time is the cycle's concern:
/// the stay is recorded at `seen_at`, the reading's own time, so a reading
/// taken before a limit hit is never capacity after it (see (f) in
/// `tests_issue_1907_cycle.rs`).
#[test]
fn a_passive_reading_below_100_is_fresh_and_recorded_at_its_own_time() {
    let read = 1_760_000_000;
    let p = rd(40.0, 30.0, Some(read));
    assert_eq!(room(&p), Room::Fresh);
    let probes = Some((p, Vec::new()));
    assert_eq!(
        seen_at(probes.as_ref(), read + 60),
        read,
        "not the cycle's time"
    );
    assert_eq!(
        seen_at(probes.as_ref(), read - 60),
        read - 60,
        "never later than the cycle"
    );
    let now_read = Some((rd(40.0, 30.0, None), Vec::new()));
    assert_eq!(
        seen_at(now_read.as_ref(), read),
        read,
        "a read made in this cycle"
    );
    assert_eq!(seen_at(None, read), read);
}

#[test]
fn an_active_spend_block_makes_any_reading_unread() {
    let probes = |p: Probed| Some((p, Vec::new()));
    let fresh = rd(40.0, 30.0, None);
    for tag in [None, Some(Tag::On), Some(Tag::In), Some(Tag::Out)] {
        let got = Room::live(
            probes(fresh.clone()).as_ref(),
            &fresh.reading,
            false,
            tag,
            true,
        );
        assert_eq!(got, Room::Unread, "{tag:?}");
    }
    let got = Room::live(
        probes(fresh.clone()).as_ref(),
        &fresh.reading,
        true,
        None,
        true,
    );
    assert_eq!(got, Room::Unread, "an API-key login with a block");
    assert_eq!(
        Room::live(None, &fresh.reading, false, None, true),
        Room::Unread
    );
}

#[test]
fn exactly_100_or_more_on_either_window_is_unread() {
    for (s, w) in [(100.0, 10.0), (10.0, 100.0), (100.0, 100.0), (150.0, 0.0)] {
        assert_eq!(room(&rd(s, w, None)), Room::Unread, "s={s} w={w}");
    }
}

#[test]
fn nan_is_unread() {
    assert_eq!(room(&rd(f64::NAN, 10.0, None)), Room::Unread);
    assert_eq!(room(&rd(10.0, f64::NAN, None)), Room::Unread);
}

#[test]
fn an_exhausted_reading_is_unread_even_with_figures_below_100() {
    assert_eq!(room(&exh(43.0, 99.0)), Room::Unread);
    assert_eq!(room(&exh(100.0, 100.0)), Room::Unread);
}

#[test]
fn a_failed_probe_is_unread() {
    assert_eq!(room(&Probed::failed()), Room::Unread);
    let mut known = Probed::failed();
    known.unknown = false;
    assert_eq!(room(&known), Room::Unread, "failed but not unknown");
}

#[test]
fn every_probe_flag_makes_a_good_reading_unread() {
    type Flag = fn(&mut Probed);
    let flags: [(&str, Flag); 4] = [
        ("carried", |p| p.carried = true),
        ("unknown", |p| p.unknown = true),
        ("still_running", |p| p.still_running = true),
        ("spend_limit", |p| p.spend_limit = true),
    ];
    for (what, set) in flags {
        let mut p = rd(40.0, 30.0, None);
        set(&mut p);
        assert_eq!(room(&p), Room::Unread, "{what}");
    }
}

#[test]
fn without_a_probe_the_reading_decides() {
    let measured = Reading::Measured {
        session: 40.0,
        week: 30.0,
    };
    assert_eq!(Room::of(None, &measured, false), Room::Fresh);
    assert_eq!(Room::of(None, &Reading::Failed, false), Room::Unread);
    assert_eq!(Room::of(None, &Reading::EXHAUSTED, false), Room::Unread);
}

#[test]
fn a_platform_live_login_is_platform_whatever_its_probe() {
    for p in [rd(0.0, 0.0, None), Probed::failed(), exh(100.0, 100.0)] {
        assert_eq!(Room::of(Some(&p), &p.reading, true), Room::Platform);
    }
    assert_eq!(Room::of(None, &Reading::Failed, true), Room::Platform);
}

#[test]
fn a_stay_on_credits_read_this_cycle_counts_but_never_advises() {
    let probes = |p: Probed| Some((p, Vec::new()));
    let on = Some(Tag::On);
    let full = exh(100.0, 100.0);
    let got = Room::live(
        probes(full.clone()).as_ref(),
        &full.reading,
        false,
        on,
        false,
    );
    assert_eq!(
        got,
        Room::Platform,
        "an overflow login read at 100% runs on credits"
    );
    let fresh = rd(40.0, 30.0, None);
    let got = Room::live(
        probes(fresh.clone()).as_ref(),
        &fresh.reading,
        false,
        on,
        false,
    );
    assert_eq!(got, Room::Fresh);
    let mut unread = vec![Probed::failed()];
    for flag in [0, 1, 2] {
        let mut p = exh(100.0, 100.0);
        match flag {
            0 => p.carried = true,
            1 => p.spend_limit = true,
            _ => p.still_running = true,
        }
        unread.push(p);
    }
    for p in unread {
        let got = Room::live(probes(p.clone()).as_ref(), &p.reading, false, on, false);
        assert_eq!(got, Room::Unread, "{p:?}");
    }
    for tag in [None, Some(Tag::In), Some(Tag::Out)] {
        let got = Room::live(
            probes(full.clone()).as_ref(),
            &full.reading,
            false,
            tag,
            false,
        );
        assert_eq!(got, Room::Unread, "{tag:?}");
    }
    assert_eq!(
        Room::live(None, &full.reading, false, on, false),
        Room::Unread
    );
}

// --- Room::stay ------------------------------------------------------------

fn with_alerts(alert: Option<&str>, advisory: Option<&str>) -> RotateLedger {
    RotateLedger {
        alert: alert.map(str::to_string),
        advisory_alert: advisory.map(str::to_string),
        ..RotateLedger::default()
    }
}

#[test]
fn stayed_unread_is_its_own_outcome() {
    assert_eq!(CycleOutcome::StayedUnread.as_str(), "stayed-unread");
    assert_eq!(CycleOutcome::Stayed.as_str(), "stayed");
}

#[test]
fn a_fresh_or_platform_stay_is_stayed_and_clears_both_alerts() {
    for r in [Room::Fresh, Room::Platform] {
        for (a, adv) in [(Some(KEY), None), (None, Some(KEY)), (None, None)] {
            let mut l = with_alerts(a, adv);
            assert_eq!(r.stay(&mut l), CycleOutcome::Stayed, "{r:?}");
            assert_eq!(l.alert, None, "{r:?}");
            assert_eq!(l.advisory_alert, None, "{r:?}");
        }
    }
}

#[test]
fn an_unread_stay_is_stayed_unread_and_leaves_both_alerts() {
    let mut l = with_alerts(Some(KEY), None);
    assert_eq!(Room::Unread.stay(&mut l), CycleOutcome::StayedUnread);
    assert_eq!(l.alert.as_deref(), Some(KEY));
    let mut l = with_alerts(None, Some(KEY));
    assert_eq!(Room::Unread.stay(&mut l), CycleOutcome::StayedUnread);
    assert_eq!(l.advisory_alert.as_deref(), Some(KEY));
    assert_eq!(l.alert, None);
}

// --- Room::alert -----------------------------------------------------------

#[test]
fn the_no_capacity_kind_is_shared() {
    assert_eq!(NO_CAPACITY, "no-capacity");
}

#[test]
fn a_no_capacity_alert_on_a_fresh_live_login_is_advisory() {
    let mut l = with_alerts(None, None);
    let got = Room::Fresh.alert(&mut sink(), &mut l, NO_CAPACITY, KEY, 1, "ALERT: x");
    assert_eq!(got, CycleOutcome::Stayed);
    assert_eq!(l.alert, None);
    assert_eq!(l.advisory_alert.as_deref(), Some(KEY));
}

#[test]
fn an_advisory_alert_replaces_a_hard_one_for_a_fresh_live_login() {
    let mut l = with_alerts(Some(KEY), None);
    let got = Room::Fresh.alert(&mut sink(), &mut l, NO_CAPACITY, KEY, 1, "ALERT: x");
    assert_eq!(got, CycleOutcome::Stayed);
    assert_eq!(l.alert, None);
    assert_eq!(l.advisory_alert.as_deref(), Some(KEY));
}

#[test]
fn every_other_alert_is_hard() {
    let cases = [
        (Room::Unread, NO_CAPACITY),
        (Room::Platform, NO_CAPACITY),
        (Room::Fresh, "platform-blocked"),
        (Room::Fresh, "key-held"),
        (Room::Fresh, "refused swap"),
        (Room::Platform, "platform-blocked"),
        (Room::Unread, "key-held"),
    ];
    for (r, kind) in cases {
        let key = format!("{kind}:work");
        let mut l = with_alerts(None, Some(KEY));
        let got = r.alert(&mut sink(), &mut l, kind, &key, 1, "ALERT: x");
        assert_eq!(got, CycleOutcome::Alerted, "{r:?} {kind}");
        assert_eq!(l.alert.as_deref(), Some(key.as_str()), "{r:?} {kind}");
        assert_eq!(
            l.advisory_alert, None,
            "a hard ALERT clears the advisory key"
        );
    }
}

// --- The two alert fields --------------------------------------------------

#[test]
fn should_alert_deduplicates_across_both_fields() {
    let mut l = RotateLedger::default();
    assert!(should_alert_advisory(&mut l, KEY), "first time");
    assert!(!should_alert_advisory(&mut l, KEY), "same key");
    assert_eq!(l.advisory_alert.as_deref(), Some(KEY));
    assert!(
        !should_alert(&mut l, KEY),
        "the key moves to alert silently"
    );
    assert_eq!(l.alert.as_deref(), Some(KEY));
    assert_eq!(l.advisory_alert, None, "never both");
    assert!(!should_alert_advisory(&mut l, KEY), "and back, silently");
    assert_eq!(l.alert, None);
    assert_eq!(l.advisory_alert.as_deref(), Some(KEY));
    assert!(
        should_alert(&mut l, "key-held:work"),
        "a new key is written"
    );
}

#[test]
fn clear_alert_clears_both_fields() {
    let mut l = with_alerts(Some(KEY), None);
    clear_alert(&mut l);
    assert_eq!(l.alert, None);
    let mut l = with_alerts(None, Some(KEY));
    clear_alert(&mut l);
    assert_eq!(l.advisory_alert, None);
}

// --- Ledger validation and older readers -----------------------------------

fn state() -> (tempfile::TempDir, RotateState) {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let rs = RotateState::new(tmp.path());
    rs.ensure().unwrap();
    (tmp, rs)
}

#[test]
fn an_advisory_alert_round_trips_through_the_ledger() {
    let (_tmp, rs) = state();
    rs.save_ledger(&with_alerts(None, Some(KEY))).unwrap();
    let l = rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.advisory_alert.as_deref(), Some(KEY));
    assert_eq!(l.alert, None);
}

#[test]
fn a_ledger_without_an_advisory_alert_omits_the_field() {
    let (_tmp, rs) = state();
    rs.save_ledger(&RotateLedger::default()).unwrap();
    let raw = fs::read_to_string(&rs.ledger).unwrap();
    assert!(!raw.contains("advisory_alert"), "{raw}");
}

fn rewrite(rs: &RotateState, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&rs.ledger).unwrap()).unwrap();
    edit(&mut v);
    fs::write(&rs.ledger, serde_json::to_vec(&v).unwrap()).unwrap();
}

#[test]
fn a_ledger_with_both_alert_fields_set_is_inconsistent() {
    let (_tmp, rs) = state();
    rs.save_ledger(&RotateLedger::default()).unwrap();
    rewrite(&rs, |v| {
        v["alert"] = KEY.into();
        v["advisory_alert"] = KEY.into();
    });
    assert!(matches!(
        rs.load_ledger(),
        Err(ProfileError::RotateInconsistent(_))
    ));
}

#[test]
fn an_invalid_advisory_alert_is_inconsistent() {
    let (_tmp, rs) = state();
    rs.save_ledger(&RotateLedger::default()).unwrap();
    for bad in [
        serde_json::json!(""),
        serde_json::json!("a\nb"),
        serde_json::json!("x".repeat(400)),
        serde_json::json!(7),
    ] {
        rewrite(&rs, |v| v["advisory_alert"] = bad.clone());
        assert!(rs.load_ledger().is_err(), "{bad}");
    }
}

#[test]
fn older_readers_never_see_the_advisory_alert() {
    let l = with_alerts(None, Some(KEY));
    assert_eq!(for_older_readers(&l, true).advisory_alert, None);
    assert_eq!(
        for_older_readers(&l, false).advisory_alert.as_deref(),
        Some(KEY),
        "this binary keeps it"
    );
}
