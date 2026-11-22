//! Issue #2910: the rotator swaps only to a login with room now, seen from
//! outside. Each test runs one real `amplihack profile rotate` cycle in a
//! temporary HOME, with the stand-in `claude` of the `profile rotate`
//! fixture answering `/usage`, and reads the result where an operator
//! would: which login is in `~/.claude` afterwards, and the rotate log.
//!
//! The confirming read before a swap (rule 3) is not driven from here: it
//! is taken only for a target whose reading was stored by an earlier cycle,
//! which a single cycle cannot arrange. `rotate/tests_issue_2910_confirm.rs`
//! covers it with stand-in readings.

#![cfg(unix)]

use crate::support::profile_rotate::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Writes one `overflow_blocked` record, in the format the step writer
/// (#2893, #2911) uses, before the rotator has run. No rotator runs at this
/// point, so the lock is not needed.
fn record_block(f: &Fixture, login: &str, reason: &str, until: i64) {
    fs::create_dir_all(f.rotate_dir()).unwrap();
    fs::set_permissions(f.rotate_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    private_file(
        &f.rotate_dir().join("ledger.json"),
        &serde_json::json!({
            "version": 1,
            "overflow_blocked": {login: {"at": now() - 60, "until": until, "reason": reason}}
        })
        .to_string(),
    );
}

fn check_grammar(f: &Fixture) {
    for l in f.log().lines() {
        assert_log_grammar(l, true);
    }
}

/// The 21 November 07:20Z cycle on candide: the live login is past its
/// session threshold and the only candidate with session room is at 98% of
/// its week. Before #2910 the plan-room pass moved onto it. Now it is never
/// chosen; the cycle says why for each candidate and raises one ALERT.
/// Raising `--week-at` above 98 makes it a target again, which shows the
/// threshold, not some other rule, kept it out.
#[test]
fn a_login_at_98_percent_of_its_week_is_never_chosen() {
    let f = Fixture::new();
    f.usage("work", 90, 40);
    f.usage("spare", 10, 98);
    f.usage("extra", 95, 30);

    f.ok(&["profile", "rotate"]);
    let log = f.log();
    assert_eq!(
        ino(&f.home_login()),
        Some(f.live_ino),
        "no swap onto a 98%-week login:\n{log}"
    );
    assert!(
        log.contains("reject: spare week w=98% at or above --week-at 90"),
        "{log}"
    );
    assert!(
        log.contains("reject: extra session s=95% at or above --session-at 85"),
        "{log}"
    );
    assert_eq!(
        log.matches("ALERT: no profile has capacity live=work")
            .count(),
        1,
        "one clear line, no swap:\n{log}"
    );
    assert!(!log.contains(" swap "), "{log}");
    println!("[#2910 week] {}", log.replace('\n', "\n[#2910 week] "));
    check_grammar(&f);

    f.ok(&["profile", "rotate", "--week-at", "99"]);
    let log = f.log();
    assert_eq!(
        ino(&f.home_login()),
        Some(f.spare_ino),
        "below a raised --week-at, spare is a target:\n{log}"
    );
    // The pass that moved candide onto a 98%-week login: plan-room, now held
    // to `--week-at` like every other pass.
    assert!(
        log.lines()
            .any(|l| l.contains(" swap work->spare live ") && l.ends_with(" reason=plan-room")),
        "{log}"
    );
    f.assert_each_login_once();
    check_grammar(&f);
}

/// A login with a recorded session limit is passed over before its reset,
/// although its reading has the most room, and the log names the limit and
/// the reset. Any limit kind counts, not only `spend_limit`.
#[test]
fn a_blocked_login_is_passed_over_before_its_reset() {
    let f = Fixture::new();
    f.usage("work", 100, 40);
    f.usage("spare", 10, 20);
    f.usage("extra", 50, 50);
    let until = now() + 3 * 3600;
    record_block(&f, "spare", "session_limit", until);

    f.ok(&["profile", "rotate"]);
    let log = f.log();
    assert_eq!(
        ino(&f.home_login()),
        Some(f.extra_ino),
        "spare (10/20) has more room than extra (50/50) but is blocked:\n{log}"
    );
    let stamp = chrono::DateTime::from_timestamp(until, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let want = format!("reject: spare session limit recorded until {stamp}");
    assert!(log.contains(&want), "want {want:?} in:\n{log}");
    assert!(log.contains(" swap work->extra live "), "{log}");
    println!(
        "[#2910 blocked] {}",
        log.replace('\n', "\n[#2910 blocked] ")
    );
    f.assert_each_login_once();
    check_grammar(&f);
}

/// The same login, with a weekly limit whose reset has passed, is an
/// ordinary candidate again and, having the most room, is chosen.
#[test]
fn a_blocked_login_is_chosen_after_its_reset() {
    let f = Fixture::new();
    f.usage("work", 100, 40);
    f.usage("spare", 10, 20);
    f.usage("extra", 50, 50);
    record_block(&f, "spare", "week_limit", now() - 10);

    f.ok(&["profile", "rotate"]);
    let log = f.log();
    assert_eq!(
        ino(&f.home_login()),
        Some(f.spare_ino),
        "a block whose reset passed holds nothing:\n{log}"
    );
    assert!(log.contains(" swap work->spare live "), "{log}");
    assert!(!log.contains("reject: spare"), "{log}");
    println!(
        "[#2910 released] {}",
        log.replace('\n', "\n[#2910 released] ")
    );
    f.assert_each_login_once();
    check_grammar(&f);
}
