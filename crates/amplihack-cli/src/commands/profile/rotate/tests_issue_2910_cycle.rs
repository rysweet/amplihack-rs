//! Issue #2910 in a real cycle: the 22 November day, blocks of every kind,
//! the hold, the ceiling, the 429 storm and the lines a cycle writes.
//! Wired from `target_seen.rs`.
//!
//! The fleet, the stub measurement and the guarded environment are in
//! `tests_issue_2910_fixture`: the real swap, ledger and log under a temporary
//! HOME and profile root, a stubbed probe, a synthetic decision clock, and no
//! `claude`, network or inherited profile root.
//!
//! In these fixtures the live login is `work`; on 22 November it was a login
//! the docs call `rrowan`.

use super::tests_issue_2910_fixture::{
    Fleet, NAMES, STILL_RUNNING, alerts, clock, expired, fleet, lines_with, name, nov22_table,
    reporter, run, run_mode, set_clock, unread,
};
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::rotate::cycle::{CycleOutcome, Mode};
use crate::commands::profile::rotate::decide::Probed;
use crate::commands::profile::rotate::measure::ProbeLabel;
use crate::commands::profile::rotate::settings::Settings;
use crate::commands::profile::rotate::tests_issue_1870::{exh, rd};
use crate::commands::profile::usage_history;
use std::fs;

// --- The 22 November cycle (AC2) --------------------------------------------------------

// `NAMES`, `nov22_table` and `STILL_RUNNING` are in the fixture, shared with
// `tests_issue_2910_deployed`, which runs the same readings at both settings.

fn assert_no_stale_alert(log: &str) {
    assert!(!log.contains("no profile has capacity"), "{log}");
    assert!(alerts(log).is_empty(), "{log}");
}

#[test]
fn ac2_the_0556_cycle_swaps_to_llarame_in_one_organisation() {
    let f = fleet(NAMES, &[]);
    let out = run(
        &f,
        reporter(),
        exh(100.0, 67.0),
        &nov22_table(),
        STILL_RUNNING,
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("llarame"), "{log}");
    assert!(
        log.contains("swap work->llarame live s=100% w=67% target s=0% w=95% reason=plan-room"),
        "{log}"
    );
    assert_no_stale_alert(&log);
}

#[test]
fn ac2_with_every_login_in_the_live_organisation_it_swaps_to_llarame() {
    let mut orgs: Vec<(&str, &str)> = NAMES.iter().map(|who| (*who, "org-a")).collect();
    orgs.push(("work", "org-a"));
    let f = fleet(NAMES, &orgs);
    let out = run(
        &f,
        reporter(),
        exh(100.0, 67.0),
        &nov22_table(),
        STILL_RUNNING,
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("llarame"), "{log}");
    assert!(!log.contains("affinity: crossing"), "{log}");
    assert_no_stale_alert(&log);
}

/// The live login is alone in its organisation and has no room, so nothing in
/// its organisation or unresolved qualifies: a confirmed crossing, with its
/// warning, and never a stay with a login at 100%.
#[test]
fn ac2_a_live_login_alone_in_its_organisation_crosses_to_llarame_with_the_warning() {
    let mut orgs: Vec<(&str, &str)> = NAMES.iter().map(|who| (*who, "org-b")).collect();
    orgs.push(("work", "org-a"));
    let f = fleet(NAMES, &orgs);
    let out = run(
        &f,
        reporter(),
        exh(100.0, 67.0),
        &nov22_table(),
        STILL_RUNNING,
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("llarame"), "{log}");
    assert!(log.contains("affinity: crossing work->llarame"), "{log}");
    assert_no_stale_alert(&log);
    // No identifier of an account or an organisation reaches the log.
    for secret in ["org-a", "org-b", "@example.com", "u-llarame", "u-work"] {
        assert!(!log.contains(secret), "{secret} in the log:\n{log}");
    }
}

/// With qquinn in the live login's organisation and llarame in another,
/// the same-organisation login comes first, whatever its tier.
#[test]
fn ac2_a_same_organisation_login_comes_before_a_better_one_in_another() {
    let f = fleet(
        NAMES,
        &[("work", "org-a"), ("qquinn", "org-a"), ("llarame", "org-b")],
    );
    let out = run(
        &f,
        reporter(),
        exh(100.0, 67.0),
        &nov22_table(),
        STILL_RUNNING,
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("qquinn"), "{log}");
    assert!(!log.contains("affinity: crossing"), "{log}");
    assert_no_stale_alert(&log);
}

// --- 05:48 and the cycles after the swap (AC9) -------------------------------------------

#[test]
fn ac9_the_0548_cycle_stays_with_a_note_and_no_alert() {
    let f = fleet(NAMES, &[]);
    let out = run(
        &f,
        reporter(),
        rd(86.0, 65.0, None),
        &nov22_table(),
        STILL_RUNNING,
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Stayed, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert_eq!(
        lines_with(&log, "stay: live room 14").len(),
        1,
        "one stay note:\n{log}"
    );
    assert!(
        log.contains("stay: live room 14, best other login has room 12, a move needs 17"),
        "{log}"
    );
    assert_no_stale_alert(&log);
    // Advisory: the cycle recorded a stay with capacity, under its own key.
    let l = f.e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.advisory_alert.as_deref(), Some("no-capacity/stay:work"));
    assert_eq!(l.alert, None);
}

#[test]
fn ac9_the_stay_note_is_written_once_per_key() {
    let f = fleet(NAMES, &[]);
    for _ in 0..3 {
        run(
            &f,
            reporter(),
            rd(86.0, 65.0, None),
            &nov22_table(),
            STILL_RUNNING,
        );
    }
    assert_eq!(
        lines_with(&f.log(), "stay: live room 14").len(),
        1,
        "{}",
        f.log()
    );
}

/// One cycle after the swap: the live login is llarame (0/95, room 5). qquinn
/// has room 12 and is past `--session-at`, so inside `--hold` the rotator stays
/// and says why; once the hold has run out it moves.
fn after_the_swap_table() -> Vec<(&'static str, Probed)> {
    nov22_table()
        .into_iter()
        .filter(|(who, _)| *who != "llarame")
        .chain([("rrowan", exh(100.0, 67.0))])
        .collect()
}

fn after_the_swap_fleet() -> Fleet {
    let mut names = NAMES.to_vec();
    names.push("rrowan");
    fleet(&names, &[])
}

#[test]
fn ac9_after_the_swap_the_hold_keeps_the_rotator_on_llarame() {
    let f = after_the_swap_fleet();
    f.set_swapped_at(clock() - 600);
    let out = run(
        &f,
        reporter(),
        rd(0.0, 95.0, None),
        &after_the_swap_table(),
        STILL_RUNNING,
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Stayed, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(
        log.contains(
            "stay: held after last swap (600s of 3600s): qquinn is at s=88%, at or above --session-at"
        ),
        "{log}"
    );
    assert_no_stale_alert(&log);
    let l = f.e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.advisory_alert.as_deref(), Some("no-capacity/held:work"));
}

#[test]
fn ac9_once_the_hold_has_run_out_the_rotator_moves_on() {
    let f = after_the_swap_fleet();
    f.set_swapped_at(clock() - 7200);
    let out = run(
        &f,
        reporter(),
        rd(0.0, 95.0, None),
        &after_the_swap_table(),
        STILL_RUNNING,
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("qquinn"), "{log}");
    assert!(log.contains("reason=plan-room"), "{log}");
}

#[test]
fn ac9_a_ledger_with_no_swap_time_holds_nothing() {
    let f = after_the_swap_fleet();
    // No `swapped_at`: a login changed outside amplihack writes none (a
    // `profile switch` writes one, as `tests_switch` shows).
    let out = run(
        &f,
        reporter(),
        rd(0.0, 95.0, None),
        &after_the_swap_table(),
        STILL_RUNNING,
    );
    assert_eq!(out, CycleOutcome::Swapped, "{}", f.log());
    assert!(f.live_is("qquinn"));
}

#[test]
fn ac9_hold_zero_turns_the_hold_off() {
    let f = after_the_swap_fleet();
    f.set_swapped_at(clock() - 5);
    let s = Settings {
        hold: 0,
        ..reporter()
    };
    let out = run(
        &f,
        s,
        rd(0.0, 95.0, None),
        &after_the_swap_table(),
        STILL_RUNNING,
    );
    assert_eq!(out, CycleOutcome::Swapped, "{}", f.log());
}

// --- S1: no crossing while the live login has room (D10) ------------------------------------

#[test]
fn s1_a_live_login_with_room_never_crosses_and_says_so_with_a_stay_note() {
    let f = fleet(&["third"], &[("work", "org-a"), ("third", "org-b")]);
    // `work` is past `--session-at` but has room 14; `third` has room 80.
    let out = run(
        &f,
        reporter(),
        rd(86.0, 65.0, None),
        &[("third", rd(0.0, 20.0, None))],
        &[],
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Stayed, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(!log.contains("affinity: crossing"), "{log}");
    assert!(log.contains("stay: "), "a stay note: {log}");
    assert!(alerts(&log).is_empty(), "{log}");
}

#[test]
fn s1_the_same_login_is_taken_the_moment_the_live_login_has_no_room() {
    let f = fleet(&["third"], &[("work", "org-a"), ("third", "org-b")]);
    let out = run(
        &f,
        reporter(),
        exh(100.0, 65.0),
        &[("third", rd(0.0, 20.0, None))],
        &[],
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("third"), "{log}");
    assert!(log.contains("affinity: crossing work->third"), "{log}");
}

// --- AC3: the week ceiling in a cycle ----------------------------------------------------------

#[test]
fn ac3_a_login_at_98_percent_of_its_week_is_never_chosen_and_the_summary_counts_it() {
    let f = fleet(&["third"], &[]);
    let out = run(
        &f,
        reporter(),
        exh(100.0, 20.0),
        &[("third", rd(10.0, 98.0, None))],
        &[],
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Alerted, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(
        log.contains(
            "ALERT: no capacity known live=work s=100% w=20% (candidates: 1 above week ceiling 95%)"
        ),
        "{log}"
    );
    assert!(!log.contains("no profile has capacity"), "{log}");
}

#[test]
fn ac3_a_login_at_exactly_95_percent_is_eligible() {
    let f = fleet(&["third"], &[]);
    let out = run(
        &f,
        reporter(),
        exh(100.0, 20.0),
        &[("third", rd(10.0, 95.0, None))],
        &[],
    );
    assert_eq!(out, CycleOutcome::Swapped, "{}", f.log());
    assert!(f.live_is("third"));
}

#[test]
fn ac3_the_ceiling_is_the_setting() {
    let f = fleet(&["third"], &[]);
    let s = Settings {
        target_week_max: 80,
        ..reporter()
    };
    let out = run(
        &f,
        s,
        exh(100.0, 20.0),
        &[("third", rd(10.0, 85.0, None))],
        &[],
    );
    assert_eq!(out, CycleOutcome::Alerted, "{}", f.log());
    assert!(f.log().contains("1 above week ceiling 80%"), "{}", f.log());
}

/// Off an overflow profile onto a plan login: the ceiling applies there too.
#[test]
fn ac3_leaving_an_overflow_login_never_picks_a_week_of_98() {
    let f = fleet(&[], &[]);
    f.mark_overflow("work");
    // `work` is the overflow login, healthy. `spare` is the only plan login.
    let out = run(
        &f,
        reporter(),
        rd(10.0, 10.0, None),
        &[("spare", rd(10.0, 98.0, None))],
        &[],
    );
    assert!(f.live_is("work"), "{out:?}\n{}", f.log());
    assert!(!f.log().contains("reason=overflow-out"), "{}", f.log());
}

#[test]
fn ac3_leaving_an_overflow_login_takes_a_plan_login_at_the_ceiling() {
    let f = fleet(&[], &[]);
    f.mark_overflow("work");
    let out = run(
        &f,
        reporter(),
        rd(10.0, 10.0, None),
        &[("spare", rd(10.0, 95.0, None))],
        &[],
    );
    assert_eq!(out, CycleOutcome::Swapped, "{}", f.log());
    assert!(f.log().contains("reason=overflow-out"), "{}", f.log());
}

// --- AC4: recorded blocks of every kind -------------------------------------------------------------

const KINDS: &[&str] = &["spend_limit", "session_limit", "week_limit"];

fn block_table() -> Vec<(&'static str, Probed)> {
    vec![
        ("llarame", rd(0.0, 40.0, None)),
        ("qquinn", rd(10.0, 50.0, None)),
    ]
}

#[test]
fn ac4_a_login_with_a_block_is_never_chosen_before_its_reset() {
    for kind in KINDS {
        let f = fleet(&["llarame", "qquinn"], &[]);
        f.block("llarame", kind, clock() + 3600);
        let out = run(&f, reporter(), exh(100.0, 40.0), &block_table(), &[]);
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "{kind}: {log}");
        assert!(f.live_is("qquinn"), "{kind}: llarame is blocked:\n{log}");
        assert!(log.contains("swap work->qquinn"), "{kind}: {log}");
    }
}

#[test]
fn ac4_a_login_with_a_block_is_chosen_once_its_reset_has_come() {
    for kind in KINDS {
        let f = fleet(&["llarame", "qquinn"], &[]);
        let until = clock() + 3600;
        f.block("llarame", kind, until);
        // The decision clock reaches the reset exactly: `now >= until`.
        set_clock(until);
        let out = run(&f, reporter(), exh(100.0, 40.0), &block_table(), &[]);
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "{kind}: {log}");
        assert!(f.live_is("llarame"), "{kind}: the block has ended:\n{log}");
    }
}

#[test]
fn ac4_one_second_before_the_reset_the_block_still_holds() {
    let f = fleet(&["llarame", "qquinn"], &[]);
    let until = clock() + 3600;
    f.block("llarame", "week_limit", until);
    set_clock(until - 1);
    run(&f, reporter(), exh(100.0, 40.0), &block_table(), &[]);
    assert!(f.live_is("qquinn"), "{}", f.log());
}

#[test]
fn ac4_a_blocked_live_login_has_no_room_whatever_its_probe_shows() {
    // `work` reads 40% and 40%, but a session limit is recorded for it until
    // later: it cannot run work, so the move needs no margin.
    let f = fleet(&["llarame"], &[]);
    f.block("work", "session_limit", clock() + 3600);
    let out = run(
        &f,
        reporter(),
        rd(40.0, 40.0, None),
        &[("llarame", rd(30.0, 40.0, None))],
        &[],
    );
    // Not an upgrade (the gain is under 30), and not held for margin.
    assert_eq!(out, CycleOutcome::Swapped, "{}", f.log());
    assert!(f.live_is("llarame"), "{}", f.log());
}

#[test]
fn ac4_the_summary_counts_blocked_logins_with_the_earliest_reset() {
    let f = fleet(&["llarame", "qquinn"], &[]);
    let t = clock();
    f.block("llarame", "session_limit", t + 7200);
    f.block("qquinn", "week_limit", t + 3600);
    let out = run(&f, reporter(), exh(100.0, 40.0), &block_table(), &[]);
    let log = f.log();
    assert_eq!(out, CycleOutcome::Alerted, "{log}");
    // The reset is shown as UTC to the minute, of the earliest block.
    let when = chrono::DateTime::from_timestamp(t + 3600, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%MZ")
        .to_string();
    let want = format!(
        "ALERT: no profile has capacity live=work s=100% w=40% (candidates: 2 blocked until {when})"
    );
    assert!(log.contains(&want), "{log}");
}

// --- AC6: no eligible login -------------------------------------------------------------------------

fn all_full() -> Vec<(&'static str, Probed)> {
    vec![
        ("aanderson", exh(0.0, 100.0)),
        ("bbuck", exh(100.0, 10.0)),
        ("ccapstone", exh(100.0, 100.0)),
    ]
}

#[test]
fn ac6_with_no_eligible_login_nothing_moves_and_one_summary_line_is_written() {
    let f = fleet(&["aanderson", "bbuck", "ccapstone"], &[]);
    let out = run(&f, reporter(), exh(100.0, 67.0), &all_full(), &[]);
    let log = f.log();
    assert_eq!(out, CycleOutcome::Alerted, "{log}");
    assert!(f.live_is("work"), "{log}");
    let a = alerts(&log);
    assert_eq!(a.len(), 1, "{log}");
    assert!(
        a[0].ends_with(
            "ALERT: no profile has capacity live=work s=100% w=67% (candidates: 3 exhausted)"
        ),
        "{log}"
    );
    assert!(!log.contains(" swap "), "{log}");
}

#[test]
fn ac6_a_second_identical_cycle_writes_no_second_line() {
    let f = fleet(&["aanderson", "bbuck", "ccapstone"], &[]);
    run(&f, reporter(), exh(100.0, 67.0), &all_full(), &[]);
    let out = run(&f, reporter(), exh(100.0, 67.0), &all_full(), &[]);
    assert_eq!(out, CycleOutcome::Alerted);
    assert_eq!(alerts(&f.log()).len(), 1, "{}", f.log());
}

#[test]
fn ac6_a_change_of_wording_writes_a_new_line_and_a_change_of_counts_does_not() {
    let f = fleet(&["aanderson", "bbuck", "ccapstone"], &[]);
    run(&f, reporter(), exh(100.0, 67.0), &all_full(), &[]);
    // One login is now unknown: "no profile has capacity" can no longer be said.
    let one_unknown = vec![
        ("aanderson", exh(0.0, 100.0)),
        ("bbuck", exh(100.0, 10.0)),
        ("ccapstone", unread()),
    ];
    let labels = [("ccapstone", ProbeLabel::Http429)];
    run(&f, reporter(), exh(100.0, 67.0), &one_unknown, &labels);
    let log = f.log();
    assert_eq!(alerts(&log).len(), 2, "{log}");
    assert!(
        log.contains(
            "ALERT: no capacity known live=work s=100% w=67% (candidates: 1 unknown (HTTP 429), 2 exhausted)"
        ),
        "{log}"
    );
    // The counts change within the same wording: no new line.
    let two_unknown = vec![
        ("aanderson", exh(0.0, 100.0)),
        ("bbuck", unread()),
        ("ccapstone", unread()),
    ];
    let labels = [
        ("bbuck", ProbeLabel::Http429),
        ("ccapstone", ProbeLabel::Http429),
    ];
    run(&f, reporter(), exh(100.0, 67.0), &two_unknown, &labels);
    assert_eq!(alerts(&f.log()).len(), 2, "{}", f.log());
}

#[test]
fn ac6_the_cycle_does_not_end_in_an_error_when_no_login_has_room() {
    // `run` expects an `Ok`; a one-shot cycle (the command) does too.
    let f = fleet(&["aanderson", "bbuck", "ccapstone"], &[]);
    let out = run_mode(
        &f,
        reporter(),
        exh(100.0, 67.0),
        &all_full(),
        &[],
        Mode::OneShot,
    );
    assert_eq!(out, CycleOutcome::Alerted);
}

// --- The 429 storm (AC7) ------------------------------------------------------------------------------

#[test]
fn ac7_a_login_that_answered_429_is_chosen_on_its_last_good_reading_when_the_live_login_is_full() {
    let f = fleet(&["llarame", "qquinn"], &[]);
    f.history("llarame", 1800, 78, 92);
    let out = run(
        &f,
        reporter(),
        exh(100.0, 40.0),
        &[("llarame", unread()), ("qquinn", unread())],
        &[
            ("llarame", ProbeLabel::Http429),
            ("qquinn", ProbeLabel::Http429),
        ],
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("llarame"), "{log}");
    // The probe's own error is in the log, beside the login it was for.
    assert!(
        log.contains("llarame:unknown, HTTP 429, last s=78%,w=92% 30m ago"),
        "{log}"
    );
    assert!(log.contains("qquinn:unknown, HTTP 429"), "{log}");
    assert!(
        log.contains(
            "swap work->llarame live s=100% w=40% target s=78% w=92% last 30m ago reason=plan-room unconfirmed (HTTP 429)"
        ),
        "{log}"
    );
    assert!(alerts(&log).is_empty(), "{log}");
}

#[test]
fn ac7_with_room_on_the_live_login_a_429_login_is_not_a_reason_to_leave_it() {
    let f = fleet(&["llarame"], &[]);
    f.history("llarame", 1800, 78, 92);
    // `work` has room 8 and `llarame` has room 8: a move needs 11.
    let out = run(
        &f,
        reporter(),
        rd(83.0, 92.0, None),
        &[("llarame", unread())],
        &[("llarame", ProbeLabel::Http429)],
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Stayed, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(
        log.contains("stay: live room 8, best other login has room 8, a move needs 11"),
        "{log}"
    );
    assert!(alerts(&log).is_empty(), "{log}");
}

#[test]
fn ac7_with_no_history_every_login_unknown_gives_one_line_naming_the_cause() {
    let names = ["aa", "bb", "cc", "dd", "ee", "ff", "gg", "hh", "ii"];
    let f = fleet(&names, &[]);
    let table: Vec<(&str, Probed)> = names
        .iter()
        .enumerate()
        .map(|(i, who)| (*who, if i < 6 { unread() } else { expired() }))
        .collect();
    let labels: Vec<(&str, ProbeLabel)> = names[..6]
        .iter()
        .map(|who| (*who, ProbeLabel::Http429))
        .collect();
    let out = run(&f, reporter(), exh(100.0, 71.0), &table, &labels);
    let log = f.log();
    assert_eq!(out, CycleOutcome::Alerted, "{log}");
    assert!(f.live_is("work"), "{log}");
    let a = alerts(&log);
    assert_eq!(a.len(), 1, "{log}");
    assert!(
        a[0].ends_with(
            "ALERT: no capacity known live=work s=100% w=71% (candidates: 6 unknown (HTTP 429), 3 failed)"
        ),
        "{log}"
    );
    assert!(!log.contains("no profile has capacity"), "{log}");
}

#[test]
fn ac7_a_failed_login_is_shown_as_failed_and_an_unknown_one_as_unknown() {
    let f = fleet(&["aa", "bb"], &[]);
    f.history("aa", 600, 10, 10);
    f.history("bb", 600, 10, 10);
    run(
        &f,
        reporter(),
        exh(100.0, 71.0),
        &[("aa", expired()), ("bb", unread())],
        &[("bb", ProbeLabel::Transport)],
    );
    let log = f.log();
    // A credential failure gets no last good reading, so it is never a target.
    assert!(log.contains("aa:failed"), "{log}");
    assert!(
        log.contains("bb:unknown, transport, last s=10%,w=10% 10m ago"),
        "{log}"
    );
}

// --- The cycle line gives each login's reason (D11) -----------------------------------------------------

#[test]
fn d11_the_cycle_line_says_why_a_login_is_not_a_target() {
    let f = fleet(&["above", "blocked", "fine"], &[]);
    f.block("blocked", "week_limit", clock() + 3600);
    run(
        &f,
        reporter(),
        exh(100.0, 20.0),
        &[
            ("above", rd(0.0, 98.0, None)),
            ("blocked", rd(0.0, 10.0, None)),
            ("fine", rd(0.0, 10.0, None)),
        ],
        &[],
    );
    let log = f.log();
    assert!(log.contains("above:s=0%,w=98% above ceiling 95%"), "{log}");
    let until = chrono::DateTime::from_timestamp(clock() + 3600, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%MZ")
        .to_string();
    assert!(
        log.contains(&format!(
            "blocked:s=0%,w=10% blocked until {until} (week limit)"
        )),
        "{log}"
    );
    // A login that is a target carries no reason after its figures.
    let fine = log
        .lines()
        .find(|l| l.contains("fine:s=0%,w=10%"))
        .expect("fine is in the cycle line");
    assert!(
        !fine.contains("fine:s=0%,w=10% above") && !fine.contains("fine:s=0%,w=10% blocked"),
        "{fine}"
    );
}

// --- A guess is never written back as a measurement (T7) ----------------------------------------------------

#[test]
fn t7_a_last_good_reading_changes_the_decision_only() {
    let f = fleet(&["llarame"], &[]);
    f.history("llarame", 1800, 78, 92);
    run(
        &f,
        reporter(),
        exh(100.0, 40.0),
        &[("llarame", unread())],
        &[("llarame", ProbeLabel::Http429)],
    );
    // The cycle wrote no new point for llarame: the probe read nothing, and
    // the substituted figure is not a reading. The one point is the seeded one.
    let (entries, _) = usage_history::read_all(&f.e.root, now() + 3600);
    let with_llarame = entries
        .iter()
        .filter(|e| e.profiles.contains_key("llarame"))
        .count();
    assert_eq!(with_llarame, 1, "{entries:?}");
    // The no-room record is not written for it either.
    let l = f.e.rs.load_ledger().unwrap().unwrap();
    let recorded = serde_json::to_string(&l.no_room).unwrap();
    assert!(!recorded.contains("llarame"), "{recorded}");
}

// --- AC10: the dry run takes the same path --------------------------------------------------------------------

#[test]
fn ac10_a_dry_run_on_the_22_nov_readings_would_swap_and_moves_nothing() {
    // The target, llarame, is named on the printed `would swap` line, which QA
    // case 1 checks on the real command; the cycle test above names it for
    // the same decision. Here: the same path decides a swap and writes nothing.
    let f = fleet(NAMES, &[]);
    let before_log = f.log();
    let before_ledger = fs::read(&f.e.rs.ledger).unwrap();
    let out = run_mode(
        &f,
        reporter(),
        exh(100.0, 67.0),
        &nov22_table(),
        STILL_RUNNING,
        Mode::DryRun,
    );
    assert_eq!(
        out,
        CycleOutcome::Swapped,
        "a dry run reports what it would do"
    );
    assert!(f.live_is("work"), "nothing moved");
    assert_eq!(f.log(), before_log, "a dry run writes no log");
    assert_eq!(
        fs::read(&f.e.rs.ledger).unwrap(),
        before_ledger,
        "nor rotate state"
    );
}

#[test]
fn ac10_a_dry_run_on_the_0548_readings_would_stay() {
    let f = fleet(NAMES, &[]);
    let before_log = f.log();
    // The outcome a dry run returns does not depend on the note it prints
    // (`would stay (live room 14, ...)`, checked in QA case 6 on the real
    // command's output); what matters here is that it moves and writes nothing.
    let out = run_mode(
        &f,
        reporter(),
        rd(86.0, 65.0, None),
        &nov22_table(),
        STILL_RUNNING,
        Mode::DryRun,
    );
    assert_eq!(
        out,
        CycleOutcome::Alerted,
        "a login with room 14 does not leave for room 5: a stay note, which a dry run reports as `Alerted`"
    );
    assert!(f.live_is("work"));
    assert_eq!(f.log(), before_log);
}

// --- The organisation check after authorise (S4) ------------------------------------------------------------------

/// The pure check is tested in `tests_issue_2910_pool`. This guards its
/// wiring: a source file other than the pool's own calls it, so a refusal of
/// an unallowed crossing happens at the swap, after authorisation, and not
/// only in the decision.
#[test]
fn s4_the_swap_step_calls_the_organisation_check() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/rotate");
    let mut callers = Vec::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        if !file.ends_with(".rs") || file.starts_with("tests_") || file == "affinity_pool.rs" {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        if text.lines().any(|l| {
            let code = l.split("//").next().unwrap_or_default();
            code.contains("org_check(")
        }) {
            callers.push(file);
        }
    }
    assert!(!callers.is_empty(), "no rotate source calls org_check");
}

/// S4 end to end, on the real swap: after authorisation has read both logins,
/// a confirmed crossing the decision did not allow (`may_cross` false) is
/// refused with the fixed text and moves nothing; the same crossing goes ahead
/// when the decision allowed it; a login whose organisation is not recorded is
/// not a crossing; and a hand switch (`None`) is not checked at all.
#[test]
fn s4_the_swap_refuses_a_crossing_the_decision_did_not_allow() {
    use crate::commands::profile::ProfileError;
    use crate::commands::profile::auto_switch::kit::FakeExchange;
    use crate::commands::profile::rotate::login_identity::{Budget, Reader, default_read};
    use crate::commands::profile::rotate::swap::{Source, swap_with};
    use crate::commands::profile::rotate::swap_authorize::CROSS_REFUSED;
    use crate::commands::profile::rotate::tests_swap::ino_at;
    // The exchange the test holds, as issue #2935 has every swap test do, so
    // S4 is checked on every system and not only where `renameat2` exists.
    let attempt = |orgs: &[(&str, &str)], may_cross: Option<bool>| {
        let f = fleet(&[], orgs);
        let spare = ino_at(&f.e.slot("spare"));
        let (read, budget) = (default_read(), Budget::new());
        let got = swap_with(
            &f.e.store,
            &f.e.rs,
            &f.e.home,
            Source::Profile(name("work")),
            &name("spare"),
            now(),
            Reader {
                read: &read,
                budget: &budget,
            },
            None,
            may_cross,
            FakeExchange::working().as_ref(),
        );
        (f, spare, got)
    };
    let crossing = [("work", "org-a"), ("spare", "org-b")];

    let (f, _, got) = attempt(&crossing, Some(false));
    match got {
        Err(ProfileError::RotateRefused(why)) => assert!(why.contains(CROSS_REFUSED), "{why}"),
        other => panic!("a crossing the decision did not allow must be refused: {other:?}"),
    }
    assert!(f.live_is("work"), "a refused swap moves no login");

    for (orgs, may_cross, what) in [
        (&crossing[..], Some(true), "a crossing the decision allowed"),
        (&crossing[..], None, "a hand switch"),
        (&crossing[..1], Some(false), "an organisation not recorded"),
    ] {
        let (f, spare, got) = attempt(orgs, may_cross);
        assert!(got.is_ok(), "{what} goes ahead: {got:?}");
        assert_eq!(ino_at(&f.e.live()), spare, "{what} makes spare live");
    }
}

// --- The counts in the summary (D9) ---------------------------------------------------------------------

/// One part per label, in the order `HTTP 429`, `transport`, `identity
/// unavailable`, `timeout`, `still running`, `probe error`, whatever order the
/// logins were probed in, and no profile name anywhere in the line.
#[test]
fn d9_the_unknown_parts_come_one_per_label_in_the_fixed_order_with_no_names() {
    let names = ["quokka", "wombat", "numbat", "dingo", "emu", "kiwi"];
    let f = fleet(&names, &[]);
    let table: Vec<(&str, Probed)> = names.iter().map(|who| (*who, unread())).collect();
    let labels = [
        ("quokka", ProbeLabel::Timeout),
        ("wombat", ProbeLabel::Http429),
        ("numbat", ProbeLabel::Http429),
        ("dingo", ProbeLabel::Transport),
        ("emu", ProbeLabel::StillRunning),
        ("kiwi", ProbeLabel::ProbeError),
    ];
    run(&f, reporter(), exh(100.0, 71.0), &table, &labels);
    let log = f.log();
    let a = alerts(&log);
    assert_eq!(a.len(), 1, "{log}");
    assert!(
        a[0].ends_with(
            "ALERT: no capacity known live=work s=100% w=71% (candidates: 2 unknown (HTTP 429), 1 unknown (transport), 1 unknown (timeout), 1 unknown (still running), 1 unknown (probe error))"
        ),
        "{log}"
    );
    for who in names {
        assert!(
            !a[0].contains(who),
            "{who} is named in the summary: {}",
            a[0]
        );
    }
}

/// A figure that is not a finite number of zero or more is no reading: the
/// login counts as failed, and never ranks.
#[test]
fn d9_a_reading_that_is_not_a_number_counts_as_failed() {
    for (s, w) in [
        (f64::NAN, 10.0),
        (10.0, f64::NAN),
        (-5.0, 10.0),
        (f64::INFINITY, 10.0),
    ] {
        let f = fleet(&["odd"], &[]);
        let out = run(
            &f,
            reporter(),
            exh(100.0, 40.0),
            &[("odd", rd(s, w, None))],
            &[],
        );
        let log = f.log();
        assert_eq!(out, CycleOutcome::Alerted, "{s}/{w}: {log}");
        assert!(f.live_is("work"), "{s}/{w}: {log}");
        assert!(
            log.contains("ALERT: no capacity known live=work s=100% w=40% (candidates: 1 failed)"),
            "{s}/{w}: {log}"
        );
    }
}

#[test]
fn d9_a_reading_of_100_or_more_counts_as_exhausted_and_never_ranks() {
    let f = fleet(&["huge"], &[]);
    let out = run(
        &f,
        reporter(),
        exh(100.0, 40.0),
        &[("huge", rd(1e9, 10.0, None))],
        &[],
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Alerted, "{log}");
    assert!(
        log.contains(
            "ALERT: no profile has capacity live=work s=100% w=40% (candidates: 1 exhausted)"
        ),
        "{log}"
    );
}

/// The wording is the one the cycle can stand behind: a login the rotator has
/// not seen full keeps it from saying that no login has capacity.
#[test]
fn d9_no_profile_has_capacity_is_never_written_while_a_login_is_unknown_or_failed() {
    for unknown_or_failed in [unread(), expired()] {
        let f = fleet(&["aa", "bb"], &[]);
        run(
            &f,
            reporter(),
            exh(100.0, 40.0),
            &[("aa", exh(0.0, 100.0)), ("bb", unknown_or_failed)],
            &[],
        );
        assert!(!f.log().contains("no profile has capacity"), "{}", f.log());
        assert!(f.log().contains("no capacity known"), "{}", f.log());
    }
}

/// Issue #2910, the deputy's third point: the line never says `no profile has
/// capacity` when that was not shown. A plan login whose directory fails the
/// ownership and mode check (issue #1698) is never probed, so its room was
/// never read; the rotator already holds a move onto overflow for exactly that
/// reason. The summary must not claim every login is known to have no room.
#[test]
fn ac6_a_plan_login_that_was_never_read_is_not_a_login_known_to_have_no_room() {
    let f = fleet(&["aanderson", "bbuck", "ccapstone"], &[]);
    // `ccapstone`'s directory is group- and world-writable: the scan refuses
    // it and it is skipped, so it carries no reading at all.
    use std::os::unix::fs::PermissionsExt;
    let dir = f.e.root.join("ccapstone");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    let readings = vec![("aanderson", exh(0.0, 100.0)), ("bbuck", exh(100.0, 10.0))];
    let out = run(&f, reporter(), exh(100.0, 67.0), &readings, &[]);
    let log = f.log();
    assert_eq!(out, CycleOutcome::Alerted, "{log}");
    assert!(
        !log.contains("no profile has capacity"),
        "ccapstone's room was never read, so no login is known to have no room: {log}"
    );
    assert!(
        log.contains(
            "ALERT: no capacity known live=work s=100% w=67% \
             (candidates: 1 not read (directory refused), 2 exhausted)"
        ),
        "the count says how many were not read and why: {log}"
    );
    // The cycle says the same thing twice: the skip line and the count agree.
    assert!(log.contains("skip: ccapstone (directory refused)"), "{log}");
}
