//! Issue #2909 with the issue #2910 target rule (#3048): what the pre-swap
//! confirmation does with a target an event cycle took from a kept reading
//! (`Stored::usable`). The cycle, the freshness gate, the swap and the log
//! are the real ones, on the issue #2910 fixture (`tests_issue_2910_fixture`):
//! the measurement and the re-read are stubbed, and nothing reads a login.
//!
//! Contract:
//! - A kept reading has no reading time in the cycle that takes it, so a
//!   target chosen on it is read again before any swap, and the swap stands
//!   on what that read finds.
//! - A kept target whose re-read says nothing (a labelled unknown) is swapped
//!   to `unconfirmed` only when the live login has no room, as for any
//!   reading the confirmation could not refresh ("Rotate confirmation");
//!   while the live login has room it is rejected.
//! - A kept reading is never replaced by a last good reading: it is a reading
//!   the login gave, not a probe that said nothing.

use super::Stored;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::cycle::{Ctx, CycleOutcome, Mode};
use crate::commands::profile::rotate::decide::last_good::LastGood;
use crate::commands::profile::rotate::decide::target_seen::tests_issue_2910_fixture::{
    clock, fleet, name, reporter, run_full, stub, unread,
};
use crate::commands::profile::rotate::decide::target_seen::{Extra, reading_used};
use crate::commands::profile::rotate::decide::{Probed, Reading, Win, Windows};
use crate::commands::profile::rotate::fresh::RereadFn;
use crate::commands::profile::rotate::measure::ProbeLabel;
use crate::commands::profile::rotate::tests_issue_1870::{exh, rd};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

type Calls = Mutex<Vec<String>>;

fn record(calls: &Calls, who: &ProfileName) {
    calls.lock().unwrap().push(who.to_string());
}

fn calls(c: &Calls) -> Vec<String> {
    c.lock().unwrap().clone()
}

/// A direct reading of `session` and `week` whose windows reset an hour
/// after `now`.
fn measured(session: f64, week: f64, now: i64) -> Probed {
    let win = |used| {
        Some(Win {
            used,
            resets_at: Some(now + 3600),
        })
    };
    Probed {
        windows: Windows {
            session: win(session),
            week: win(week),
            no_session_limit: false,
        },
        ..rd(session, week, None)
    }
}

/// `who`'s reading as an event cycle at `now` takes it: kept by the cycle that
/// measured 60 seconds earlier, and still standing for the login.
fn kept_at(who: &str, session: f64, week: f64, now: i64) -> Probed {
    let mut s = Stored::default();
    let read = vec![(name(who), measured(session, week, now))];
    s.keep(None, Some(&(Probed::failed(), read)), now - 60);
    let max_age = i64::try_from(reporter().interval).unwrap();
    s.usable(&name(who), now, max_age)
        .expect("a reading taken within --interval, its windows open, stands")
}

fn kept(who: &str, session: f64, week: f64) -> Probed {
    kept_at(who, session, week, clock())
}

/// One watch cycle with `top` on a kept reading of 0% and 40%, the live
/// login on `live`, and `reread` as the confirmation's read.
fn run_on_kept(live: Probed, reread: RereadFn) -> (CycleOutcome, bool, String) {
    let f = fleet(&["top"], &[]);
    let table = [("top", kept("top", 0.0, 40.0))];
    let out = run_full(
        &f,
        reporter(),
        live,
        &table,
        &[],
        Mode::Watch,
        Some(reread),
        stub,
    );
    (out, f.live_is("top"), f.log())
}

static READ_ROOM: Calls = Mutex::new(Vec::new());

fn reread_room(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
    record(&READ_ROOM, who);
    (rd(10.0, 50.0, None), None)
}

#[test]
fn a_kept_target_is_read_again_and_the_swap_stands_on_the_fresh_reading() {
    let (out, moved, log) = run_on_kept(exh(100.0, 40.0), reread_room);
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(moved, "{log}");
    assert_eq!(calls(&READ_ROOM), ["top"], "read once, before the swap");
    assert!(log.contains("reread: top reading stale"), "{log}");
    assert!(log.contains("target s=10% w=50%"), "{log}");
    assert!(!log.contains("unconfirmed"), "{log}");
}

static READ_FULL: Calls = Mutex::new(Vec::new());

fn reread_full(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
    record(&READ_FULL, who);
    (exh(100.0, 40.0), None)
}

#[test]
fn a_kept_target_that_filled_up_is_rejected_on_its_fresh_reading() {
    let (out, moved, log) = run_on_kept(exh(100.0, 40.0), reread_full);
    assert_ne!(out, CycleOutcome::Swapped, "{log}");
    assert!(!moved, "{log}");
    assert_eq!(calls(&READ_FULL), ["top"]);
    assert!(
        log.contains("reject: top no room on a fresh reading"),
        "{log}"
    );
}

static READ_UNKNOWN_NO_ROOM: Calls = Mutex::new(Vec::new());

fn reread_unknown_no_room(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
    record(&READ_UNKNOWN_NO_ROOM, who);
    (unread(), Some(ProbeLabel::Http429))
}

/// D7 of issue #2910 applies to a kept reading as to any reading the
/// confirmation could not refresh: the login was read first, and the swap
/// line says the move was not confirmed.
#[test]
fn a_kept_target_whose_reread_says_nothing_is_taken_unconfirmed_when_the_live_login_is_full() {
    let (out, moved, log) = run_on_kept(exh(100.0, 40.0), reread_unknown_no_room);
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(moved, "{log}");
    assert_eq!(calls(&READ_UNKNOWN_NO_ROOM), ["top"]);
    assert!(log.contains("unconfirmed (HTTP 429)"), "{log}");
}

static READ_UNKNOWN_ROOM: Calls = Mutex::new(Vec::new());

fn reread_unknown_room(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
    record(&READ_UNKNOWN_ROOM, who);
    (unread(), Some(ProbeLabel::Http429))
}

#[test]
fn a_kept_target_whose_reread_says_nothing_is_rejected_while_the_live_login_has_room() {
    // Past `--session-at 85`, so a move is wanted, but the live login can
    // still run work.
    let (out, moved, log) = run_on_kept(rd(90.0, 40.0, None), reread_unknown_room);
    assert_ne!(out, CycleOutcome::Swapped, "{log}");
    assert!(!moved, "{log}");
    assert_eq!(calls(&READ_UNKNOWN_ROOM), ["top"]);
    assert!(
        log.contains("reject: top reading unknown (HTTP 429)"),
        "{log}"
    );
    assert!(!log.contains("unconfirmed"), "{log}");
}

#[test]
fn a_kept_reading_is_never_replaced_by_a_last_good_reading() {
    let now = 1_793_389_376;
    let p = kept_at("top", 0.0, 40.0, now);
    // Even with a label and a last good reading for the login, which
    // `Extra::for_cycle` never gives a login the cycle did not probe.
    let extra = Extra {
        labels: BTreeMap::from([(name("top"), ProbeLabel::Http429)]),
        last_good: Some(BTreeMap::from([(
            name("top"),
            LastGood {
                reading: Reading::Exhausted {
                    session: Some(100.0),
                    week: Some(40.0),
                },
                at: now - 600,
            },
        )])),
        ..Extra::default()
    };
    assert!(extra.substitute(&name("top"), &p).is_none());
    assert_eq!(reading_used(&extra, &name("top"), &p), p.reading);
}
