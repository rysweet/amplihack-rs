//! Issue #1794: `amplihack wait` blocks in-process, so an agent never needs
//! `tail -f /dev/null` or a `sleep` loop to wait.
//!
//! Every child goes through `isolated(amplihack_bin())`. The only other processes started
//! are `/bin/sleep` targets for `--until pid-exited:<PID>`; each is killed and
//! reaped by [`Reaped`] when the test ends, pass or fail. Nothing here runs
//! `tail`, touches tmux or reads the real HOME.

use crate::isolated_amplihack::isolated;
use crate::support::amplihack_bin::amplihack_bin;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn amplihack() -> Command {
    let mut cmd = isolated(amplihack_bin());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Allowance for starting the binary on a loaded host, added to every upper
/// bound. Each bound stays below the time the target or deadline would take,
/// so a wait that did not stop early still fails.
const STARTUP_SLACK: Duration = Duration::from_secs(5);

fn run(args: &[&str]) -> (Output, Duration) {
    // Build the command first: the first `amplihack_bin()` call may run
    // `cargo build`, which must not count towards the measured time.
    let mut cmd = amplihack();
    cmd.args(args);
    let start = Instant::now();
    let out = cmd.output().expect("amplihack binary must start");
    (out, start.elapsed())
}

/// A `/bin/sleep` target, killed and reaped on drop.
struct Reaped(Child);

impl Reaped {
    fn sleep(secs: &str) -> Self {
        Self(
            Command::new("/bin/sleep")
                .arg(secs)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("/bin/sleep must start"),
        )
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn seconds_two_exits_zero_in_window() {
    let (out, elapsed) = run(&["wait", "--seconds", "2"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        elapsed >= Duration::from_secs(2) && elapsed < Duration::from_secs(2) + STARTUP_SLACK,
        "wait --seconds 2 took {elapsed:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "wait writes nothing to stdout, got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn seconds_zero_returns_at_once() {
    let (out, elapsed) = run(&["wait", "--seconds", "0"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(elapsed < STARTUP_SLACK, "took {elapsed:?}");
}

/// The parent PID field of a `/proc/<pid>/stat` line, read after the last `)`
/// so a comm containing spaces or parentheses cannot shift the fields.
#[cfg(target_os = "linux")]
fn ppid_of(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_os = "linux")]
#[test]
fn seconds_spawns_no_children() {
    let mut child = amplihack()
        .args(["wait", "--seconds", "3"])
        .spawn()
        .expect("amplihack binary must start");
    let wait_pid = child.id();

    // Sample several times while the wait is running.
    let mut children = Vec::new();
    for _ in 0..8 {
        std::thread::sleep(Duration::from_millis(250));
        for entry in std::fs::read_dir("/proc").expect("/proc is readable") {
            let Ok(entry) = entry else { continue };
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            if ppid_of(&stat) == Some(wait_pid) {
                children.push(stat);
            }
        }
    }
    let status = child.wait().expect("wait for amplihack");
    assert!(
        children.is_empty(),
        "amplihack wait must not start child processes; found: {children:?}"
    );
    assert_eq!(status.code(), Some(0));
}

#[test]
fn until_pid_exited_returns_after_target_exits() {
    // Resolve the binary first: the first `amplihack_bin()` call may run
    // `cargo build`, which can outlast the one-second target.
    let _ = amplihack_bin();
    let target = Reaped::sleep("1");
    let pid = target.pid().to_string();
    let cond = format!("pid-exited:{pid}");
    // The sleep is our child, not amplihack's: it stays a zombie until
    // reaped, which the command must treat as exited.
    let (out, elapsed) = run(&["wait", "--until", &cond, "--deadline", "10"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        elapsed < Duration::from_secs(1) + STARTUP_SLACK,
        "took {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(500),
        "returned before the target could have exited: {elapsed:?}"
    );
}

#[test]
fn until_pid_that_is_already_gone_returns_at_once() {
    let mut target = Reaped::sleep("0");
    let pid = target.pid().to_string();
    let _ = target.0.wait();
    let cond = format!("pid-exited:{pid}");
    let (out, elapsed) = run(&["wait", "--until", &cond, "--deadline", "10"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(elapsed < STARTUP_SLACK, "took {elapsed:?}");
}

#[test]
fn until_deadline_exits_124() {
    let target = Reaped::sleep("30");
    let pid = target.pid().to_string();
    let cond = format!("pid-exited:{pid}");
    let (out, elapsed) = run(&["wait", "--until", &cond, "--deadline", "1"]);
    assert_eq!(out.status.code(), Some(124), "stderr: {}", stderr(&out));
    assert!(
        elapsed >= Duration::from_secs(1) && elapsed < Duration::from_secs(1) + STARTUP_SLACK,
        "took {elapsed:?}"
    );
    let err = stderr(&out);
    assert!(
        err.contains("amplihack wait: deadline of 1 s passed")
            && err.contains(&format!("pid {pid} still running")),
        "unexpected stderr: {err}"
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn invalid_arguments_exit_2() {
    let cases: &[&[&str]] = &[
        &["wait"],
        &[
            "wait",
            "--seconds",
            "1",
            "--until",
            "pid-exited:1",
            "--deadline",
            "5",
        ],
        &["wait", "--seconds", "-1"],
        &["wait", "--seconds", "abc"],
        &["wait", "--seconds", "86401"],
        &["wait", "--until", "pid-exited:1"],
        &["wait", "--deadline", "5"],
        &["wait", "--until", "pid-exited:1", "--deadline", "0"],
        &["wait", "--until", "pid-exited:1", "--deadline", "86401"],
        &["wait", "--until", "pid-exited:0", "--deadline", "5"],
        &["wait", "--until", "pid-exited:-1", "--deadline", "5"],
        &["wait", "--until", "pid-exited:abc", "--deadline", "5"],
        &["wait", "--until", "pid-exited:", "--deadline", "5"],
        &[
            "wait",
            "--until",
            "pid-exited:2147483648",
            "--deadline",
            "5",
        ],
        &["wait", "--until", "1234", "--deadline", "5"],
        &["wait", "--until", "file-exists:/x", "--deadline", "5"],
        &["wait", "--until", "PID-EXITED:1", "--deadline", "5"],
    ];
    for args in cases {
        let (out, elapsed) = run(args);
        // clap also exits 2 for an unknown subcommand; that must not pass.
        assert!(
            !stderr(&out).contains("unrecognized subcommand"),
            "{args:?}: the wait subcommand must exist; stderr: {}",
            stderr(&out)
        );
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?} must exit 2; stderr: {}",
            stderr(&out)
        );
        assert!(elapsed < STARTUP_SLACK, "{args:?} took {elapsed:?}");
    }
}

#[test]
fn unknown_condition_kind_names_the_supported_form() {
    let (out, _) = run(&["wait", "--until", "file-exists:/x", "--deadline", "5"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("pid-exited:<PID>"),
        "error must list the supported form: {}",
        stderr(&out)
    );
}

#[test]
fn help_mentions_flags() {
    let (out, _) = run(&["wait", "--help"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in ["--seconds", "--until", "--deadline", "pid-exited"] {
        assert!(help.contains(flag), "help must mention {flag}: {help}");
    }
}

#[test]
fn top_level_help_lists_wait() {
    let (out, _) = run(&["--help"]);
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(
        help.lines().any(|l| l.trim_start().starts_with("wait")),
        "top-level help must list the wait subcommand: {help}"
    );
}
