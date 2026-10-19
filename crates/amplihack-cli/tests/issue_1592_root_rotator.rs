//! Issue #1592: a running rotator keeps the root it started with, and
//! `rotate status` and `rotate stop` find it through `~/.amplihack/rotate-root`
//! from a shell whose root now resolves elsewhere. Every rotator runs under a
//! temporary HOME and is stopped (or killed) before the test ends.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;
use std::fs;
use std::path::PathBuf;
use std::process::Output;
use std::time::{Duration, Instant};

const ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";

fn rotate_record(f: &Fixture) -> PathBuf {
    f.home.join(".amplihack").join("rotate-root")
}

fn other_root(f: &Fixture) -> PathBuf {
    f.home.join("other-profiles")
}

fn ok_no_env(f: &Fixture, args: &[&str]) -> Output {
    let out = f.cmd_no_root_env(args).output().unwrap();
    assert_no_secrets(&out);
    assert!(out.status.success(), "{args:?}: {}", text(&out.stderr));
    out
}

/// Stops the rotator if a test fails before stopping it: `rotate stop`
/// under the test's root first, then SIGTERM only to a process that is
/// still a rotator and was started with this test's root, so a reused pid
/// is never signalled.
struct Guard {
    pid: i32,
    root: PathBuf,
    stop: std::process::Command,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.stop.output();
        let cmdline = fs::read(format!("/proc/{}/cmdline", self.pid)).unwrap_or_default();
        let cmdline = String::from_utf8_lossy(&cmdline);
        let environ = fs::read(format!("/proc/{}/environ", self.pid)).unwrap_or_default();
        let ours = format!("{ROOT_ENV}={}", self.root.display());
        let tied = environ.split(|b| *b == 0).any(|v| v == ours.as_bytes());
        if cmdline.contains("rotate") && cmdline.contains("watch") && tied {
            // SAFETY: plain kill(2) of a process this test started.
            unsafe { libc::kill(self.pid, libc::SIGTERM) };
        }
    }
}

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn pid_file(f: &Fixture) -> PathBuf {
    f.rotate_dir().join("rotate.pid")
}

#[test]
fn the_rotator_keeps_its_start_root_and_status_finds_it() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    let r = f.root.display().to_string();
    let out = f.ok(&[
        "profile",
        "rotate",
        "start",
        "--interval",
        "1",
        "--hold",
        "0",
    ]);
    assert!(pid_file(&f).starts_with(f.tmp.path()));
    let pid: i32 = fs::read_to_string(pid_file(&f))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _guard = Guard {
        pid,
        root: f.root.clone(),
        stop: f.cmd(&["profile", "rotate", "stop"]),
    };
    assert!(
        text(&out.stdout).contains(&format!("root: {r}\n")),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(
        fs::read_to_string(f.rotate_dir().join("root")).unwrap(),
        format!("{r}\n")
    );
    assert_eq!(
        fs::read_to_string(rotate_record(&f)).unwrap(),
        format!("{r}\n")
    );

    // The shell's root moves elsewhere: no variable, another saved root.
    ok_no_env(&f, &["profile", "root", other_root(&f).to_str().unwrap()]);
    let log_len = f.log().len();
    wait_for("a cycle after the root moved", || {
        f.log()[log_len..]
            .lines()
            .any(|l| l.contains(" cycle live="))
    });
    for mut cmd in [
        f.cmd_no_root_env(&["profile", "rotate", "status"]),
        f.cmd(&["profile", "rotate", "status"]),
    ] {
        cmd.env(ROOT_ENV, "");
        let out = cmd.output().unwrap();
        let s = text(&out.stdout);
        assert!(
            s.starts_with(&format!("root: {r} (pinned by running rotator)\n")),
            "{s}"
        );
        assert!(
            s.contains("rotate: running\n") && s.contains(&format!("pid: {pid}\n")),
            "{s}"
        );
    }

    let out = ok_no_env(&f, &["profile", "rotate", "stop"]);
    assert!(text(&out.stdout).contains(&format!("rotate: stopped (pid {pid})")));
    assert!(!pid_file(&f).exists());
    assert!(
        rotate_record(&f).exists(),
        "stop leaves the record in place"
    );
    let s = text(&ok_no_env(&f, &["profile", "rotate", "status"]).stdout);
    let want = format!(
        "root: {} (source: config)\nrotate: stopped\n",
        other_root(&f).display()
    );
    assert!(s.starts_with(&want), "{s}");
}

/// A record naming a root whose rotator is not running is ignored, and
/// its pid file is neither removed nor signalled.
#[test]
fn a_stale_rotate_root_record_is_ignored() {
    let f = Fixture::new();
    let other = other_root(&f);
    let foreign = other.join(".auto-switch.d/rotate/rotate.pid");
    private_file(&foreign, &format!("{}\n", std::process::id()));
    private_file(&rotate_record(&f), &format!("{}\n", other.display()));

    let s = text(&f.ok(&["profile", "rotate", "status"]).stdout);
    let want = format!(
        "root: {} (source: env)\nrotate: stopped\n",
        f.root.display()
    );
    assert!(s.starts_with(&want), "{s}");
    let out = f.ok(&["profile", "rotate", "stop"]);
    assert!(
        text(&out.stdout).contains("rotate: not running"),
        "{}",
        text(&out.stdout)
    );
    assert!(foreign.exists(), "the other root's pid file is untouched");
}
