//! Issue #1797 in the rotator: what a cycle adds to the usage history, and
//! the `capacity:` line of `rotate status`.
//!
//! Contract:
//! - `usage_history::from_probes` records the live probe under its profile
//!   only when the live login is a profile's (never an unowned login or the
//!   API-key profile), and leaves failed, spend-limited and weekless probes
//!   out.
//! - `run_cycle` appends one line after the state lock is released when the
//!   measurement carries probes, and none when it does not (a stub). A
//!   history that cannot be written puts one fixed line in the rotate log and
//!   never fails the cycle.
//! - `control::capacity_line` reads only the history and the weights file:
//!   no probe, no process, no `claude`.
//!
//! In process, on the `tests_swap.rs` fixture, with `Ctx::measure` stubbed.

use super::control::capacity_line;
use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::decide::{Decision, Probed, Reading, Win, Windows};
use super::identify::LiveId;
use super::observe::{Measurement, Snap};
use super::settings;
use super::tests_swap::{Env, env, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Liveness, now};
use crate::commands::profile::usage_history::{self, Entry, Point, from_probes, to_line};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;

/// 2026-10-30T16:15:00Z.
const T: i64 = 1_793_376_900;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

fn probed(s: f64, w: f64) -> Probed {
    Probed {
        reading: Reading::Measured {
            session: s,
            week: w,
        },
        windows: Windows {
            session: Some(Win {
                used: s,
                resets_at: Some(T + HOUR),
            }),
            week: Some(Win {
                used: w,
                resets_at: Some(T + DAY),
            }),
        },
        spend_limit: false,
        unknown: false,
    }
}

fn pt(s: Option<u8>, w: u8, w_reset: Option<i64>, s_reset: Option<i64>) -> Point {
    Point {
        s,
        w,
        w_reset,
        s_reset,
    }
}

// ---------------------------------------------------------------------------
// from_probes
// ---------------------------------------------------------------------------

#[test]
fn from_probes_records_the_live_profile_under_its_name() {
    let probes = (probed(10.0, 20.0), vec![(n("spare"), probed(30.4, 40.5))]);
    let s = from_probes(T, Some(&n("work")), &probes, None);
    assert_eq!(s.at, T);
    assert_eq!(
        s.points["work"],
        pt(Some(10), 20, Some(T + DAY), Some(T + HOUR))
    );
    assert_eq!(
        s.points["spare"],
        pt(Some(30), 41, Some(T + DAY), Some(T + HOUR)),
        "rounded half away from zero"
    );
}

#[test]
fn from_probes_drops_an_unowned_live_login() {
    let probes = (probed(10.0, 20.0), vec![(n("spare"), probed(30.0, 40.0))]);
    let s = from_probes(T, None, &probes, None);
    assert_eq!(s.points.keys().collect::<Vec<_>>(), ["spare"]);
}

#[test]
fn from_probes_never_records_the_api_key_profile() {
    let probes = (
        Probed::platform(),
        vec![
            (n("key"), Probed::platform()),
            (n("spare"), probed(30.0, 40.0)),
        ],
    );
    let s = from_probes(T, None, &probes, Some(&n("key")));
    assert_eq!(s.points.keys().collect::<Vec<_>>(), ["spare"]);
    assert!(
        !s.unknown.iter().any(|u| u == "key"),
        "not a login, so not unknown"
    );
}

#[test]
fn from_probes_leaves_failed_spend_limited_and_weekless_probes_out() {
    let mut spend = probed(10.0, 10.0);
    spend.spend_limit = true;
    spend.reading = Reading::Failed;
    let mut weekless = probed(10.0, 10.0);
    weekless.windows.week = None;
    weekless.reading = Reading::Failed;
    let probes = (
        probed(1.0, 2.0),
        vec![
            (n("failed"), Probed::failed()),
            (n("spend"), spend),
            (n("weekless"), weekless),
        ],
    );
    let s = from_probes(T, Some(&n("live")), &probes, None);
    assert_eq!(s.points.keys().collect::<Vec<_>>(), ["live"]);
    assert_eq!(s.unknown, ["failed", "spend", "weekless"]);
}

#[test]
fn from_probes_records_an_exhausted_week_as_100() {
    let mut out = probed(40.0, 100.0);
    out.reading = Reading::Exhausted;
    let probes = (probed(1.0, 2.0), vec![(n("out"), out)]);
    let s = from_probes(T, None, &probes, None);
    assert_eq!(s.points["out"].w, 100);
}

// ---------------------------------------------------------------------------
// The cycle appends one line
// ---------------------------------------------------------------------------

fn ctx(e: &Env, measure: super::cycle::MeasureFn) -> Ctx {
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut c = Ctx::new(e.store.clone(), e.home.clone(), s);
    c.scan = |_| Liveness::Known(BTreeSet::new());
    c.measure = Some(measure);
    c
}

fn stay(probed: Option<super::observe::Probes>) -> Measurement {
    Measurement {
        live: Reading::Measured {
            session: 10.0,
            week: 20.0,
        },
        others: vec![(
            n("spare"),
            Reading::Measured {
                session: 30.0,
                week: 40.0,
            },
        )],
        decision: Decision::Stay,
        note: None,
        failed: 0,
        tag: None,
        new_blocks: Vec::new(),
        next_plan_reset: None,
        measured_at: now(),
        probed,
    }
}

fn measured(_: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    assert_eq!(snap.live, LiveId::Known(n("work")), "work is live");
    Ok(stay(Some((
        probed(10.0, 20.0),
        vec![(n("spare"), probed(30.0, 40.0))],
    ))))
}

fn stubbed(_: &Ctx, _: &Snap) -> Result<Measurement, ProfileError> {
    Ok(stay(None))
}

fn history(e: &Env) -> std::path::PathBuf {
    e.root.join(".auto-switch.d").join(usage_history::FILE)
}

fn lines(e: &Env) -> Vec<String> {
    fs::read_to_string(history(e))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn a_measured_cycle_appends_one_line_with_every_probed_profile() {
    let e = env();
    let mut c = ctx(&e, measured);
    let before = now();
    assert_eq!(
        run_cycle(&mut c, Mode::OneShot).unwrap(),
        CycleOutcome::Stayed
    );
    let first = lines(&e);
    assert_eq!(first.len(), 1, "{first:?}");
    let entry: Entry = usage_history::parse_line(&first[0], now()).expect("a readable line");
    assert!(
        entry.at >= before - 5 && entry.at <= now() + 5,
        "{}",
        entry.at
    );
    let names: Vec<&str> = entry.profiles.keys().map(String::as_str).collect();
    assert_eq!(names, ["spare", "work"]);
    assert_eq!(entry.profiles["work"].w, 20);
    assert_eq!(entry.profiles["spare"].s, Some(30));
    for needle in ["@example.com", "u-work", "u-spare", "s_reset", "SECRET"] {
        assert!(!first[0].contains(needle), "{needle}: {}", first[0]);
    }
    assert_eq!(
        fs::metadata(history(&e)).unwrap().permissions().mode() & 0o777,
        0o600
    );

    assert_eq!(
        run_cycle(&mut c, Mode::OneShot).unwrap(),
        CycleOutcome::Stayed
    );
    assert_eq!(lines(&e).len(), 2, "one line per measured cycle");
}

#[test]
fn a_cycle_without_probes_appends_nothing() {
    let e = env();
    let mut c = ctx(&e, stubbed);
    run_cycle(&mut c, Mode::OneShot).unwrap();
    assert!(lines(&e).is_empty());
}

#[test]
fn a_history_that_cannot_be_written_logs_one_line_and_the_cycle_goes_on() {
    let e = env();
    let target = e.root.join("elsewhere");
    fs::write(&target, "").unwrap();
    std::os::unix::fs::symlink(&target, history(&e)).unwrap();
    let mut c = ctx(&e, measured);
    assert_eq!(
        run_cycle(&mut c, Mode::OneShot).unwrap(),
        CycleOutcome::Stayed
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "", "never followed");
    let log = fs::read_to_string(&e.rs.log).unwrap_or_default();
    assert_eq!(
        log.matches("usage history not updated (unsafe file)")
            .count(),
        1,
        "{log}"
    );
    assert!(!log.contains(&*e.root.to_string_lossy()), "no path: {log}");
}

// ---------------------------------------------------------------------------
// `rotate status`: the capacity line
// ---------------------------------------------------------------------------

fn write_history(e: &Env, entries: &[Entry]) {
    let body: String = entries
        .iter()
        .map(|x| format!("{}\n", to_line(x)))
        .collect();
    fs::create_dir_all(e.root.join(".auto-switch.d")).unwrap();
    fs::write(history(e), body).unwrap();
    fs::set_permissions(history(e), fs::Permissions::from_mode(0o600)).unwrap();
}

fn entry(at: i64, points: &[(&str, Point)]) -> Entry {
    Entry {
        at,
        profiles: points
            .iter()
            .map(|(k, p)| (k.to_string(), p.clone()))
            .collect::<BTreeMap<_, _>>(),
    }
}

#[test]
fn no_history_is_no_recorded_cycle() {
    let e = env();
    assert_eq!(capacity_line(&e.root, T), "capacity: no recorded cycle");
    write_history(&e, &[]);
    assert_eq!(capacity_line(&e.root, T), "capacity: no recorded cycle");
    fs::write(history(&e), "garbage\n").unwrap();
    assert_eq!(capacity_line(&e.root, T), "capacity: no recorded cycle");
}

#[test]
fn the_newest_entry_is_now_and_earlier_entries_give_the_rate() {
    let e = env();
    let far = Some(T + 5 * DAY);
    // 6 h before: 0 + 0 + 30 + 25; now 10 + 30 + 68 + 85: 138 points,
    // 0.230 login-weeks an hour; 2.07 left, so about 9 hours.
    write_history(
        &e,
        &[
            entry(
                T - 6 * HOUR,
                &[
                    ("review", pt(Some(0), 0, far, None)),
                    ("work", pt(Some(0), 0, far, None)),
                    ("team", pt(Some(0), 30, far, None)),
                    ("night", pt(Some(0), 25, far, None)),
                ],
            ),
            entry(
                T,
                &[
                    ("review", pt(Some(5), 10, far, None)),
                    ("work", pt(Some(20), 30, far, None)),
                    ("team", pt(Some(100), 68, far, None)),
                    ("night", pt(Some(100), 85, far, None)),
                ],
            ),
        ],
    );
    assert_eq!(
        capacity_line(&e.root, T + 3 * 60),
        "capacity: headroom 2.07 login-weeks of 4 (52%), usable 2 of 4 logins, runway ~9 h \
         (as of 2026-10-30T16:15:00Z, 3m ago)"
    );
}

#[test]
fn n_is_the_number_of_profiles_in_the_newest_entry() {
    let e = env();
    write_history(
        &e,
        &[
            entry(
                T - HOUR,
                &[
                    ("a", pt(Some(1), 1, None, None)),
                    ("b", pt(Some(1), 1, None, None)),
                ],
            ),
            entry(T, &[("a", pt(Some(1), 50, None, None))]),
        ],
    );
    assert_eq!(
        capacity_line(&e.root, T + 7200),
        "capacity: headroom 0.50 login-weeks of 1 (50%), usable 1 of 1 logins, runway unknown \
         (as of 2026-10-30T16:15:00Z, 2h ago)"
    );
}

#[test]
fn a_newest_entry_with_no_profiles_has_unknown_headroom() {
    let e = env();
    write_history(&e, &[entry(T, &[])]);
    assert_eq!(
        capacity_line(&e.root, T + 45),
        "capacity: headroom unknown (no valid readings), usable 0 of 0 logins, runway unknown \
         (as of 2026-10-30T16:15:00Z, 45s ago)"
    );
}

#[test]
fn the_capacity_line_applies_the_weights_file() {
    let e = env();
    write_history(&e, &[entry(T, &[("work", pt(Some(1), 50, None, None))])]);
    let w = e.root.join(".auto-switch.d/capacity-weights.json");
    fs::write(&w, r#"{"work": 2.0}"#).unwrap();
    fs::set_permissions(&w, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        capacity_line(&e.root, T + DAY + 1),
        "capacity: headroom 0.50 login-weeks of 1 (50%)   [weighted: 1.00 of 2.00], usable 1 of 1 \
         logins, runway unknown (as of 2026-10-30T16:15:00Z, 1d ago)"
    );
}
