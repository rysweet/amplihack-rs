//! Issue #1746, review of #1766: the background rotator that `profile rotate
//! start` launches follows the `sessions.daily_archive`,
//! `sessions.archive_days` and `sessions.archive_dest` config keys, and
//! `sessions restore` in a shell finds what it archived.
//!
//! The switch used to be `AMPLIHACK_ROTATE_SESSIONS_ARCHIVE`, which
//! `daemon::spawn` removes with every other `AMPLIHACK_ROTATE*` variable
//! before it starts the rotator, so turning it off did nothing. These tests
//! start a real rotator against a temporary HOME holding one old session,
//! and check what it does with the key set by `amplihack config set`.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::{Fixture, text};
use crate::support::sessions_archive::age;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const ID: &str = "4f1c2a7e-9b3d-4c55-8e21-6a0f3d9b7c10";

/// Kills the rotator named in the fixture's pid file when a test ends
/// without stopping it, so none outlives its temporary HOME.
struct Reaper(PathBuf);

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

fn wait_for(what: &str, f: &Fixture, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "timed out waiting for {what}; log:\n{}",
            f.log()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// `<home>/.claude/projects/-p/<ID>.jsonl`, ten days old.
fn old_session(f: &Fixture) -> PathBuf {
    let dir = f.home.join(".claude/projects/-p");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{ID}.jsonl"));
    fs::write(&file, "{\"type\":\"user\"}\n").unwrap();
    age(&file, 10);
    file
}

fn record(f: &Fixture) -> PathBuf {
    f.rotate_dir().join("sessions-archive.last")
}

fn archive_dir(f: &Fixture) -> PathBuf {
    f.home.join(".amplihack/session-archive")
}

fn cycles(f: &Fixture) -> usize {
    f.log().matches(" cycle live=").count()
}

fn start(f: &Fixture) -> Reaper {
    let reaper = Reaper(f.rotate_dir().join("rotate.pid"));
    let out = f.ok(&[
        "profile",
        "rotate",
        "start",
        "--interval",
        "1",
        "--hold",
        "0",
    ]);
    assert!(
        text(&out.stdout).contains("rotate: started (pid "),
        "{}",
        text(&out.stdout)
    );
    reaper
}

#[test]
fn a_rotator_started_with_daily_archive_off_never_archives() {
    let f = Fixture::new();
    let session = old_session(&f);
    f.ok(&["config", "set", "sessions.daily_archive", "false"]);
    let _reaper = start(&f);
    // The daily run follows each cycle, so by the third cycle two passes
    // have each had their chance to run it.
    wait_for("three cycles", &f, || cycles(&f) >= 3);
    f.ok(&["profile", "rotate", "stop"]);
    let log = f.log();
    assert!(!log.contains("sessions archive: "), "{log}");
    assert!(session.exists(), "the old session was not archived");
    assert!(!record(&f).exists(), "off writes no record");
    assert!(!archive_dir(&f).exists(), "no archive directory");
}

#[test]
fn turning_daily_archive_on_reaches_the_running_rotator() {
    let f = Fixture::new();
    let session = old_session(&f);
    f.ok(&["config", "set", "sessions.daily_archive", "false"]);
    let _reaper = start(&f);
    wait_for("two cycles", &f, || cycles(&f) >= 2);
    assert!(session.exists());
    // No restart: the running rotator reads the file on its next pass.
    f.ok(&["config", "set", "sessions.daily_archive", "true"]);
    wait_for("the daily run", &f, || {
        f.log().contains("sessions archive: archived ")
    });
    f.ok(&["profile", "rotate", "stop"]);
    let log = f.log();
    assert!(
        log.contains("sessions archive: archived 1 sessions, "),
        "{log}"
    );
    assert!(!session.exists(), "the old session was archived");
    assert!(
        archive_dir(&f)
            .join("-p")
            .join(format!("{ID}.tar.gz"))
            .is_file(),
        "{log}"
    );
    assert!(record(&f).exists());
}

#[test]
fn the_running_rotator_and_restore_in_a_shell_use_the_same_configured_dest() {
    // Review of #1766: days and DEST used to come from the rotator's
    // environment, fixed when it was launched, so a DEST set later in a
    // shell sent `restore` to a directory the daily run never wrote to.
    // Both now read the config keys.
    let f = Fixture::new();
    let session = old_session(&f);
    f.ok(&["config", "set", "sessions.daily_archive", "false"]);
    let _reaper = start(&f);
    wait_for("two cycles", &f, || cycles(&f) >= 2);
    // Set after the rotator started, then turn the run on.
    let dest = f.home.join("configured-archive");
    f.ok(&[
        "config",
        "set",
        "sessions.archive_dest",
        dest.to_str().unwrap(),
    ]);
    f.ok(&["config", "set", "sessions.archive_days", "5"]);
    f.ok(&["config", "set", "sessions.daily_archive", "true"]);
    wait_for("the daily run", &f, || {
        f.log().contains("sessions archive: archived ")
    });
    f.ok(&["profile", "rotate", "stop"]);
    let log = f.log();
    assert!(
        log.contains("sessions archive: archived 1 sessions, ")
            && log.contains("(older than 5 days; "),
        "{log}"
    );
    assert!(!session.exists(), "the old session was archived");
    assert!(
        dest.join("-p").join(format!("{ID}.tar.gz")).is_file(),
        "{log}"
    );
    assert!(
        !archive_dir(&f).exists(),
        "nothing went to the default DEST"
    );
    // No --dest: restore finds the session through the same key.
    let out = f.ok(&["sessions", "restore", ID]);
    assert!(
        text(&out.stdout).starts_with("restored 1 files to "),
        "{}",
        text(&out.stdout)
    );
    assert!(session.is_file(), "restored into ~/.claude/projects");
}
