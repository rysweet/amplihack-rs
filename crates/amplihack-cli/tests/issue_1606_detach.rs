//! Issue #1606: `recipe run --detach`, `recipe stop`, and a signal to
//! `recipe run` ending the whole recipe tree.
//!
//! Black-box through the real binary, with a stub `recipe-runner-rs` that
//! records its pid and starts a child, the way the real runner starts agents.
//! Hermetic: `env_clear()`, a temp `HOME`, and a private `TMUX_TMPDIR`, so
//! the tests never touch the developer's tmux server (a guard kills the
//! private one). Linux only (`/proc`); skipped when tmux is not installed.

#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::socket_dir::{SUN_PATH_MAX, tmux_socket_dir, tmux_socket_fits};
use crate::support::wait::{CEILING, output_within, wait_child, wait_until};
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Records its pid, its environment and its child's pid, then waits.
const STUB_RUNNER: &str = r#"#!/bin/sh
case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
env > "$RECORD_DIR/env.tmp" && mv "$RECORD_DIR/env.tmp" "$RECORD_DIR/env.txt"
echo $$ > "$RECORD_DIR/runner.tmp" && mv "$RECORD_DIR/runner.tmp" "$RECORD_DIR/runner.pid"
sleep 300 &
echo $! > "$RECORD_DIR/child.tmp" && mv "$RECORD_DIR/child.tmp" "$RECORD_DIR/child.pid"
wait
"#;

const RECIPE: &str = r#"name: issue-1606-detach
description: long-running no-op
version: 1.0.0
context: {}
steps:
  - id: noop
    type: bash
    command: "true"
"#;

pub(crate) struct Fx {
    _tmp: tempfile::TempDir,
    /// Set only when the socket directory had to leave `TMPDIR`.
    _tmux_tmp: Option<tempfile::TempDir>,
    pub(crate) home: PathBuf,
    project: PathBuf,
    tools: PathBuf,
    pub(crate) records: PathBuf,
    tmux_dir: PathBuf,
    recipe: PathBuf,
}

impl Fx {
    pub(crate) fn new() -> Self {
        Self::with_runner(STUB_RUNNER)
    }

    pub(crate) fn with_runner(runner: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let (tmux_dir, tmux_tmp) = tmux_socket_dir(&root);
        let fx = Self {
            home: root.join("home"),
            project: root.join("project"),
            tools: root.join("tools"),
            records: root.join("records"),
            tmux_dir,
            recipe: root.join("issue-1606-detach.yaml"),
            _tmp: tmp,
            _tmux_tmp: tmux_tmp,
        };
        for dir in [&fx.home, &fx.project, &fx.tools, &fx.records, &fx.tmux_dir] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::create_dir_all(fx.project.join(".git")).unwrap();
        fs::write(
            fx.project.join(".git").join("HEAD"),
            "ref: refs/heads/main\n",
        )
        .unwrap();
        crate::support::exec_stub::write_executable(
            &fx.tools.join("recipe-runner-rs"),
            runner,
            0o755,
        );
        fs::write(&fx.recipe, RECIPE).unwrap();
        fx
    }

    pub(crate) fn cmd(&self, args: &[&str], env: &[(&str, &str)]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.project)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("TMUX_TMPDIR", &self.tmux_dir)
            .env("RECORD_DIR", &self.records)
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"))
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            // Issue #1973: the pane gets the wait ceiling to start the run on
            // a loaded host. Set before the per-call slice, so a test that
            // sets its own value (the start-timeout test) still wins.
            .env("AMPLIHACK_TEST_DETACH_START_SECS", "600")
            .stdin(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        c
    }

    pub(crate) fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        output_within("amplihack to exit", &mut self.cmd(args, env))
    }

    pub(crate) fn recipe_run_args(&self) -> Vec<String> {
        vec![
            "recipe".into(),
            "run".into(),
            self.recipe.to_str().unwrap().into(),
            "-w".into(),
            self.project.to_str().unwrap().into(),
        ]
    }

    /// False, with a message, when no socket directory short enough was
    /// found, so a test that starts a tmux server cannot run here. The
    /// directory comes from `support::socket_dir::tmux_socket_dir`: under
    /// `TMPDIR` when the socket fits, else under `XDG_RUNTIME_DIR`, never
    /// `/tmp`.
    pub(crate) fn tmux_socket_usable(&self) -> bool {
        let fits = tmux_socket_fits(&self.tmux_dir);
        if !fits {
            eprintln!(
                "skipping: the tmux socket under {} would exceed the {SUN_PATH_MAX}-byte \
                 sun_path limit and XDG_RUNTIME_DIR offers no shorter directory; \
                 run with a shorter TMPDIR",
                self.tmux_dir.display()
            );
        }
        fits
    }

    pub(crate) fn tmux(&self, args: &[&str]) -> Output {
        Command::new("tmux")
            .args(args)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("TMUX_TMPDIR", &self.tmux_dir)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    pub(crate) fn pid(&self, name: &str) -> i32 {
        let path = self.records.join(name);
        wait_for(|| path.is_file(), name);
        fs::read_to_string(path).unwrap().trim().parse().unwrap()
    }

    pub(crate) fn runner_env(&self, key: &str) -> Option<String> {
        let body = fs::read_to_string(self.records.join("env.txt")).unwrap();
        body.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")).map(str::to_string))
    }
}

impl Fx {
    /// Start the private tmux server as a Copilot session would have: its
    /// global environment holds COPILOT_CLI=1. Returns the `TMUX` value of a
    /// pane of that server.
    fn foreign_server(&self) -> String {
        assert!(
            self.tmux(&["new-session", "-d", "-s", "host"])
                .status
                .success()
        );
        assert!(
            self.tmux(&["set-environment", "-g", "COPILOT_CLI", "1"])
                .status
                .success()
        );
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        format!("{}/tmux-{uid}/default,1,0", self.tmux_dir.display())
    }

    pub(crate) fn log_dir(&self) -> PathBuf {
        self.home.join(".amplihack").join("recipe-logs")
    }

    /// Files in the log directory whose names end with `suffix`.
    fn leftovers(&self, suffix: &str) -> Vec<String> {
        fs::read_dir(self.log_dir())
            .map(|dir| {
                dir.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.ends_with(suffix))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
        // A failed test must not leave the stub runner or its child running.
        for name in ["runner.pid", "child.pid"] {
            if let Some(pid) = fs::read_to_string(self.records.join(name))
                .ok()
                .and_then(|text| text.trim().parse::<i32>().ok())
                && alive(pid)
            {
                // SAFETY: a process this test's own stub started.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

pub(crate) fn tmux_available() -> bool {
    let found = Command::new("tmux")
        .arg("-V")
        .output()
        .is_ok_and(|out| out.status.success());
    if !found {
        eprintln!("skipping: tmux is not installed");
    }
    found
}

/// Waits for `done` under the shared ceiling (issue #1973).
pub(crate) fn wait_for(done: impl FnMut() -> bool, what: &str) {
    wait_until(what, done);
}

/// Waits for amplihack to exit under the shared ceiling (issue #1973).
pub(crate) fn wait_bounded(child: &mut Child) -> std::process::ExitStatus {
    wait_child("amplihack to exit", child)
}

/// The delay of a `Retrying in {delay:?}.` line: `Duration`'s debug form,
/// a decimal number then `ns`, `µs`, `ms` or `s`. Panics with the line if
/// it cannot be read, so the check that uses it never passes vacuously.
fn retry_delay(line: &str) -> Duration {
    let rest = line
        .split("Retrying in ")
        .nth(1)
        .unwrap_or_else(|| panic!("no `Retrying in` in {line:?}"));
    let text = rest
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap_or_default()
        .trim_end_matches('.');
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or_else(|| panic!("no unit in the retry delay of {line:?}"));
    let (number, unit) = text.split_at(split);
    let number: f64 = number
        .parse()
        .unwrap_or_else(|e| panic!("cannot read the retry delay of {line:?}: {e}"));
    let secs = match unit {
        "s" => number,
        "ms" => number / 1e3,
        "µs" | "us" => number / 1e6,
        "ns" => number / 1e9,
        _ => panic!("unknown unit {unit:?} in the retry delay of {line:?}"),
    };
    Duration::from_secs_f64(secs)
}

/// Alive and not a zombie.
pub(crate) fn alive(pid: i32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z" && state != "X")
    })
}

pub(crate) fn assert_gone(pids: &[i32]) {
    wait_for(
        || pids.iter().all(|pid| !alive(*pid)),
        &format!("processes {pids:?} to exit"),
    );
}

pub(crate) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The bug in the issue: killing the tmux session (SIGHUP to `recipe run`)
/// left the setsid'd runner and its agent child running under PID 1.
#[test]
fn sighup_to_recipe_run_ends_the_runner_and_its_children() {
    let fx = Fx::new();
    let args = fx.recipe_run_args();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut amp = fx
        .cmd(&args, &[])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let runner = fx.pid("runner.pid");
    let child = fx.pid("child.pid");
    // SAFETY: signalling a child this test started.
    unsafe { libc::kill(amp.id() as i32, libc::SIGHUP) };
    let status = wait_bounded(&mut amp);
    assert_eq!(status.code(), Some(129), "exit status {status:?}");
    assert_gone(&[runner, child]);
}

#[test]
fn configured_detach_starts_a_tmux_run_that_stop_ends() {
    if !tmux_available() {
        return;
    }
    let fx = Fx::new();
    if !fx.tmux_socket_usable() {
        return;
    }
    let set = fx.run(&["config", "set", "recipe.detach", "tmux"], &[]);
    assert!(set.status.success(), "{}", text(&set.stderr));
    let set = fx.run(&["config", "set", "agent.binary", "claude"], &[]);
    assert!(set.status.success(), "{}", text(&set.stderr));

    let args = fx.recipe_run_args();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // Launched from a pane of a tmux server a Copilot session started: the
    // server's marker is in the pane's environment and its global one. And a
    // secret the run must still get.
    let tmux = fx.foreign_server();
    let out = fx.run(
        &args,
        &[
            ("TMUX", tmux.as_str()),
            ("AMPLIHACK_AGENT_BINARY", "copilot"),
            ("AMPLIHACK_AGENT_BINARY_SOURCE", "tmux_server:COPILOT_CLI"),
            ("COPILOT_CLI", "1"),
            ("ISSUE_1606_SECRET", "s3cr3t value 'quoted'"),
        ],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    let session = stdout
        .lines()
        .find_map(|l| l.strip_prefix("session: "))
        .unwrap_or_else(|| panic!("no session line: {stdout}"))
        .to_string();
    let log = PathBuf::from(
        stdout
            .lines()
            .find_map(|l| l.strip_prefix("log: "))
            .unwrap_or_else(|| panic!("no log line: {stdout}")),
    );
    assert!(
        session.starts_with("amp-issue-1606-detach-project-"),
        "{session}"
    );
    assert!(
        fx.tmux(&["has-session", "-t", &format!("={session}")])
            .status
            .success()
    );

    let runner = fx.pid("runner.pid");
    let child = fx.pid("child.pid");
    wait_for(|| fx.records.join("env.txt").is_file(), "runner env");
    // The configured CLI crossed into tmux, tagged, without the marker.
    assert_eq!(
        fx.runner_env("AMPLIHACK_AGENT_BINARY").as_deref(),
        Some("claude")
    );
    assert_eq!(
        fx.runner_env("AMPLIHACK_AGENT_BINARY_SOURCE").as_deref(),
        Some("user_config:claude")
    );
    assert_eq!(fx.runner_env("COPILOT_CLI"), None);
    assert_eq!(
        fx.runner_env("ISSUE_1606_SECRET").as_deref(),
        Some("s3cr3t value 'quoted'")
    );

    // Private log, private directory, spec file gone.
    let dir = log.parent().unwrap();
    assert_eq!(dir, fx.home.join(".amplihack").join("recipe-logs"));
    assert_eq!(
        fs::metadata(dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let leftovers: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".spec.json"))
        .collect();
    assert!(leftovers.is_empty(), "the run spec must be deleted");
    wait_for(
        || fs::read_to_string(&log).is_ok_and(|l| l.contains("amplihack.recipe.log_pointer")),
        "the log to receive the run's output",
    );

    let stop = fx.run(&["recipe", "stop", &session], &[]);
    assert!(stop.status.success(), "{}", text(&stop.stderr));
    assert!(text(&stop.stdout).contains(&format!("stopped session {session}")));
    assert_gone(&[runner, child]);
    assert!(
        !fx.tmux(&["has-session", "-t", &format!("={session}")])
            .status
            .success()
    );
}

#[test]
fn stop_by_run_id_ends_a_foreground_run() {
    let fx = Fx::new();
    let args = fx.recipe_run_args();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut amp = fx
        .cmd(&args, &[])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = amp.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some(json) = line.strip_prefix("amplihack.recipe.log_pointer ") {
                let event: serde_json::Value = serde_json::from_str(json).unwrap();
                if event["event"] == "early" {
                    let _ = tx.send(event["run_id"].as_str().unwrap().to_string());
                }
            }
        }
    });
    let run_id = rx.recv_timeout(CEILING).expect("early pointer");
    let runner = fx.pid("runner.pid");
    let child = fx.pid("child.pid");

    let stop = fx.run(&["recipe", "stop", &run_id], &[]);
    assert!(stop.status.success(), "{}", text(&stop.stderr));
    assert_gone(&[runner, child]);
    let status = wait_bounded(&mut amp);
    assert!(!status.success(), "a stopped run must not report success");
}

#[test]
fn stop_of_an_unknown_target_fails() {
    let fx = Fx::new();
    let out = fx.run(&["recipe", "stop", "no-such-run"], &[]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("no-such-run"),
        "{}",
        text(&out.stderr)
    );
}

/// Fails every attempt with a transient (retryable) transport error.
const TRANSIENT_RUNNER: &str = r#"#!/bin/sh
case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
echo $$ > "$RECORD_DIR/runner.tmp" && mv "$RECORD_DIR/runner.tmp" "$RECORD_DIR/runner.pid"
echo attempt >> "$RECORD_DIR/attempts"
echo "API Error: 529 overloaded_error" >&2
printf '%s\n' '{"recipe_name":"issue-1606-detach","success":false,"step_results":[],"context":{}}'
exit 1
"#;

/// Review of #1648: a Ctrl-C during retry backoff (10 s and up) ends the run
/// at once; it is not swallowed, and no new runner is started.
#[test]
fn a_signal_during_retry_backoff_stops_the_run_without_another_attempt() {
    let fx = Fx::with_runner(TRANSIENT_RUNNER);
    let args = fx.recipe_run_args();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut amp = fx
        .cmd(&args, &[])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = amp.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            // The receiver is gone once the test has its line.
            if line.contains("Retrying in") && tx.send(line).is_err() {
                return;
            }
        }
    });
    let line = rx
        .recv_timeout(CEILING)
        .expect("the first attempt was not retried");
    let delay = retry_delay(&line);
    let started = Instant::now();
    // SAFETY: signalling a child this test started.
    unsafe { libc::kill(amp.id() as i32, libc::SIGINT) };
    let status = wait_bounded(&mut amp);
    // Issue #1973: measured against the backoff the run announced, not a
    // fixed 5 s. A run that slept out its backoff before acting on the
    // signal takes at least `delay`.
    assert!(
        started.elapsed() < delay,
        "took {:?} to stop, not less than the {delay:?} backoff",
        started.elapsed()
    );
    assert_eq!(status.code(), Some(130), "{status:?}");
    let attempts = fs::read_to_string(fx.records.join("attempts")).unwrap();
    assert_eq!(
        attempts.lines().count(),
        1,
        "a runner was started after the signal"
    );
}

/// Review of #1648: a pattern without `{ts}` reuses a name whose log is left
/// over from an earlier run; the next launch takes the next free name.
#[test]
fn a_session_pattern_without_ts_keeps_working_across_runs() {
    if !tmux_available() {
        return;
    }
    let fx = Fx::new();
    if !fx.tmux_socket_usable() {
        return;
    }
    let set = fx.run(
        &["config", "set", "recipe.tmux_session_pattern", "fixed"],
        &[],
    );
    assert!(set.status.success(), "{}", text(&set.stderr));
    let mut args = fx.recipe_run_args();
    args.push("--detach".into());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let session_of = |out: &Output| {
        text(&out.stdout)
            .lines()
            .find_map(|l| l.strip_prefix("session: ").map(str::to_string))
            .unwrap_or_else(|| panic!("no session: {}", text(&out.stderr)))
    };
    let first = fx.run(&args, &[]);
    assert!(first.status.success(), "{}", text(&first.stderr));
    assert_eq!(session_of(&first), "fixed");
    fx.pid("child.pid");
    let stop = fx.run(&["recipe", "stop", "fixed"], &[]);
    assert!(stop.status.success(), "{}", text(&stop.stderr));
    assert!(fx.log_dir().join("fixed.log").is_file());

    let second = fx.run(&args, &[]);
    assert!(second.status.success(), "{}", text(&second.stderr));
    assert_eq!(session_of(&second), "fixed-2");
    let stop = fx.run(&["recipe", "stop", "fixed-2"], &[]);
    assert!(stop.status.success(), "{}", text(&stop.stderr));
}

#[test]
fn detach_without_tmux_fails_and_leaves_nothing_behind() {
    let fx = Fx::new();
    let mut args = fx.recipe_run_args();
    args.push("--detach".into());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // Only the stub runner on PATH: no tmux.
    let out = fx.run(&args, &[("PATH", fx.tools.to_str().unwrap())]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("needs tmux"),
        "{}",
        text(&out.stderr)
    );
    assert!(fx.leftovers(".spec.json").is_empty());
    assert!(fx.leftovers(".context.json").is_empty());
}

/// A tmux that accepts `new-session` but never starts the pane.
const SILENT_TMUX: &str = r#"#!/bin/sh
case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
case "$1" in
  -V) echo "tmux 3.4" ;;
  new-session|kill-session) exit 0 ;;
  *) exit 1 ;;
esac
"#;

/// The spec holds the caller's environment: when the pane never picks it up,
/// the launch fails and removes it (and the context file).
#[test]
fn a_spec_the_pane_never_reads_is_removed() {
    let fx = Fx::new();
    crate::support::exec_stub::write_executable(&fx.tools.join("tmux"), SILENT_TMUX, 0o755);
    let mut args = fx.recipe_run_args();
    args.extend([
        "--detach".into(),
        "-c".into(),
        "secret_context=hunter2".into(),
    ]);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = fx.run(&args, &[("AMPLIHACK_TEST_DETACH_START_SECS", "1")]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("did not start"),
        "{}",
        text(&out.stderr)
    );
    assert!(fx.leftovers(".spec.json").is_empty(), "spec left behind");
    assert!(
        fx.leftovers(".context.json").is_empty(),
        "context left behind"
    );
}

/// Review of #1648: `recipe stop` refuses a tmux session it did not start.
#[test]
fn stop_refuses_a_session_it_did_not_start() {
    if !tmux_available() {
        return;
    }
    let fx = Fx::new();
    if !fx.tmux_socket_usable() {
        return;
    }
    assert!(
        fx.tmux(&["new-session", "-d", "-s", "someone-elses", "sleep", "300"])
            .status
            .success()
    );
    let out = fx.run(&["recipe", "stop", "someone-elses"], &[]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("was not started by `amplihack recipe run --detach`"),
        "{}",
        text(&out.stderr)
    );
    assert!(
        fx.tmux(&["has-session", "-t", "=someone-elses"])
            .status
            .success()
    );
}

/// SIGKILL to `recipe run` cannot be caught; the kernel then sends the runner
/// SIGTERM (PR_SET_PDEATHSIG).
#[test]
fn a_sigkilled_recipe_run_takes_its_runner_with_it() {
    let fx = Fx::new();
    let args = fx.recipe_run_args();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut amp = fx
        .cmd(&args, &[])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let runner = fx.pid("runner.pid");
    fx.pid("child.pid");
    amp.kill().unwrap();
    amp.wait().unwrap();
    assert_gone(&[runner]);
}

/// Starts a child that leaves the runner's process group and session.
const SETSID_RUNNER: &str = r#"#!/bin/sh
case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
echo $$ > "$RECORD_DIR/runner.tmp" && mv "$RECORD_DIR/runner.tmp" "$RECORD_DIR/runner.pid"
setsid sleep 300 &
echo $! > "$RECORD_DIR/escaped.tmp" && mv "$RECORD_DIR/escaped.tmp" "$RECORD_DIR/escaped.pid"
sleep 300 &
echo $! > "$RECORD_DIR/child.tmp" && mv "$RECORD_DIR/child.tmp" "$RECORD_DIR/child.pid"
wait
"#;

/// Review of #1648: a descendant that setsid'd out of the runner's group is
/// still ended when `recipe run` is stopped.
#[test]
fn a_stop_also_ends_a_descendant_that_left_the_group() {
    let fx = Fx::with_runner(SETSID_RUNNER);
    let args = fx.recipe_run_args();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut amp = fx
        .cmd(&args, &[])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let runner = fx.pid("runner.pid");
    let escaped = fx.pid("escaped.pid");
    let child = fx.pid("child.pid");
    // SAFETY: signalling a child this test started.
    unsafe { libc::kill(amp.id() as i32, libc::SIGHUP) };
    let status = wait_bounded(&mut amp);
    assert_eq!(status.code(), Some(129), "{status:?}");
    let result = std::panic::catch_unwind(|| assert_gone(&[runner, child, escaped]));
    if alive(escaped) {
        // SAFETY: a process this test's stub started.
        unsafe { libc::kill(escaped, libc::SIGKILL) };
    }
    result.unwrap();
}

/// Reports success, then signals its parent (`recipe run`) and exits at once:
/// the signal and the exit race.
const SIGNAL_AT_EXIT_RUNNER: &str = r#"#!/bin/sh
case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
printf '%s\n' '{"recipe_name":"issue-1606-detach","success":true,"step_results":[],"context":{}}'
kill -TERM $PPID
exit 0
"#;

/// Review of #1648: a stop signal that arrives as the runner exits ends in a
/// stop, never in success. Which side of the race the wait loop sees first is
/// not deterministic; both must give 143, and that is what is asserted.
#[test]
fn a_signal_as_the_runner_exits_is_still_a_stop() {
    let fx = Fx::with_runner(SIGNAL_AT_EXIT_RUNNER);
    let args = fx.recipe_run_args();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    for _ in 0..3 {
        let out = fx.run(&args, &[]);
        assert_eq!(out.status.code(), Some(143), "{}", text(&out.stderr));
    }
}

/// Review of #1648: when the pane cannot start the run, the launch says why.
/// The tmux wrapper moves the caller's directory away just before the pane
/// starts, so starting the run in it fails.
#[test]
fn a_pane_start_up_error_is_reported_by_the_launch() {
    if !tmux_available() {
        return;
    }
    let real_tmux = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v tmux"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let fx = Fx::new();
    if !fx.tmux_socket_usable() {
        return;
    }
    let caller = fx.records.join("caller");
    fs::create_dir_all(&caller).unwrap();
    crate::support::exec_stub::write_executable(
        &fx.tools.join("tmux"),
        format!(
            "#!/bin/sh\ncase \"$*\" in --capabilities) printf '%b\\n' '\\173\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]\\175'; exit 0 ;; esac\nif [ \"$1\" = new-session ]; then mv \"$CALLER_DIR\" \"$CALLER_DIR.moved\"; fi\nexec {real_tmux} \"$@\"\n"
        ),
        0o755,
    );
    let mut args = fx.recipe_run_args();
    args.push("--detach".into());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = fx
        .cmd(&args, &[("CALLER_DIR", caller.to_str().unwrap())])
        .current_dir(&caller)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", text(&out.stdout));
    let err = text(&out.stderr);
    assert!(err.contains("the detached run could not start"), "{err}");
    assert!(err.contains("failed to start recipe run in"), "{err}");
    assert!(fx.leftovers(".spec.json").is_empty());
    assert!(fx.leftovers(".context.json").is_empty());
}

// --- issue #2175: stopping a run by its run id through its record ---------------

/// The `run_id` of the first `early` log pointer in `line`, if it has one.
fn early_run_id(line: &str) -> Option<String> {
    let json = line.split_once("amplihack.recipe.log_pointer ")?.1;
    let event: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    (event["event"] == "early").then(|| event["run_id"].as_str().map(str::to_string))?
}

impl Fx {
    /// Starts `recipe run` in the foreground and returns it with the run id
    /// of its `early` log pointer line, read from its stderr.
    pub(crate) fn spawn_foreground_run(&self) -> (Child, String) {
        let args = self.recipe_run_args();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut amp = self
            .cmd(&args, &[])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = amp.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut sent = false;
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !sent && let Some(id) = early_run_id(&line) {
                    let _ = tx.send(id);
                    sent = true;
                }
            }
        });
        let run_id = rx.recv_timeout(CEILING).expect("early pointer");
        (amp, run_id)
    }

    /// `<work root>/runs` of this fixture: `HOME/.amplihack/work/runs`.
    pub(crate) fn runs_dir(&self) -> PathBuf {
        self.home.join(".amplihack").join("work").join("runs")
    }

    /// Every `runs/*/procs/<run_id>.json` under the fixture's work root.
    fn records_of(&self, run_id: &str) -> Vec<PathBuf> {
        fs::read_dir(self.runs_dir())
            .map(|dir| {
                dir.filter_map(Result::ok)
                    .map(|e| e.path().join("procs").join(format!("{run_id}.json")))
                    .filter(|p| p.is_file())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Writes a record by hand, as `recipe run` would, in a run directory of
    /// its own. Returns the record's path.
    fn plant_record(&self, run_id: &str, body: &str) -> PathBuf {
        let procs = self.runs_dir().join("planted-run-dir").join("procs");
        fs::create_dir_all(&procs).unwrap();
        for dir in [
            self.runs_dir(),
            self.runs_dir().join("planted-run-dir"),
            procs.clone(),
        ] {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let path = procs.join(format!("{run_id}.json"));
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
}

/// Field 22 of `/proc/<pid>/stat`: the start time in clock ticks.
fn start_ticks(pid: u32) -> u64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let rest = stat.rsplit_once(')').unwrap().1;
    rest.split_whitespace().nth(19).unwrap().parse().unwrap()
}

/// A `sleep` this test owns; killed and reaped when dropped.
struct Sleeper(Child);

impl Sleeper {
    /// `own_group`: the sleep leads its own process group, like the runner.
    fn start(own_group: bool) -> Self {
        use std::os::unix::process::CommandExt;
        let mut c = Command::new("sleep");
        c.arg("300").stdin(Stdio::null());
        if own_group {
            c.process_group(0);
        }
        Self(c.spawn().unwrap())
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    /// Still running: not exited and not yet reaped.
    fn running(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn record_body(run_id: &str, recipe_run: (u32, u64), runner: (u32, u64)) -> String {
    serde_json::json!({
        "version": 1,
        "run_id": run_id,
        "recipe_run": {"pid": recipe_run.0, "start_ticks": recipe_run.1},
        "runner": {"pid": runner.0, "pgid": runner.0, "start_ticks": runner.1},
    })
    .to_string()
}

/// The record exists as soon as the `early` line is printed, is private, names
/// exactly this run's processes, and is removed when the attempt ends.
#[test]
fn a_running_run_has_a_private_record_that_names_its_processes() {
    let fx = Fx::new();
    let (mut amp, run_id) = fx.spawn_foreground_run();
    // No wait: the record is written before the early line.
    let records = fx.records_of(&run_id);
    assert_eq!(
        records.len(),
        1,
        "one record for run {run_id} under {}",
        fx.runs_dir().display()
    );
    let record = &records[0];
    let procs = record.parent().unwrap();
    assert_eq!(
        fs::symlink_metadata(procs).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::symlink_metadata(record).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let runner = fx.pid("runner.pid");
    let child = fx.pid("child.pid");
    let value: serde_json::Value = serde_json::from_slice(&fs::read(record).unwrap()).unwrap();
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["recipe_run", "run_id", "runner", "version"],
        "{value}"
    );
    assert_eq!(value["version"], 1, "{value}");
    assert_eq!(value["run_id"], run_id.as_str(), "{value}");
    assert_eq!(value["recipe_run"]["pid"], amp.id(), "{value}");
    assert_eq!(
        value["recipe_run"]["start_ticks"],
        start_ticks(amp.id()),
        "{value}"
    );
    assert_eq!(value["runner"]["pid"], runner, "{value}");
    assert_eq!(value["runner"]["pgid"], runner, "{value}");
    assert_eq!(
        value["runner"]["start_ticks"],
        start_ticks(runner as u32),
        "{value}"
    );

    let stop = fx.run(&["recipe", "stop", &run_id], &[]);
    assert!(stop.status.success(), "{}", text(&stop.stderr));
    assert!(
        text(&stop.stdout).contains(&format!("stopped run {run_id}")),
        "{}",
        text(&stop.stdout)
    );
    assert_gone(&[runner, child]);
    let status = wait_bounded(&mut amp);
    assert!(!status.success(), "a stopped run must not report success");
    assert!(
        fx.records_of(&run_id).is_empty(),
        "the attempt's record must be removed when it ends"
    );
}

/// A detached run is stopped by its run id as well as by its session.
#[test]
fn stop_by_run_id_ends_a_detached_run() {
    if !tmux_available() {
        return;
    }
    let fx = Fx::new();
    if !fx.tmux_socket_usable() {
        return;
    }
    let mut args = fx.recipe_run_args();
    args.push("--detach".into());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = fx.run(&args, &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    let log = PathBuf::from(
        stdout
            .lines()
            .find_map(|l| l.strip_prefix("log: "))
            .unwrap_or_else(|| panic!("no log line: {stdout}")),
    );
    let mut run_id = None;
    wait_for(
        || {
            run_id = fs::read_to_string(&log)
                .ok()
                .and_then(|body| body.lines().find_map(early_run_id));
            run_id.is_some()
        },
        "the detached run's early pointer",
    );
    let run_id = run_id.unwrap();
    let runner = fx.pid("runner.pid");
    let child = fx.pid("child.pid");

    let stop = fx.run(&["recipe", "stop", &run_id], &[]);
    assert!(stop.status.success(), "{}", text(&stop.stderr));
    assert!(
        text(&stop.stdout).contains(&format!("stopped run {run_id}")),
        "{}",
        text(&stop.stdout)
    );
    assert_gone(&[runner, child]);
}

#[test]
fn stop_of_an_unknown_run_quotes_its_id() {
    let fx = Fx::new();
    let out = fx.run(&["recipe", "stop", "no-such-run"], &[]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr)
            .contains(r#"recipe stop: no tmux session or running recipe run named "no-such-run""#),
        "{}",
        text(&out.stderr)
    );
}

/// A `runs/` folder `stop` cannot read is "not found" with the reason
/// underneath, and nothing is signalled. Root reads any folder, so there is
/// nothing to check as root.
#[test]
fn stop_reports_why_it_could_not_read_the_runs_folder() {
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let fx = Fx::new();
    let id = "unreadable-run";
    fx.plant_record(id, "{}");
    let runs = fx.runs_dir();
    fs::set_permissions(&runs, fs::Permissions::from_mode(0o300)).unwrap();
    let out = fx.run(&["recipe", "stop", id], &[]);
    fs::set_permissions(&runs, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!out.status.success(), "{}", text(&out.stdout));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(
            r#"recipe stop: no tmux session or running recipe run named "unreadable-run""#
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("cannot list {}", runs.display()))
            && stderr.contains("ermission denied"),
        "the reason must be shown: {stderr}"
    );
}

/// An id that could name another path is refused before any record is read.
#[test]
fn stop_refuses_an_id_that_could_name_a_path() {
    let fx = Fx::new();
    for id in ["../x", "a/b", ".hidden", "a b", "a\u{1b}[31m"] {
        let out = fx.run(&["recipe", "stop", id], &[]);
        assert!(!out.status.success(), "{id:?} must be refused");
        let expected = format!("recipe stop: invalid run id {id:?}");
        assert!(
            text(&out.stderr).contains(&expected),
            "{id:?}: expected {expected:?} in {}",
            text(&out.stderr)
        );
        assert!(
            !text(&out.stderr).contains('\u{1b}'),
            "control characters must be printed escaped"
        );
    }
}

/// A process inside a run (it inherits `AMPLIHACK_RECIPE_RUN_ID`) cannot stop
/// that run by its id, and the run keeps going.
#[test]
fn stop_refuses_to_stop_the_run_it_runs_inside() {
    let fx = Fx::new();
    let (mut amp, run_id) = fx.spawn_foreground_run();
    let runner = fx.pid("runner.pid");
    let child = fx.pid("child.pid");

    let out = fx.run(
        &["recipe", "stop", &run_id],
        &[("AMPLIHACK_RECIPE_RUN_ID", run_id.as_str())],
    );
    assert!(!out.status.success(), "{}", text(&out.stdout));
    let expected = format!("recipe stop: refusing to stop run {run_id:?} from inside it");
    assert!(
        text(&out.stderr).contains(&expected),
        "expected {expected:?} in {}",
        text(&out.stderr)
    );
    assert!(alive(runner) && alive(child), "nothing may be signalled");
    assert!(
        amp.try_wait().unwrap().is_none(),
        "recipe run must keep running"
    );

    let stop = fx.run(&["recipe", "stop", &run_id], &[]);
    assert!(stop.status.success(), "{}", text(&stop.stderr));
    assert_gone(&[runner, child]);
    wait_bounded(&mut amp);
}

/// A record found by its run id is acted on through its pids and the
/// runner's process group, with no other lookup.
#[test]
fn stop_signals_the_processes_a_record_names() {
    let fx = Fx::new();
    let mut recipe_run = Sleeper::start(false);
    let mut runner = Sleeper::start(true);
    let id = "planted-run";
    fx.plant_record(
        id,
        &record_body(
            id,
            (recipe_run.pid(), start_ticks(recipe_run.pid())),
            (runner.pid(), start_ticks(runner.pid())),
        ),
    );
    let out = fx.run(&["recipe", "stop", id], &[]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains(&format!("stopped run {id}")),
        "{}",
        text(&out.stdout)
    );
    let a = wait_child("the recorded recipe run to exit", &mut recipe_run.0);
    let b = wait_child("the recorded runner to exit", &mut runner.0);
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(a.signal(), Some(libc::SIGTERM), "{a:?}");
    assert_eq!(b.signal(), Some(libc::SIGTERM), "{b:?}");
}

/// A record whose start times no longer match (a reused pid, or a record left
/// by a SIGKILLed run) is not acted on.
#[test]
fn stop_leaves_alone_processes_whose_start_time_differs() {
    let fx = Fx::new();
    let mut recipe_run = Sleeper::start(false);
    let mut runner = Sleeper::start(true);
    let id = "stale-run";
    fx.plant_record(
        id,
        &record_body(
            id,
            (recipe_run.pid(), start_ticks(recipe_run.pid()) + 1),
            (runner.pid(), start_ticks(runner.pid()) + 1),
        ),
    );
    let out = fx.run(&["recipe", "stop", id], &[]);
    assert!(!out.status.success(), "{}", text(&out.stdout));
    assert!(
        text(&out.stderr)
            .contains(r#"recipe stop: no tmux session or running recipe run named "stale-run""#),
        "{}",
        text(&out.stderr)
    );
    assert!(
        recipe_run.running() && runner.running(),
        "nothing may be signalled"
    );
}

/// A record others could have written is not trusted.
#[test]
fn stop_ignores_a_record_writable_by_others() {
    let fx = Fx::new();
    let mut recipe_run = Sleeper::start(false);
    let mut runner = Sleeper::start(true);
    let id = "open-record";
    let path = fx.plant_record(
        id,
        &record_body(
            id,
            (recipe_run.pid(), start_ticks(recipe_run.pid())),
            (runner.pid(), start_ticks(runner.pid())),
        ),
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    let out = fx.run(&["recipe", "stop", id], &[]);
    assert!(!out.status.success(), "{}", text(&out.stdout));
    assert!(
        text(&out.stderr).contains(&format!("{id:?}")),
        "{}",
        text(&out.stderr)
    );
    assert!(
        recipe_run.running() && runner.running(),
        "nothing may be signalled"
    );
}
