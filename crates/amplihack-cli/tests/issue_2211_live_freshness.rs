//! Issue #2211: the rotator's live reading freshness, driven from outside
//! through the built `amplihack` binary on the synthetic fixture of
//! `support::profile_rotate` (temporary HOME and profile root, fake
//! `claude`, login files holding only sentinel values).
//!
//! The waits are on the rotator's own log lines, never on elapsed time.

#![cfg(target_os = "linux")]

use crate::support::fake_proc::FakeProc;
use crate::support::profile_rotate::*;
use crate::support::wait::{output_within, wait_until};
use std::fs;

/// Kills the rotator a test started if the test fails before `stop`.
struct Reaper(std::path::PathBuf, FakeProc);

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Ok(s) = fs::read_to_string(&self.0)
            && let Ok(pid) = s.trim().parse::<i32>()
            && pid > 1
        {
            let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            if String::from_utf8_lossy(&cmdline).contains("rotate") {
                // SAFETY: plain kill(2) of a process this test started.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

/// One-shot: a moving, recent live reading below `--session-at` stays and
/// makes no live re-read; a reading over it still swaps on threshold.
#[test]
fn one_shot_fresh_live_reading_stays_and_threshold_still_swaps() {
    let f = Fixture::new();
    f.usage("work", 50, 40);
    // No candidate is 30 points better, so no upgrade swap either.
    f.usage("spare", 45, 40);
    let out = f.ok(&["profile", "rotate", "--session-at", "70"]);
    assert_no_secrets(&out);
    let log = f.log();
    assert!(log.contains(" cycle live=work"), "{log}");
    assert!(!log.contains("stale live reading"), "fresh reading: {log}");
    assert!(!log.contains(" swap "), "50% stays: {log}");
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));

    f.usage("work", 90, 40);
    f.usage("spare", 10, 20);
    let out = f.ok(&["profile", "rotate", "--session-at", "70"]);
    assert_no_secrets(&out);
    let log = f.log();
    assert!(
        log.lines()
            .any(|l| l.contains(" swap work->") && l.ends_with("reason=threshold")),
        "{log}"
    );
    assert!(!log.contains("stale live reading"), "{log}");
    for l in log.lines() {
        assert_log_grammar(l, true);
    }
    f.assert_each_login_once();
}

/// A running rotator whose live login reads the same non-zero figures on
/// three cycles re-reads it once, logs `stale live reading; re-read`, takes
/// the unchanged re-read as true and does not swap below the threshold.
#[test]
fn running_rotator_rereads_a_frozen_live_reading() {
    let f = Fixture::new();
    f.usage("work", 50, 40);
    // No candidate is 30 points better, so the live login stays put.
    f.usage("spare", 45, 40);
    let reaper = Reaper(f.rotate_dir().join("rotate.pid"), FakeProc::new());
    f.with_proc(&reaper.1);
    let out = output_within(
        "`profile rotate start`",
        &mut f.cmd(&[
            "profile",
            "rotate",
            "start",
            "--interval",
            "1",
            "--hold",
            "0",
            "--session-at",
            "70",
        ]),
    );
    assert_no_secrets(&out);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let pid: i32 = fs::read_to_string(&reaper.0)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    reaper.1.link_own(pid as u32);

    wait_until("a stale live reading re-read", || {
        f.log().contains("stale live reading; re-read")
    });
    wait_until("a cycle after the re-read", || {
        f.log()
            .split("stale live reading; re-read")
            .nth(1)
            .is_some_and(|rest| rest.contains(" cycle live=work"))
    });
    let (out, refused) = f.stop_until_gone(pid);
    f.assert_stopped(&out, refused, pid);

    let log = f.log();
    assert!(!log.contains(" swap "), "a frozen 50% stays: {log}");
    assert!(!log.contains("reread: work failed"), "{log}");
    let first = log
        .lines()
        .position(|l| l.ends_with("stale live reading; re-read"))
        .unwrap();
    let cycles_before = log
        .lines()
        .take(first)
        .filter(|l| l.contains(" cycle live=work work:s=50%,w=40%"))
        .count();
    assert!(
        cycles_before >= 3,
        "the third identical reading is the first stale one: {log}"
    );
    for l in log.lines() {
        assert_log_grammar(l, true);
    }
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    f.assert_each_login_once();
}
