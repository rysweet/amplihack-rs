//! Issue #1698: the rotator when probes are slow, end to end through the
//! binary with the rotate `Fixture` (temporary HOME and profile root, a fake
//! `claude`, `isolated(..)`; never a real `claude` or profile root).
//!
//! Contract:
//! - A candidate whose probe passes the deadline shows as `<name>:failed`
//!   in the cycle line, never as a percentage, is never swapped to, warns
//!   `probe of <name> timed out` on stderr, and is probed again on the next
//!   cycle.
//! - The live probe counts against `AMPLIHACK_PROFILE_USAGE_PARALLEL`.
//! - A plan login whose probe timed out holds back the move from a plan
//!   login onto an overflow profile; the cycle alerts instead.
//!
//! The fake answers no usage request here, so every probe falls back to
//! `/usage`, the default `auto` path.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;
use std::fs;
use std::process::Output;

const TIMEOUT_ENV: &str = "AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS";

/// `p`'s `/usage` sleeps 30 s, past any deadline these tests set.
fn slow(f: &Fixture, p: &str) {
    fs::write(f.canned.join(format!("{}.slow", email(p))), "").unwrap();
}

fn rotate(f: &Fixture, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = f.cmd(args);
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    assert_no_secrets(&out);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    out
}

fn cycle_line(out: &Output) -> String {
    let stdout = text(&out.stdout);
    stdout
        .lines()
        .find(|l| l.contains("cycle live="))
        .unwrap_or_else(|| panic!("no cycle line: {stdout}"))
        .to_string()
}

#[test]
fn a_timed_out_candidate_is_failed_never_chosen_and_probed_again() {
    let f = Fixture::new();
    // work is at the session threshold; extra has no capacity, so only a
    // misread spare could be a target.
    slow(&f, "spare");
    f.usage("extra", 99, 99);
    for cycle in 1..=2 {
        let out = rotate(&f, &["profile", "rotate"], &[(TIMEOUT_ENV, "3")]);
        let line = cycle_line(&out);
        assert_log_grammar(&line, true);
        assert!(line.contains(" spare:failed"), "{line}");
        assert!(!line.contains("spare:s="), "never a percentage: {line}");
        assert!(
            !text(&out.stdout).contains("->spare"),
            "{}",
            text(&out.stdout)
        );
        assert!(
            text(&out.stderr).contains("warning: rotate: probe of spare timed out"),
            "{}",
            text(&out.stderr)
        );
        assert_eq!(f.usage_probes("spare"), cycle, "probed every cycle");
    }
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    assert_each_login_once_by_inode(&f);
}

/// The fixture's three logins are where they started.
fn assert_each_login_once_by_inode(f: &Fixture) {
    assert_eq!(f.where_is(f.live_ino).as_deref(), Some("~/.claude"));
    assert_eq!(f.where_is(f.spare_ino).as_deref(), Some("spare"));
    assert_eq!(f.where_is(f.extra_ino).as_deref(), Some("extra"));
}

#[test]
fn the_live_probe_counts_against_the_bound() {
    let f = Fixture::new();
    f.add_logged_in("fourth", 60, 60);
    f.add_logged_in("fifth", 70, 70);
    fs::write(f.canned.join("marks"), "").unwrap();
    rotate(
        &f,
        &["profile", "rotate", "--dry-run"],
        &[("AMPLIHACK_PROFILE_USAGE_PARALLEL", "2")],
    );
    let marks = fs::read_to_string(f.records.join("marks")).unwrap();
    let (mut running, mut peak, mut starts) = (0usize, 0usize, 0usize);
    for line in marks.lines() {
        if line.starts_with("S ") {
            starts += 1;
            running += 1;
            peak = peak.max(running);
        } else {
            running -= 1;
        }
    }
    assert_eq!(starts, 5, "the live login and four candidates: {marks}");
    assert_eq!(running, 0, "{marks}");
    assert_eq!(peak, 2, "five probes at a bound of 2: {marks}");
}

#[test]
fn a_timed_out_plan_probe_holds_the_move_onto_overflow() {
    let f = Fixture::new();
    f.ok(&["profile", "overflow", "add", "extra"]);
    // work is out and back in 3 hours, beyond the 30-minute wait; spare's
    // probe times out, so it may still have capacity.
    f.usage_resetting("work", 100, 40, 3);
    slow(&f, "spare");
    f.usage("extra", 100, 100);
    let out = rotate(&f, &["profile", "rotate"], &[(TIMEOUT_ENV, "5")]);
    let stdout = text(&out.stdout);
    assert!(cycle_line(&out).contains(" spare:failed"), "{stdout}");
    assert!(!stdout.contains("reason=overflow-in"), "{stdout}");
    // The ALERT counts spare among the candidates that could not be read.
    assert!(
        stdout.contains("ALERT: no profile has capacity live=work")
            && stdout.contains("(candidates: 1 failed, 1 exhausted)"),
        "{stdout}"
    );
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));

    // Once spare reads exhausted too, the same cycle moves onto extra.
    fs::remove_file(f.canned.join(format!("{}.slow", email("spare")))).unwrap();
    f.usage_resetting("spare", 100, 100, 3);
    let out = rotate(&f, &["profile", "rotate"], &[]);
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("swap work->extra") && stdout.contains("reason=overflow-in"),
        "{stdout}"
    );
}
