//! Issue #2910 at the thresholds the hosts run, `--session-at 95 --week-at
//! 98`, beside the reporter's 85/90; the first cycle of a rotator restarted
//! onto this release while runs wait on a usage limit; and the organisation
//! check after authorisation, when it refuses. Wired from `target_seen.rs`.
//! These answer the first review round of PR #3048.
//!
//! The fixture is `tests_issue_2910_fixture`, as for `tests_issue_2910_cycle`:
//! the real swap, ledger and log under a temporary HOME and profile root, a
//! stubbed probe, a synthetic decision clock, and no `claude`, network or
//! inherited profile root. Every fixture run builds a new cycle context from
//! the ledger and the settings alone, as `daemon::watch` does when a rotator
//! starts, so each run is also the first cycle after a restart.
//!
//! Whether a waiting run wakes is read as the usage-limit wait reads it: the
//! ledger's bytes go through `amplihack_utils::rotator_status::parse_cycle`,
//! and the run's limit hit at `start` wakes on `fresh_room(start)` (the
//! rotator-capacity exit) and on a login change only when `has_capacity() &&
//! says_room()` (`limit_wait::change_may_end`).

use super::tests_issue_2910_fixture::{
    BOTH, NAMES, STILL_RUNNING, alerts, clock, fleet, lines_with, name, nov22_table, reporter,
    reporter_deployed, run, run_tuned, set_clock,
};
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::rotate::alert::{NO_CAPACITY, alert_key};
use crate::commands::profile::rotate::cycle::{Ctx, CycleOutcome, Mode};
use crate::commands::profile::rotate::decide::Probed;
use crate::commands::profile::rotate::identify::Account;
use crate::commands::profile::rotate::ledger::LastCycle;
use crate::commands::profile::rotate::tests::tests_fixture_identity;
use crate::commands::profile::rotate::tests_issue_1870::{exh, rd};
use crate::commands::profile::rotate::tests_swap::{CREDS, Env, login, private};
use amplihack_utils::rotator_status::{CycleStatus, parse_cycle};
use std::fs;
use std::path::Path;

/// The login each setting moves to from the 05:56Z readings, and the tail of
/// its swap line. At 85/90 llarame (session 0%) is the only login in the
/// first tier. At 95/98 every login with a session below 95% is in the first
/// tier, and qquinn has the most room now (12, as yyandis, with the lower
/// week); both of its windows are below the deployed thresholds, so the swap
/// is a threshold swap.
fn target_0556(label: &str) -> (&'static str, &'static str) {
    match label {
        "85/90" => ("llarame", "target s=0% w=95% reason=plan-room"),
        _ => ("qquinn", "target s=88% w=20% reason=threshold"),
    }
}

fn assert_no_alert(label: &str, log: &str) {
    assert!(!log.contains("no profile has capacity"), "{label}: {log}");
    assert!(alerts(log).is_empty(), "{label}: {log}");
}

// --- The 22 November readings at both settings -----------------------------------------

#[test]
fn the_0556_cycle_moves_to_a_login_below_100_percent_on_both_windows_at_both_settings() {
    for (label, s) in BOTH {
        let f = fleet(NAMES, &[]);
        let out = run(&f, s(), exh(100.0, 67.0), &nov22_table(), STILL_RUNNING);
        let log = f.log();
        let (who, tail) = target_0556(label);
        assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is(who), "{label}: {log}");
        assert!(
            log.contains(&format!("swap work->{who} live s=100% w=67% {tail}")),
            "{label}: {log}"
        );
        assert_no_alert(label, &log);
    }
}

#[test]
fn a_dry_run_on_the_0556_readings_names_the_same_login_at_both_settings() {
    for (label, s) in BOTH {
        let f = fleet(NAMES, &[]);
        let before = f.log();
        let out = run_tuned(
            &f,
            s(),
            exh(100.0, 67.0),
            &nov22_table(),
            STILL_RUNNING,
            Mode::DryRun,
            &|_| {},
        );
        assert_eq!(out, CycleOutcome::Swapped, "{label}");
        assert!(f.live_is("work"), "{label}: a dry run moves nothing");
        assert_eq!(f.log(), before, "{label}: a dry run writes no log");
    }
}

/// yyandis (88/71) is the only login in the live login's organisation: tier
/// 2 at 85/90 and tier 1 at 95/98, room 12 at both. Every login that ranks
/// above it is in another organisation, and the live login has no room, so a
/// crossing is open; the same-organisation login still comes first.
#[test]
fn a_same_organisation_login_comes_before_a_better_one_in_another_at_both_settings() {
    for (_, s) in BOTH {
        let mut orgs: Vec<(&str, &str)> = NAMES.iter().map(|who| (*who, "org-b")).collect();
        orgs.retain(|(who, _)| *who != "yyandis");
        orgs.extend([("work", "org-a"), ("yyandis", "org-a")]);
        let f = fleet(NAMES, &orgs);
        let out = run(&f, s(), exh(100.0, 67.0), &nov22_table(), STILL_RUNNING);
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "{log}");
        assert!(f.live_is("yyandis"), "{log}");
        assert!(!log.contains("affinity: crossing"), "{log}");
    }
}

#[test]
fn a_live_login_alone_in_its_organisation_crosses_with_the_warning_at_both_settings() {
    for (label, s) in BOTH {
        let mut orgs: Vec<(&str, &str)> = NAMES.iter().map(|who| (*who, "org-b")).collect();
        orgs.push(("work", "org-a"));
        let f = fleet(NAMES, &orgs);
        let out = run(&f, s(), exh(100.0, 67.0), &nov22_table(), STILL_RUNNING);
        let log = f.log();
        let (who, _) = target_0556(label);
        assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is(who), "{label}: {log}");
        assert!(
            log.contains(&format!("affinity: crossing work->{who}")),
            "{label}: {log}"
        );
        assert_no_alert(label, &log);
    }
}

/// 05:48Z, live 86/65 (room 14). At 85/90 the live login is past
/// `--session-at`, no login leaves it 3 more points, and it stays with a note.
/// At 95/98 it is below both thresholds: nothing needs to move, an upgrade
/// would need room 44, and the stay is a plain one, with no note and no
/// advisory key. Neither setting raises an ALERT.
#[test]
fn the_0548_cycle_stays_with_no_alert_at_both_settings() {
    for (label, s) in BOTH {
        let f = fleet(NAMES, &[]);
        let out = run(&f, s(), rd(86.0, 65.0, None), &nov22_table(), STILL_RUNNING);
        let log = f.log();
        assert_eq!(out, CycleOutcome::Stayed, "{label}: {log}");
        assert!(f.live_is("work"), "{label}: {log}");
        assert_no_alert(label, &log);
        let l = f.e.rs.load_ledger().unwrap().unwrap();
        assert_eq!(l.alert, None, "{label}");
        let v = wait_view(&f.e);
        if label == "85/90" {
            assert!(
                log.contains("stay: live room 14, best other login has room 12, a move needs 17"),
                "{log}"
            );
            assert_eq!(l.advisory_alert.as_deref(), Some("no-capacity/stay:work"));
            assert_eq!(v.live_room, Some(false), "86% is past --session-at 85");
        } else {
            assert!(lines_with(&log, "stay:").is_empty(), "no note: {log}");
            assert_eq!(l.advisory_alert, None);
            assert_eq!(v.live_room, Some(true), "86% is below --session-at 95");
        }
    }
}

/// At 95/98, as the live session rises through the morning: below 95% it
/// stays; at 95% (room 5) and above, it moves to qquinn, the login with the
/// most room now, at once. qquinn's session is below `--session-at`, so the
/// hold after the last swap does not apply, and no step raises an ALERT.
#[test]
fn at_the_deployed_settings_the_rising_live_session_moves_to_qquinn_without_an_alert() {
    let steps = [
        (rd(91.0, 66.0, None), None),
        (
            rd(95.0, 66.0, None),
            Some("live s=95% w=66% target s=88% w=20%"),
        ),
        (
            rd(97.0, 66.0, None),
            Some("live s=97% w=66% target s=88% w=20%"),
        ),
        (
            exh(100.0, 67.0),
            Some("live s=100% w=67% target s=88% w=20%"),
        ),
    ];
    for (live, swap) in steps {
        let f = fleet(NAMES, &[]);
        f.set_swapped_at(clock() - 600);
        let out = run(
            &f,
            reporter_deployed(),
            live.clone(),
            &nov22_table(),
            STILL_RUNNING,
        );
        let log = f.log();
        match swap {
            None => {
                assert_eq!(out, CycleOutcome::Stayed, "{live:?}: {log}");
                assert!(lines_with(&log, "stay:").is_empty(), "{log}");
            }
            Some(tail) => {
                assert_eq!(out, CycleOutcome::Swapped, "{live:?}: {log}");
                assert!(f.live_is("qquinn"), "{log}");
                assert!(log.contains(&format!("swap work->qquinn {tail}")), "{log}");
            }
        }
        assert_no_alert("95/98", &log);
    }
}

/// One cycle after the swap to qquinn at 95/98: qquinn (88/20, room 12) is
/// below both thresholds, so nothing needs to move and llarame (room 5) is no
/// upgrade. The rotator stays, with no note and no hold note.
#[test]
fn at_the_deployed_settings_the_cycle_after_the_swap_stays_on_qquinn() {
    let mut names = NAMES.to_vec();
    names.push("rrowan");
    let f = fleet(&names, &[]);
    f.set_swapped_at(clock() - 60);
    let table: Vec<(&str, Probed)> = nov22_table()
        .into_iter()
        .filter(|(who, _)| *who != "qquinn")
        .chain([("rrowan", exh(100.0, 67.0))])
        .collect();
    let out = run(
        &f,
        reporter_deployed(),
        rd(88.0, 20.0, None),
        &table,
        STILL_RUNNING,
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Stayed, "{log}");
    assert!(lines_with(&log, "stay:").is_empty(), "{log}");
    assert_no_alert("95/98", &log);
    assert_eq!(wait_view(&f.e).live_room, Some(true));
}

// --- A restart onto this release with runs in usage-limit waits --------------------------

/// The ledger as the usage-limit wait reads it.
fn wait_view(e: &Env) -> CycleStatus {
    let raw = fs::read(&e.rs.ledger).expect("the cycle wrote a ledger");
    parse_cycle(&raw, now() + 1).expect("the wait parses the ledger")
}

/// When the waiting runs met their limits: three hours, ten minutes and one
/// second before the decision clock.
fn limit_hits() -> [i64; 3] {
    [clock() - 3 * 3600, clock() - 600, clock() - 1]
}

/// Every run waiting since one of `hits` wakes: on the rotator's capacity,
/// and on the login change the cycle made.
fn assert_waits_wake(what: &str, v: &CycleStatus, hits: [i64; 3]) {
    for start in hits {
        assert!(
            v.fresh_room(start),
            "{what}: a run waiting since {start}: {v:?}"
        );
    }
    assert!(v.has_capacity() && v.says_room(), "{what}: {v:?}");
}

/// No run waiting since one of `hits` wakes, on either ground.
fn assert_waits_held(what: &str, v: &CycleStatus, hits: [i64; 3]) {
    for start in hits {
        assert!(
            !v.fresh_room(start),
            "{what}: a run waiting since {start}: {v:?}"
        );
    }
    assert!(!(v.has_capacity() && v.says_room()), "{what}: {v:?}");
}

/// The ledger the previous release left at 05:56Z on 22 November: the hard
/// no-capacity ALERT key, a last cycle `alerted` with no room on the live
/// login, and a swap ten minutes earlier.
fn previous_release_ledger(e: &Env, outcome: &str) {
    let mut l = e.rs.load_ledger().unwrap().unwrap();
    if outcome == "alerted" {
        l.alert = Some(alert_key(NO_CAPACITY, Some(&name("work"))));
    }
    l.last_cycle = Some(LastCycle {
        at: clock() - 60,
        measured_at: Some(clock() - 61),
        outcome: outcome.into(),
        overflow: None,
        live_room: Some(false),
    });
    l.swapped_at = Some(clock() - 600);
    e.rs.save_ledger(&l).unwrap();
}

/// The rotator is restarted onto this release at 05:56Z with runs held in
/// limit waits. Its first cycle clears the old ALERT, moves to a login below
/// `--session-at` on its session and below 100% on its week, records room,
/// and so wakes every waiting run, at both settings. The swap ten minutes
/// earlier holds nothing, because the live login has no room.
#[test]
fn the_first_cycle_after_a_restart_moves_to_a_login_with_room_and_wakes_the_waits() {
    for (label, s) in BOTH {
        let f = fleet(NAMES, &[]);
        let hits = limit_hits();
        previous_release_ledger(&f.e, "alerted");
        assert_waits_held(
            &format!("{label}, before the restart"),
            &wait_view(&f.e),
            hits,
        );

        let out = run(&f, s(), exh(100.0, 67.0), &nov22_table(), STILL_RUNNING);
        let log = f.log();
        let (who, _) = target_0556(label);
        assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is(who), "{label}: {log}");
        let v = wait_view(&f.e);
        assert_eq!(v.outcome.as_deref(), Some("swapped"), "{label}");
        assert!(!v.alert, "{label}: the old ALERT is cleared: {v:?}");
        assert_eq!(v.live_room, Some(true), "{label}: {v:?}");
        assert_waits_wake(label, &v, hits);
    }
}

/// The restart finds the live login past `--session-at` with room 4 (96/40),
/// after a stay that recorded no room, so the waits are held. The first cycle
/// moves to qquinn (room 12) at both settings, because 12 is at least 4 + 3.
/// At 95/98 qquinn's session is below `--session-at`, so the cycle records
/// room and every wait wakes. At 85/90 it is not (88% against 85%), so the
/// cycle records no room and the waits stay held: flag 8 of the PR, #3067.
#[test]
fn a_restart_past_session_at_moves_and_wakes_the_waits_only_onto_a_login_below_it() {
    for (label, s) in BOTH {
        let f = fleet(NAMES, &[]);
        let hits = limit_hits();
        previous_release_ledger(&f.e, "stayed");
        f.set_swapped_at(clock() - 7200);
        assert_waits_held(
            &format!("{label}, before the restart"),
            &wait_view(&f.e),
            hits,
        );

        let out = run(&f, s(), rd(96.0, 40.0, None), &nov22_table(), STILL_RUNNING);
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is("qquinn"), "{label}: {log}");
        let v = wait_view(&f.e);
        if label == "95/98" {
            assert_eq!(v.live_room, Some(true), "{v:?}");
            assert_waits_wake(label, &v, hits);
        } else {
            assert_eq!(v.live_room, Some(false), "{v:?}");
            assert_waits_held(label, &v, hits);
        }
    }
}

/// Flag 8 of the PR (#3067), end to end. The only login with room is at 96%
/// of its session, above `--session-at` at both settings. The live login is
/// full, so the rotator moves there, records no room, and the waits stay
/// held: a run woken there would meet its limit at once, which is what
/// #2857 stopped. Once that login's session window resets, the next cycle
/// reads it below `--session-at`, records room, and every wait wakes.
#[test]
fn after_a_move_onto_a_login_past_session_at_the_waits_wake_on_the_first_cycle_with_room() {
    for (label, s) in BOTH {
        let f = fleet(&["eelsworth", "ppatton"], &[]);
        // The move is two hours ago, so the ledger's times stay in the past
        // when the session reset comes; the wait reads no time in the future.
        set_clock(clock() - 7200);
        let hits = limit_hits();
        previous_release_ledger(&f.e, "alerted");
        let table = [
            ("eelsworth", rd(96.0, 82.0, None)),
            ("ppatton", exh(100.0, 99.0)),
        ];
        let out = run(&f, s(), exh(100.0, 67.0), &table, &[]);
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is("eelsworth"), "{label}: {log}");
        let v = wait_view(&f.e);
        assert_eq!(v.live_room, Some(false), "{label}: {v:?}");
        assert_waits_held(&format!("{label}, after the move"), &v, hits);

        // Two hours later, now, eelsworth's session window has reset.
        set_clock(now());
        let table = [("work", exh(100.0, 67.0)), ("ppatton", exh(100.0, 99.0))];
        let out = run(&f, s(), rd(4.0, 82.0, None), &table, &[]);
        let log = f.log();
        assert_eq!(out, CycleOutcome::Stayed, "{label}: {log}");
        let v = wait_view(&f.e);
        assert_eq!(v.live_room, Some(true), "{label}: {v:?}");
        assert_waits_wake(&format!("{label}, after the reset"), &v, hits);
    }
}

// --- The organisation check after authorisation, when it refuses (S4) --------------------

/// `alpha`'s login reads in organisation `org-a` until its identity has been
/// read `alpha-arm-at` times this test, and in `org-b` from then on. The
/// account carries an e-mail and no uuid, so `check_snapshot` and the
/// comparison of the two authorisation reads, which compare such accounts by
/// e-mail alone, do not see the change. Every other login reads as the
/// fixture seeded it.
fn alpha_changes_organisation(dir: &Path) -> Result<Account, &'static str> {
    let account = tests_fixture_identity::read(dir)?;
    if !dir.ends_with("alpha") {
        return Ok(account);
    }
    let base = dir.parent().unwrap().parent().unwrap();
    let number = |file: &str| {
        fs::read_to_string(base.join(file))
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
    };
    let count = number("alpha-reads").unwrap_or(0) + 1;
    private(&base.join("alpha-reads"), &count.to_string());
    if count < number("alpha-arm-at").unwrap_or(u32::MAX) {
        return Ok(account);
    }
    Account::from_block(&serde_json::json!({
        "emailAddress": "alpha@example.com",
        "organizationUuid": "org-b"
    }))
    .ok_or("fixture account")
}

/// `alpha` as an account with an e-mail and no uuid, in `org-a`.
fn email_only_in_org_a(e: &Env) {
    private(
        &e.root.join("alpha").join(".claude.json"),
        &serde_json::json!({"oauthAccount": {
            "emailAddress": "alpha@example.com",
            "organizationUuid": "org-a"
        }})
        .to_string(),
    );
    login(&e.root.join("alpha").join(CREDS));
}

/// The live login (90/40, room 10) is past `--session-at` and still has
/// room, so no crossing is open. The decision chooses alpha (room 80) in the
/// live login's organisation, and alpha's login then turns out to be in
/// another organisation. In a cycle that swaps, alpha's identity is read
/// three times before revalidation, once by revalidation's snapshot (read
/// 4), then once by each of the two authorisation reads (reads 5 and 6).
/// Wherever the change is first seen, nothing moves in that cycle. Beta
/// (room 70) needs its own direct confirmation in the next cycle, when the
/// snapshot puts alpha in another organisation. No journal remains after
/// either revalidation or authorisation rejects alpha.
#[test]
fn a_target_that_changes_organisation_leaves_the_next_login_for_another_cycle() {
    let table = [
        ("alpha", rd(10.0, 20.0, None)),
        ("beta", rd(30.0, 30.0, None)),
    ];
    let tune = |ctx: &mut Ctx| ctx.identity = alpha_changes_organisation;
    for (arm, refused) in [("4", false), ("5", true), ("6", true)] {
        let f = fleet(
            &["alpha", "beta"],
            &[("work", "org-a"), ("alpha", "org-a"), ("beta", "org-a")],
        );
        email_only_in_org_a(&f.e);
        private(&f.e.root.parent().unwrap().join("alpha-arm-at"), arm);
        let cycle = || {
            run_tuned(
                &f,
                reporter(),
                rd(90.0, 40.0, None),
                &table,
                &[],
                Mode::Watch,
                &tune,
            )
        };

        let out = cycle();
        let log = f.log();
        assert!(!f.live_is("alpha"), "arm {arm}: {log}");
        assert!(!f.e.rs.journal.exists(), "arm {arm}: no journal is left");
        let expected = if refused {
            CycleOutcome::Alerted
        } else {
            CycleOutcome::Skipped
        };
        assert_eq!(out, expected, "arm {arm}: {log}");
        assert!(f.live_is("work"), "arm {arm}: {log}");

        set_clock(clock() + 60);
        let out = cycle();
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "arm {arm}: {log}");
        assert!(f.live_is("beta"), "arm {arm}: {log}");
        assert!(!f.e.rs.journal.exists(), "arm {arm}: no journal is left");
    }
}
