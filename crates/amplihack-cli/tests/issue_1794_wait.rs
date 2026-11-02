//! Issue #1794: `amplihack wait --seconds N` sleeps in its own process and
//! starts nothing, so an agent can wait without `tail -f /dev/null` and
//! without leaving an orphan behind.
//!
//! The binary is started through [`crate::isolated_amplihack::isolated`], so
//! it never sees the real HOME or `AMPLIHACK_CLAUDE_PROFILE_ROOT` (#1699).
//! The child leads its own process group, so any process it started while
//! it sleeps is found by the `/proc` scan, which runs only until the child
//! is reaped. The scan only reads `/proc`; it sends no signals.

#![cfg(target_os = "linux")]

use crate::isolated_amplihack::isolated;
use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Generous: the suite runs on hosts with a load average near 100.
const UPPER_BOUND: Duration = Duration::from_secs(30);

fn wait_command(args: &[&str]) -> Command {
    let mut cmd = isolated(amplihack_bin());
    cmd.arg("wait")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn describe(out: &Output) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The PIDs listed in `/proc/<pid>/task/*/children`. A task that ends while
/// it is read is skipped.
fn children_of(pid: u32) -> Vec<String> {
    // The child is alive and unreaped while this runs, so its task
    // directory must be readable; an empty answer here would pass vacuously.
    fs::read_dir(format!("/proc/{pid}/task"))
        .expect("read /proc/<pid>/task of the running wait")
        .flatten()
        .filter_map(|task| fs::read_to_string(task.path().join("children")).ok())
        .flat_map(|text| {
            text.split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `(pid, ppid, pgid)` for every process whose stat file can be read.
fn processes() -> Vec<(u32, u32, u32)> {
    fs::read_dir("/proc")
        .expect("read /proc")
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let stat = fs::read_to_string(entry.path().join("stat")).ok()?;
            // The command name may hold spaces and parentheses; the fields
            // after the last `)` are state, ppid, pgrp, ...
            let rest = stat.get(stat.rfind(')')? + 1..)?;
            let mut fields = rest.split_whitespace().skip(1);
            let ppid = fields.next()?.parse().ok()?;
            let pgid = fields.next()?.parse().ok()?;
            Some((pid, ppid, pgid))
        })
        .collect()
}

/// Processes, other than `pid` itself, whose parent or group is `pid`.
fn descendants_of(pid: u32) -> Vec<(u32, u32, u32)> {
    processes()
        .into_iter()
        .filter(|&(p, ppid, pgid)| p != pid && (ppid == pid || pgid == pid))
        .collect()
}

/// Kills and reaps the child it holds when dropped, so a failed assert does
/// not leave a running `amplihack wait` behind.
struct KillOnDrop(Option<Child>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            // The child may already have exited; either result is fine here.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn wait_sleeps_in_process_and_starts_no_children() {
    let mut cmd = wait_command(&["--seconds", "2"]);
    cmd.process_group(0);
    let start = Instant::now();
    let mut guard = KillOnDrop(Some(cmd.spawn().expect("spawn amplihack wait")));
    let child = guard.0.as_mut().expect("child is held");
    let pid = child.id();

    // Scans run only while the child is unreaped, so its PID cannot have
    // been reused by another process. No scan runs after it is reaped.
    let mut seen = Vec::new();
    while child.try_wait().expect("try_wait").is_none() {
        seen.extend(children_of(pid));
        seen.extend(descendants_of(pid).into_iter().map(|(p, ..)| p.to_string()));
        assert!(start.elapsed() < UPPER_BOUND, "wait did not end");
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = guard
        .0
        .take()
        .expect("child is held")
        .wait_with_output()
        .expect("collect output");
    let elapsed = start.elapsed();

    assert!(seen.is_empty(), "wait started processes: {seen:?}");
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(
        elapsed >= Duration::from_secs(2),
        "returned after {elapsed:?}"
    );
    assert!(elapsed < UPPER_BOUND, "took {elapsed:?}");
    assert!(out.stdout.is_empty(), "{}", describe(&out));
    assert!(out.stderr.is_empty(), "{}", describe(&out));
}

#[test]
fn wait_runs_one_second_to_completion() {
    // Build the command first: the first `amplihack_bin()` call runs
    // `cargo build`, which must not count against the timing bounds.
    let mut cmd = wait_command(&["--seconds", "1"]);
    let start = Instant::now();
    let out = cmd.output().expect("run amplihack wait");
    let elapsed = start.elapsed();
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(
        elapsed >= Duration::from_secs(1),
        "returned after {elapsed:?}"
    );
    assert!(elapsed < UPPER_BOUND, "took {elapsed:?}");
    assert!(out.stdout.is_empty(), "{}", describe(&out));
    assert!(out.stderr.is_empty(), "{}", describe(&out));
}

#[test]
fn wait_zero_seconds_returns_at_once() {
    let mut cmd = wait_command(&["--seconds", "0"]);
    let start = Instant::now();
    let out = cmd.output().expect("run amplihack wait");
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(start.elapsed() < UPPER_BOUND);
    assert!(out.stdout.is_empty(), "{}", describe(&out));
    assert!(out.stderr.is_empty(), "{}", describe(&out));
}

#[test]
fn wait_accepts_the_upper_limit_in_help_validation() {
    // 86400 is accepted by the parser; `--help` after it shows help and does
    // not sleep, so the range check is seen without waiting a day.
    let out = wait_command(&["--seconds", "86400", "--help"])
        .output()
        .expect("run amplihack wait");
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
}

#[test]
fn wait_rejects_invalid_input_with_a_usage_error() {
    for args in [
        &["--seconds", "-1"][..],
        &["--seconds", "abc"],
        &["--seconds", "1.5"],
        &["--seconds", "86401"],
        &["--seconds", ""],
        &["--seconds"],
        &[],
        &["--seconds", "1", "extra"],
        &["5"],
    ] {
        let mut cmd = wait_command(args);
        let start = Instant::now();
        let out = cmd.output().expect("run amplihack wait");
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", describe(&out));
        assert!(!out.stderr.is_empty(), "{args:?}: {}", describe(&out));
        assert!(
            start.elapsed() < UPPER_BOUND,
            "{args:?} slept before failing"
        );
    }
}

#[test]
fn wait_help_explains_when_to_use_it() {
    let out = wait_command(&["--help"]).output().expect("run --help");
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let help = String::from_utf8_lossy(&out.stdout);
    for needle in ["--seconds", "tail -f /dev/null", "wait $pid"] {
        assert!(help.contains(needle), "help lacks {needle:?}:\n{help}");
    }
}

#[test]
fn top_level_help_lists_wait() {
    let out = isolated(amplihack_bin())
        .arg("--help")
        .output()
        .expect("run --help");
    assert!(out.status.success(), "{}", describe(&out));
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(
        help.lines().any(|l| l.trim_start().starts_with("wait ")),
        "top-level help does not list `wait`:\n{help}"
    );
}
