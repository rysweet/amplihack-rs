//! Issue #2910 (D8, rules L1 to L7, E17, T1 to T4, T8): a login's last good
//! reading, taken from `usage-history.jsonl`. Wired from `last_good.rs`.
//!
//! When a probe cannot say anything about a login (rate limited, transport
//! failure, identity unavailable, timed out, still running) the rotator uses
//! what that login last showed instead of treating it as full or as unread.
//! It is used for candidates only. The age limit is a check on data, not a
//! timer: one session window, 18 000 seconds.
//!
//! Rules (each has a test below):
//!
//! - L1: a point is considered when `now - 18000 <= at < now`.
//! - L2: the newest point per login wins; on an equal time the later line wins.
//! - L3: when the newest point is unusable there is no last good reading,
//!   even if an older point would have been usable (E17).
//! - L4: a point is unusable when its week has already reset (`w_reset <=
//!   now`), or when its session figure is missing and the login did not show
//!   "no session limit".
//! - L5: a missing `w_reset` is accepted.
//! - L6: a missing session figure with "no session limit" reads as session 0.
//! - L7: the result is `Reading::Measured` with whole-number percentages.
//!
//! API these tests assume (in `last_good.rs`):
//!
//! ```text
//! const LAST_GOOD_MAX_AGE_SECS: i64 = 18_000;
//! struct LastGood { pub reading: Reading, pub at: i64 }   // Debug, Clone, PartialEq
//! last_good(root: &Path, names: &[ProfileName], now: i64)
//!     -> BTreeMap<ProfileName, LastGood>
//! ```
//!
//! The history is written through `usage_history::append_with`, with the
//! credential variable passed as `None`, so no test depends on the
//! environment of the machine it runs on. Every root is a temporary folder.

use super::*;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::decide::Reading;
use crate::commands::profile::rotate::tests_swap::{Env, env, n};
use crate::commands::profile::usage_history::{self, Entry, Point, Writer};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

const NOW: i64 = 1_795_000_000;
const MAX_AGE: i64 = 18_000;

fn state_dir(e: &Env) {
    let d = e.root.join(".auto-switch.d");
    fs::create_dir_all(&d).unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
}

fn point(s: Option<u8>, w: u8, w_reset: Option<i64>) -> Point {
    Point {
        s,
        w,
        w_reset,
        s_reset: None,
        no_session_limit: false,
        passive: false,
    }
}

fn put_many(e: &Env, at: i64, points: &[(&str, Point)]) {
    let entry = Entry {
        at,
        profiles: points
            .iter()
            .map(|(who, p)| (who.to_string(), p.clone()))
            .collect::<BTreeMap<_, _>>(),
        by: Some(Writer::Rotate),
        unknown: Vec::new(),
    };
    usage_history::append_with(&e.root, &entry, NOW, None).unwrap();
}

fn put(e: &Env, at: i64, who: &str, p: Point) {
    put_many(e, at, &[(who, p)]);
}

fn good(week_reset_in: i64) -> Point {
    point(Some(78), 92, Some(NOW + week_reset_in))
}

fn read(e: &Env, who: &[&str], now: i64) -> BTreeMap<ProfileName, LastGood> {
    let names: Vec<ProfileName> = who.iter().map(|w| n(w)).collect();
    last_good(&e.root, &names, now)
}

fn measured(s: f64, w: f64) -> Reading {
    Reading::Measured {
        session: s,
        week: w,
    }
}

// --- The result (L7) ---------------------------------------------------------------

#[test]
fn the_last_good_reading_is_a_measured_reading_in_whole_numbers_with_its_time() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - 1800, "login-al", good(86_400));
    let got = read(&e, &["login-al"], NOW);
    let lg = got.get(&n("login-al")).expect("a last good reading");
    assert_eq!(lg.reading, measured(78.0, 92.0));
    assert_eq!(lg.at, NOW - 1800);
    assert_eq!(got.len(), 1);
}

#[test]
fn the_age_limit_is_one_session_window() {
    assert_eq!(LAST_GOOD_MAX_AGE_SECS, 18_000);
}

// --- L1: age ----------------------------------------------------------------------------

#[test]
fn a_point_exactly_one_window_old_is_used_and_one_second_older_is_not() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - MAX_AGE, "edge", good(86_400));
    put(&e, NOW - MAX_AGE - 1, "stale", good(86_400));
    let got = read(&e, &["edge", "stale"], NOW);
    assert!(got.contains_key(&n("edge")), "{got:?}");
    assert!(!got.contains_key(&n("stale")), "{got:?}");
}

#[test]
fn a_point_from_the_last_second_is_used_and_one_at_or_after_now_is_not() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - 1, "recent", good(86_400));
    put(&e, NOW, "present", good(86_400));
    put(&e, NOW + 60, "future", good(86_400));
    let got = read(&e, &["recent", "present", "future"], NOW);
    assert!(got.contains_key(&n("recent")), "{got:?}");
    assert!(!got.contains_key(&n("present")), "{got:?}");
    assert!(!got.contains_key(&n("future")), "{got:?}");
}

// --- L2: the newest point per login -----------------------------------------------------------

#[test]
fn the_newest_point_of_a_login_wins() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - 3600, "a", point(Some(10), 10, Some(NOW + 86_400)));
    put(&e, NOW - 600, "a", point(Some(20), 30, Some(NOW + 86_400)));
    put(&e, NOW - 1800, "a", point(Some(99), 99, Some(NOW + 86_400)));
    let got = read(&e, &["a"], NOW);
    assert_eq!(got[&n("a")].reading, measured(20.0, 30.0));
    assert_eq!(got[&n("a")].at, NOW - 600);
}

#[test]
fn on_an_equal_time_the_later_line_wins() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - 600, "a", point(Some(11), 11, Some(NOW + 86_400)));
    put(&e, NOW - 600, "a", point(Some(22), 22, Some(NOW + 86_400)));
    assert_eq!(read(&e, &["a"], NOW)[&n("a")].reading, measured(22.0, 22.0));
}

#[test]
fn only_the_logins_asked_for_come_back() {
    let e = env();
    state_dir(&e);
    put_many(
        &e,
        NOW - 600,
        &[
            ("a", good(86_400)),
            ("b", good(86_400)),
            ("c", good(86_400)),
        ],
    );
    let got = read(&e, &["a", "c"], NOW);
    let who: Vec<&str> = got.keys().map(|k| k.as_str()).collect();
    assert_eq!(who, ["a", "c"]);
}

#[test]
fn a_login_with_no_point_has_no_last_good_reading() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - 600, "a", good(86_400));
    assert!(read(&e, &["never-seen"], NOW).is_empty());
    assert!(read(&e, &[], NOW).is_empty());
}

// --- L3, L4: an unusable point ----------------------------------------------------------------

#[test]
fn a_point_whose_week_has_reset_is_not_used() {
    let e = env();
    state_dir(&e);
    // The week reset ten minutes ago: the figure says nothing about this week.
    put(
        &e,
        NOW - 3600,
        "reset",
        point(Some(78), 92, Some(NOW - 600)),
    );
    // A reset at exactly now has happened too.
    put(&e, NOW - 3600, "now", point(Some(78), 92, Some(NOW)));
    // One second ahead has not.
    put(&e, NOW - 3600, "ahead", point(Some(78), 92, Some(NOW + 1)));
    let got = read(&e, &["reset", "now", "ahead"], NOW);
    assert!(!got.contains_key(&n("reset")), "{got:?}");
    assert!(!got.contains_key(&n("now")), "{got:?}");
    assert!(got.contains_key(&n("ahead")), "{got:?}");
}

/// E17: when the newest point is unusable, an older usable one is not a
/// substitute. The newest point is what the login last showed.
#[test]
fn an_unusable_newest_point_hides_an_older_usable_one() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - 7200, "a", good(86_400));
    put(&e, NOW - 600, "a", point(Some(5), 5, Some(NOW - 1)));
    assert!(read(&e, &["a"], NOW).is_empty());
}

#[test]
fn a_point_with_no_session_figure_is_unusable_unless_the_login_has_no_session_limit() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - 600, "unread", point(None, 40, Some(NOW + 86_400)));
    let got = read(&e, &["unread"], NOW);
    assert!(got.is_empty(), "the session was simply not read: {got:?}");
}

// --- L5, L6 -----------------------------------------------------------------------------------------

#[test]
fn a_missing_week_reset_is_accepted() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - 600, "a", point(Some(30), 40, None));
    assert_eq!(read(&e, &["a"], NOW)[&n("a")].reading, measured(30.0, 40.0));
}

#[test]
fn no_session_limit_reads_as_a_session_of_zero() {
    let e = env();
    state_dir(&e);
    let mut p = point(None, 40, Some(NOW + 86_400));
    p.no_session_limit = true;
    put(&e, NOW - 600, "a", p);
    assert_eq!(read(&e, &["a"], NOW)[&n("a")].reading, measured(0.0, 40.0));
}

// --- Reading the history (T8, S6) ----------------------------------------------------------------------

#[test]
fn no_history_means_no_last_good_readings() {
    let e = env();
    // No `.auto-switch.d` at all, and then one with no file.
    assert!(read(&e, &["a"], NOW).is_empty());
    state_dir(&e);
    assert!(read(&e, &["a"], NOW).is_empty());
}

/// T8: a history file that is not private is refused whole, as every reader
/// of the history refuses it. Nothing is substituted from it.
#[test]
fn a_history_file_that_is_not_private_is_refused() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - 600, "a", good(86_400));
    assert_eq!(read(&e, &["a"], NOW).len(), 1, "private: read");
    let file = e.root.join(".auto-switch.d").join(usage_history::FILE);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read(&e, &["a"], NOW).is_empty(), "mode 0644: refused");
}

#[test]
fn reading_the_history_changes_nothing() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - 600, "a", good(86_400));
    let file = e.root.join(".auto-switch.d").join(usage_history::FILE);
    let before = fs::read(&file).unwrap();
    let _ = read(&e, &["a", "b"], NOW);
    let _ = read(&e, &["a", "b"], NOW);
    assert_eq!(fs::read(&file).unwrap(), before);
}

#[test]
fn a_clock_at_either_extreme_does_not_panic() {
    let e = env();
    state_dir(&e);
    put(&e, NOW - 600, "a", good(86_400));
    for now in [i64::MIN, i64::MIN + 1, 0, i64::MAX - 1, i64::MAX] {
        let _ = read(&e, &["a"], now);
    }
}

#[test]
fn a_history_that_is_garbage_gives_no_reading_and_no_panic() {
    let e = env();
    state_dir(&e);
    let file = e.root.join(".auto-switch.d").join(usage_history::FILE);
    fs::write(&file, b"not json\n{\"at\":\n\0\0\0\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(read(&e, &["a"], NOW).is_empty());
}
