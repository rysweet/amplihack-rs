//! Issue #2909: a limit record makes the rotator rotate at once, and a
//! recorded reset wakes it. Every rotator runs with `--interval 3600` in a
//! temporary HOME, so a swap within the test's ceiling can only have come
//! from the event, never from an interval tick. The "limit record" is
//! written by a stand-in that edits the rotate ledger directly; the step
//! that writes it for real belongs to #2911. No sleeps: `wait_until` only.

#![cfg(target_os = "linux")]

use crate::support::fake_proc::FakeProc;
use crate::support::profile_rotate::*;
use crate::support::wait::{output_within, wait_until};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// Kills the rotator named by the fixture's pid file if a test fails first.
struct Reaper(PathBuf, FakeProc);

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

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Starts a rotator that waits up to an hour between ticks.
fn start_long(f: &Fixture) -> (Reaper, i32) {
    let reaper = Reaper(f.rotate_dir().join("rotate.pid"), FakeProc::new());
    f.with_proc(&reaper.1);
    let out = output_within(
        "`profile rotate start --interval 3600`",
        &mut f.cmd(&[
            "profile",
            "rotate",
            "start",
            "--interval",
            "3600",
            "--hold",
            "0",
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
    (reaper, pid)
}

/// The stand-in recorder: merges one block into the ledger and renames it
/// into place, as the rotator's own atomic write does.
fn record_block(f: &Fixture, login: &str, reason: &str, at: i64, until: i64) {
    let path = f.rotate_dir().join("ledger.json");
    let mut v = f
        .ledger()
        .unwrap_or_else(|| serde_json::json!({"version": 1}));
    v["overflow_blocked"][login] = serde_json::json!({"at": at, "until": until, "reason": reason});
    let tmp = f.rotate_dir().join("ledger.json.stand-in");
    fs::write(&tmp, v.to_string()).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(&tmp, &path).unwrap();
}

fn cycles(f: &Fixture) -> usize {
    f.log().lines().filter(|l| l.contains(" cycle ")).count()
}

/// The byte offset of the first log line containing `needle`.
fn first(log: &str, needle: &str) -> Option<usize> {
    log.find(needle)
}

#[test]
fn a_limit_record_makes_the_rotator_swap_inside_its_interval() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    let (_r, pid) = start_long(&f);
    wait_until("the first cycle", || {
        f.log().lines().any(|l| l.contains(" cycle live=work"))
    });
    assert!(!f.log().contains(" swap "), "work has room: {}", f.log());
    // The login runs out and a step records the limit.
    f.usage("work", 100, 40);
    record_block(&f, "work", "session_limit", now(), now() + 3 * 3600);
    wait_until("a swap off work caused by the record", || {
        f.log()
            .lines()
            .any(|l| l.contains(" swap ") && l.contains("work->"))
    });
    let log = f.log();
    // spare (10/20) has more room than extra (50/50), so it is the target.
    assert!(log.contains(" swap work->spare live "), "{log}");
    // With `--interval 3600` the only way to a second cycle inside the
    // test's ceiling is a wake, and the wake the log names is the record.
    let woke = first(&log, "wake: ledger changed").expect("the record woke the rotator");
    assert!(
        Some(woke) < first(&log, " swap work->"),
        "the wake comes before the swap it caused:\n{log}"
    );
    assert!(!log.contains("wake: interval"), "no interval tick: {log}");
    for l in log.lines() {
        assert_log_grammar(l, true);
    }
    let (out, refused) = f.stop_until_gone(pid);
    f.assert_stopped(&out, refused, pid);
}

#[test]
fn a_recorded_reset_wakes_the_rotator_and_frees_the_login() {
    let f = Fixture::new();
    // work and extra are spent and spare is blocked until a moment from
    // now: no target at first, then the reset passes with no interval tick.
    f.usage("work", 100, 40);
    f.usage("extra", 100, 40);
    fs::create_dir_all(f.rotate_dir()).unwrap();
    fs::set_permissions(f.rotate_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    record_block(&f, "spare", "week_limit", now() - 10, now() + 4);
    let (_r, pid) = start_long(&f);
    wait_until("a swap to spare after its reset", || {
        f.log()
            .lines()
            .any(|l| l.contains(" swap ") && l.contains("->spare live "))
    });
    let (out, refused) = f.stop_until_gone(pid);
    f.assert_stopped(&out, refused, pid);
    let log = f.log();
    assert!(cycles(&f) >= 2, "{log}");
    let woke = first(&log, "wake: reset due").expect("the reset woke the rotator");
    assert!(
        Some(woke) < first(&log, "->spare live "),
        "the reset wake comes before the swap onto spare:\n{log}"
    );
    assert!(
        first(&log, "reject: spare").is_some_and(|r| r < woke),
        "before its reset spare was passed over as blocked:\n{log}"
    );
    for l in log.lines() {
        assert_log_grammar(l, true);
    }
}

#[test]
fn stop_finishes_while_the_rotator_waits_out_a_long_interval() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    let (_r, pid) = start_long(&f);
    wait_until("the first cycle", || cycles(&f) >= 1);
    let out = output_within(
        "`profile rotate stop`",
        &mut f.cmd(&["profile", "rotate", "stop"]),
    );
    assert_no_secrets(&out);
    assert!(
        out.status.success(),
        "stop must not be refused during a long wait: {}",
        text(&out.stderr)
    );
    wait_until("the rotator to exit", || !alive(pid));
}
