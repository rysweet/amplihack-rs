//! Issue #2911: a usage-limit wait is woken by the event that ends it, not
//! by a pass on a timer.
//!
//! These run the real `amplihack` binary on the real clock (`TEST_SCALE` 1),
//! so a second here is a second. The wait used to read its files every 30
//! seconds and sleep in one-second slices; it now blocks on file events, a
//! stop signal and its parent's exit (`limit_wait::wake`). Linux only: the
//! file events these tests time are inotify's.

#![cfg(target_os = "linux")]

use crate::issue_1618_fixture::{Fixture, WAIT_PREFIX, describe, events};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{real_now, rotator_running, write_cycle, x_is_live};
use crate::support::wait::{self, CEILING};
use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Well inside the 30 seconds the old pass took to notice anything, and far
/// above what a loaded host needs to deliver an inotify event.
const PROMPTLY: Duration = Duration::from_secs(10);

/// A `$HOME/.claude` step on the real clock, limited once by `message`.
fn real_time_home_step(f: &Fixture, message: &str) -> Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW")
        .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "1")
        .env("LIMIT_MESSAGE", message);
    c
}

/// A message with no reset, so the wait is a capacity wait that nothing but
/// an event ends inside its six-hour outer bound.
const NO_RESET: &str = "You've hit your session limit · try again later";

/// Spawns `c` and returns the child and a channel of its stderr lines, each
/// with the instant it was read.
fn spawn_reading(c: &mut Command) -> (std::process::Child, mpsc::Receiver<(Instant, String)>) {
    c.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = wait::spawn_once("the step", c);
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { return };
            if tx.send((Instant::now(), line)).is_err() {
                return;
            }
        }
    });
    (child, rx)
}

/// Reads lines until one starts with `prefix`, failing at the shared ceiling.
fn line_starting(
    rx: &mpsc::Receiver<(Instant, String)>,
    prefix: &str,
    seen: &mut Vec<String>,
) -> Instant {
    let deadline = Instant::now() + CEILING;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok((at, line)) => {
                let found = line.starts_with(prefix);
                seen.push(line);
                if found {
                    return at;
                }
            }
            Err(e) => panic!(
                "no line starting {prefix:?} ({e}); saw:\n{}",
                seen.join("\n")
            ),
        }
    }
}

/// The rotator's write recording a cycle with room ends the wait at once.
/// Under the 30-second pass the first reading came 30 seconds into the wait.
#[test]
fn a_rotator_write_ends_the_wait_within_seconds_not_at_a_thirty_second_pass() {
    let f = with_profiles();
    x_is_live(&f);
    let _rotator = rotator_running(&f);
    let mut c = real_time_home_step(&f, NO_RESET);
    let (mut child, rx) = spawn_reading(&mut c);
    let mut seen = Vec::new();
    let started = line_starting(&rx, &format!("{WAIT_PREFIX}start "), &mut seen);
    let start_secs = real_now();
    // The cycle must be measured after the wait's start, in a later second.
    wait::wait_until("the second after the wait's start", || {
        real_now() > start_secs
    });
    let wrote = Instant::now();
    write_cycle(&f.profiles, "Y", real_now());
    let ended = line_starting(&rx, &format!("{WAIT_PREFIX}end "), &mut seen);
    let status = wait::wait_child_with_ceiling("the step", &mut child, CEILING);
    assert!(status.success(), "{}", seen.join("\n"));
    assert!(
        seen.last()
            .is_some_and(|l| l.ends_with("end reason=rotator capacity")),
        "{}",
        seen.join("\n")
    );
    let took = ended.duration_since(wrote);
    assert!(
        took < PROMPTLY,
        "the wait noticed the rotator's write after {took:?}; it started {:?} before the write",
        wrote.duration_since(started)
    );
    assert_eq!(f.count("0"), 2, "one retry");
}

/// A stop signal ends a wait blocked on file events at once. A blocking
/// wait that retried its `poll(2)` on `EINTR` would never see it.
#[test]
fn a_stop_signal_ends_a_wait_blocked_on_file_events_at_once() {
    let f = Fixture::new();
    let mut c = real_time_home_step(&f, NO_RESET);
    let (mut child, rx) = spawn_reading(&mut c);
    let mut seen = Vec::new();
    line_starting(&rx, &format!("{WAIT_PREFIX}start "), &mut seen);
    let pid = libc::pid_t::try_from(child.id()).expect("a pid");
    let sent = Instant::now();
    // SAFETY: kill(2) on our own child.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let status = wait::wait_child_with_ceiling("the step", &mut child, CEILING);
    let took = sent.elapsed();
    while let Ok((_, line)) = rx.recv_timeout(Duration::from_secs(5)) {
        seen.push(line);
    }
    assert!(
        took < PROMPTLY,
        "the step took {took:?} to answer SIGTERM:\n{}",
        seen.join("\n")
    );
    assert_eq!(
        status.code(),
        Some(128 + libc::SIGTERM),
        "{}",
        seen.join("\n")
    );
    assert!(
        seen.iter().any(|l| l.ends_with("end reason=signal")),
        "{}",
        seen.join("\n")
    );
    assert_eq!(f.count("0"), 1, "no retry after a stop signal");
}

/// A weekly limit whose reset is days away, with nothing on the host to end
/// the wait early, is waited for once, to its reset, and the agent runs
/// again there and not before. No timer cuts the wait short and relaunches
/// the agent against the spent login.
#[test]
fn a_weekly_limit_runs_the_agent_once_at_its_reset_and_not_before() {
    let f = Fixture::new();
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR").env(
        "LIMIT_MESSAGE",
        "You've hit your weekly limit · resets Oct 20, 12am (America/Los_Angeles)",
    );
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    let ev = events(&out);
    let starts: Vec<&String> = ev.iter().filter(|l| l.starts_with("start ")).collect();
    assert_eq!(starts.len(), 1, "one wait: {ev:?}");
    assert!(
        starts[0].contains(" reset=2026-10-20 00:00:00 -07:00 ")
            && starts[0].contains(" until=2026-10-20 00:02:00 -07:00 ")
            && starts[0].ends_with(" bound=reset+margin"),
        "the wait's deadline is the stated reset plus the margin: {}",
        starts[0]
    );
    let ends: Vec<&String> = ev.iter().filter(|l| l.starts_with("end ")).collect();
    assert_eq!(ends, ["end reason=reset reached"], "{ev:?}");
    assert_eq!(
        f.count("0"),
        2,
        "the agent ran once for the limit and once at the reset: {}",
        describe(&out)
    );
}
