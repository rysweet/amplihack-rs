//! Issue #3368 in a real cycle: a live login whose week use is at or above
//! `--week-at` counts as having no room for the crossing test, so the cycle
//! may cross to a login in another organisation that qualifies. The session
//! threshold alone does not let it cross, and a login in its own
//! organisation still comes first. Wired from `target_seen.rs`.
//!
//! The fleet, the stub measurement and the guarded environment are in
//! `tests_issue_2910_fixture`: the real swap, ledger and log under a temporary
//! HOME and profile root. `work` and `second` are in `org-a`; `third` is in
//! `org-b`. The readings follow the settings, so the tests read `--week-at`
//! and `--session-at` and not fixed numbers.

use super::tests_issue_2910_fixture::{fleet, reporter, run};
use crate::commands::profile::rotate::cycle::CycleOutcome;
use crate::commands::profile::rotate::tests_issue_1870::rd;

/// The room the other logins have in every case: well past the margin over
/// the live login's room.
fn roomy() -> crate::commands::profile::rotate::decide::Probed {
    rd(0.0, 20.0, None)
}

#[test]
fn a_live_login_at_week_at_crosses_when_only_another_organisation_has_room() {
    let s = reporter();
    let week = f64::from(s.week_at);
    let f = fleet(&["third"], &[("work", "org-a"), ("third", "org-b")]);
    let out = run(&f, s, rd(10.0, week, None), &[("third", roomy())], &[]);
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("third"), "{log}");
    assert!(log.contains("affinity:"), "{log}");
}

#[test]
fn a_live_login_one_point_below_week_at_stays_when_only_another_organisation_has_room() {
    let s = reporter();
    let week = f64::from(s.week_at) - 1.0;
    let f = fleet(&["third"], &[("work", "org-a"), ("third", "org-b")]);
    let out = run(&f, s, rd(10.0, week, None), &[("third", roomy())], &[]);
    let log = f.log();
    assert_eq!(out, CycleOutcome::Stayed, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(!log.contains("affinity:"), "{log}");
}

#[test]
fn a_live_login_at_session_at_with_its_week_below_week_at_still_stays() {
    let s = reporter();
    let session = f64::from(s.session_at);
    let week = f64::from(s.week_at) - 30.0;
    let f = fleet(&["third"], &[("work", "org-a"), ("third", "org-b")]);
    let out = run(&f, s, rd(session, week, None), &[("third", roomy())], &[]);
    let log = f.log();
    assert_eq!(out, CycleOutcome::Stayed, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(!log.contains("affinity:"), "{log}");
}

#[test]
fn a_live_login_at_week_at_moves_within_its_organisation_when_it_has_room_there() {
    let s = reporter();
    let week = f64::from(s.week_at);
    let f = fleet(
        &["second", "third"],
        &[("work", "org-a"), ("second", "org-a"), ("third", "org-b")],
    );
    let out = run(
        &f,
        s,
        rd(10.0, week, None),
        &[("second", roomy()), ("third", roomy())],
        &[],
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("second"), "{log}");
    assert!(!log.contains("affinity:"), "{log}");
}
