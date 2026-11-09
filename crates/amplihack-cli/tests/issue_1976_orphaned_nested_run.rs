//! Issue #1976 (R8, D8-D10, AC7): a nested `amplihack recipe run` whose
//! parent `recipe run` exits stops itself with exit 75 and leaves an orphan
//! record, instead of running on (and pushing) with nobody watching.
//!
//! Real `amplihack` binary; the recipe runner is a stub (through
//! `RECIPE_RUNNER_RS_PATH`) that records its pid and environment, writes a
//! readiness file, then blocks reading a FIFO nobody writes. A blocked `cat`
//! stands in for the parent `recipe run`: this test reads its pid and start
//! time (field 22 of `/proc/<pid>/stat`), exports them to the nested run,
//! kills it and reaps it. The test then `wait()`s on the nested run's exit.
//!
//! No sleeps and no wall-clock windows: the only waits are the readiness file
//! (shared condition wait, issue #1973) and child exits. Hermetic:
//! `env_clear()`, a temp `HOME`. Every process killed here was started here.
//! Linux only (`pidfd_open`, `/proc`).

#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::wait::{output_within, wait_child, wait_until};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};

const ORPHAN_EXIT: i32 = 75;
const LOG_POINTER_PREFIX: &str = "amplihack.recipe.log_pointer ";

/// Records pid and env, signals readiness, then blocks on the FIFO.
const BLOCKING_RUNNER: &str = r#"#!/bin/sh
env > "$RECORD_DIR/env.tmp" && mv "$RECORD_DIR/env.tmp" "$RECORD_DIR/env.txt"
echo $$ > "$RECORD_DIR/runner.tmp" && mv "$RECORD_DIR/runner.tmp" "$RECORD_DIR/runner.pid"
: > "$RECORD_DIR/ready"
read line < "$RECORD_DIR/fifo"
printf '%s\n' '{"recipe_name":"issue-1976-orphan","success":true,"step_results":[],"context":{}}'
"#;

/// Records the parent variables it was given, and the stat and cmdline of
/// the process they name, then reports success.
const RECORDING_RUNNER: &str = r#"#!/bin/sh
env > "$RECORD_DIR/env.tmp" && mv "$RECORD_DIR/env.tmp" "$RECORD_DIR/env.txt"
echo $$ > "$RECORD_DIR/runner.pid"
if [ -n "${AMPLIHACK_PARENT_RECIPE_PID:-}" ]; then
  cat "/proc/$AMPLIHACK_PARENT_RECIPE_PID/stat" > "$RECORD_DIR/parent.stat"
  tr '\0' ' ' < "/proc/$AMPLIHACK_PARENT_RECIPE_PID/cmdline" > "$RECORD_DIR/parent.cmdline"
fi
printf '%s\n' '{"recipe_name":"issue-1976-orphan","success":true,"step_results":[],"context":{}}'
"#;

const RECIPE: &str = r#"name: issue-1976-orphan
description: one step that blocks until released
version: 1.0.0
context: {}
steps:
  - id: block
    type: bash
    command: "true"
"#;

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    tools: PathBuf,
    records: PathBuf,
    state: PathBuf,
    recipe: PathBuf,
    /// Processes this test started and must not leave running.
    spawned: std::cell::RefCell<Vec<i32>>,
}

impl Fx {
    fn new(runner: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let fx = Self {
            home: root.join("home"),
            tools: root.join("tools"),
            records: root.join("records"),
            state: root.join("state"),
            recipe: root.join("issue-1976-orphan.yaml"),
            root,
            _tmp: tmp,
            spawned: std::cell::RefCell::new(Vec::new()),
        };
        for d in [&fx.home, &fx.tools, &fx.records, &fx.state] {
            fs::create_dir_all(d).unwrap();
        }
        fs::set_permissions(&fx.state, fs::Permissions::from_mode(0o700)).unwrap();
        let status = Command::new("mkfifo")
            .arg(fx.records.join("fifo"))
            .status()
            .expect("mkfifo");
        assert!(status.success());
        crate::support::exec_stub::write_executable(
            &fx.tools.join("recipe-runner-rs"),
            runner,
            0o755,
        );
        fs::write(&fx.recipe, RECIPE).unwrap();
        fx
    }

    fn cmd(&self, parent: Option<(&str, &str)>) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(["recipe", "run"])
            .arg(&self.recipe)
            .arg("-w")
            .arg(&self.root)
            .current_dir(&self.root)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("RECORD_DIR", &self.records)
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"))
            .env("AMPLIHACK_AUTODRIVE_STATE_DIR", &self.state)
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .stdin(Stdio::null());
        if let Some((pid, start)) = parent {
            c.env("AMPLIHACK_PARENT_RECIPE_PID", pid)
                .env("AMPLIHACK_PARENT_RECIPE_STARTTIME", start);
        }
        c
    }

    /// A stand-in parent that blocks on its stdin pipe until killed.
    fn stand_in_parent(&self) -> Child {
        let child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn stand-in parent");
        self.spawned.borrow_mut().push(child.id() as i32);
        child
    }

    fn orphan_record(&self) -> Option<serde_json::Value> {
        let text = fs::read_to_string(self.state.join("orphaned.json")).ok()?;
        Some(serde_json::from_str(&text).expect("orphaned.json is JSON"))
    }

    fn runner_pid(&self) -> Option<i32> {
        fs::read_to_string(self.records.join("runner.pid"))
            .ok()
            .and_then(|t| t.trim().parse().ok())
    }

    fn runner_env(&self, key: &str) -> Option<String> {
        let body = fs::read_to_string(self.records.join("env.txt")).ok()?;
        body.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")).map(str::to_string))
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let mut pids = self.spawned.borrow().clone();
        pids.extend(self.runner_pid());
        for pid in pids {
            if alive(pid) {
                // SAFETY: a process this test (or its stub runner) started.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

fn starttime_of(pid: u32) -> u64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    starttime_in(&stat)
}

fn starttime_in(stat: &str) -> u64 {
    stat.rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap()
}

/// Alive and not a zombie.
fn alive(pid: i32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z" && state != "X")
    })
}

fn kill_and_reap(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn final_record(stderr: &str) -> serde_json::Value {
    stderr
        .lines()
        .filter_map(|l| l.strip_prefix(LOG_POINTER_PREFIX))
        .filter_map(|j| serde_json::from_str::<serde_json::Value>(j).ok())
        .find(|v| v["event"] == "final")
        .unwrap_or_else(|| panic!("no final log pointer record in stderr:\n{stderr}"))
}

fn assert_orphaned(fx: &Fx, status: ExitStatus, stderr: &str, parent_pid: u32) {
    assert_eq!(
        status.code(),
        Some(ORPHAN_EXIT),
        "an orphaned nested run exits 75\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "orphaned: parent recipe run {parent_pid} exited; stopping"
        )),
        "stderr names the orphan stop:\n{stderr}"
    );
    let fin = final_record(stderr);
    assert_eq!(fin["status"], "orphaned", "{fin}");
    assert_eq!(fin["parent_pid"], parent_pid, "{fin}");
    assert_eq!(fin["reason"], "parent recipe run exited", "{fin}");

    let record = fx.orphan_record().expect("orphaned.json in the state dir");
    assert_eq!(record["parent_pid"], parent_pid, "{record}");
    assert_eq!(record["reason"], "parent recipe run exited", "{record}");
    let meta = fs::symlink_metadata(fx.state.join("orphaned.json")).unwrap();
    assert_eq!(meta.mode() & 0o777, 0o600, "orphaned.json must be 0600");
}

/// Case 1: the parent is killed while the step runs.
#[test]
fn a_nested_run_whose_parent_dies_mid_step_stops_with_exit_75() {
    let fx = Fx::new(BLOCKING_RUNNER);
    let parent = fx.stand_in_parent();
    let parent_pid = parent.id();
    let start = starttime_of(parent_pid).to_string();
    let pid_text = parent_pid.to_string();

    let stderr_path = fx.root.join("nested.stderr");
    let mut nested = fx
        .cmd(Some((&pid_text, &start)))
        .stdout(Stdio::null())
        .stderr(fs::File::create(&stderr_path).unwrap())
        .spawn()
        .expect("spawn nested recipe run");
    fx.spawned.borrow_mut().push(nested.id() as i32);

    let ready = fx.records.join("ready");
    wait_until("the step to signal readiness", || ready.is_file());
    let runner = fx.runner_pid().expect("runner pid recorded");
    assert!(
        fx.orphan_record().is_none(),
        "no orphan record while the parent lives"
    );

    kill_and_reap(parent);
    let status = wait_child("the nested run to stop", &mut nested);
    let stderr = fs::read_to_string(&stderr_path).unwrap();

    assert_orphaned(&fx, status, &stderr, parent_pid);
    assert!(
        !alive(runner),
        "the runner's process group must be torn down (pid {runner})"
    );
}

/// Case 2: the parent is already gone when the nested run starts.
#[test]
fn a_nested_run_whose_parent_is_already_dead_never_starts_the_runner() {
    let fx = Fx::new(RECORDING_RUNNER);
    // A runner that exits at once: if the check is missing, the run ends and
    // the assertions fail at once instead of blocking on the FIFO.
    let parent = fx.stand_in_parent();
    let parent_pid = parent.id();
    let start = starttime_of(parent_pid).to_string();
    kill_and_reap(parent);

    let out = output_within(
        "the nested run to exit",
        &mut fx.cmd(Some((&parent_pid.to_string(), &start))),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_orphaned(&fx, out.status, &stderr, parent_pid);
    assert!(
        fx.runner_pid().is_none(),
        "no runner may start under a dead parent"
    );
    // The pointer contract holds without a runner: one early, one final,
    // sharing a run_id.
    let fin = final_record(&stderr);
    let early = stderr
        .lines()
        .filter_map(|l| l.strip_prefix(LOG_POINTER_PREFIX))
        .filter_map(|j| serde_json::from_str::<serde_json::Value>(j).ok())
        .find(|v| v["event"] == "early")
        .unwrap_or_else(|| panic!("no early log pointer record in stderr:\n{stderr}"));
    assert_eq!(early["run_id"], fin["run_id"], "early {early} final {fin}");
}

/// Case 3: the PID is alive but its start time does not match (PID reuse).
#[test]
fn a_reused_parent_pid_is_treated_as_gone() {
    let fx = Fx::new(RECORDING_RUNNER);
    // A runner that exits at once: if the check is missing, the run ends and
    // the assertions fail at once instead of blocking on the FIFO.
    let parent = fx.stand_in_parent();
    let parent_pid = parent.id();
    let wrong = (starttime_of(parent_pid) + 1).to_string();

    let out = output_within(
        "the nested run to exit",
        &mut fx.cmd(Some((&parent_pid.to_string(), &wrong))),
    );
    let still_alive = alive(parent_pid as i32);
    kill_and_reap(parent);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(still_alive, "the stand-in must outlive the check");
    assert_orphaned(&fx, out.status, &stderr, parent_pid);
    assert!(fx.runner_pid().is_none(), "no runner may start");
}

/// Every `recipe run` exports its own pid and start time to its runner
/// child, so a nested run started by a step can watch it.
#[test]
fn a_run_exports_its_pid_and_start_time_to_the_runner() {
    let fx = Fx::new(RECORDING_RUNNER);
    let out = output_within("the run to exit", &mut fx.cmd(None));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{stderr}");

    let pid = fx
        .runner_env("AMPLIHACK_PARENT_RECIPE_PID")
        .unwrap_or_else(|| panic!("runner env lacks AMPLIHACK_PARENT_RECIPE_PID\n{stderr}"));
    let start: u64 = fx
        .runner_env("AMPLIHACK_PARENT_RECIPE_STARTTIME")
        .expect("runner env lacks AMPLIHACK_PARENT_RECIPE_STARTTIME")
        .parse()
        .expect("start time is a number");
    assert!(pid.parse::<i32>().expect("pid is a number") > 1);

    let stat = fs::read_to_string(fx.records.join("parent.stat")).unwrap();
    assert_eq!(starttime_in(&stat), start, "start time matches /proc stat");
    let cmdline = fs::read_to_string(fx.records.join("parent.cmdline")).unwrap();
    assert!(
        cmdline.contains("recipe") && cmdline.contains("run"),
        "the exported pid is the `recipe run` process: {cmdline}"
    );
    // The fixture sets AMPLIHACK_AUTODRIVE_STATE_DIR on `recipe run`; only
    // that level may write orphaned.json, so the runner never inherits it.
    assert_eq!(
        fx.runner_env("AMPLIHACK_AUTODRIVE_STATE_DIR"),
        None,
        "the state dir is removed from the runner's environment"
    );
    assert!(
        fx.orphan_record().is_none(),
        "a top-level run is never orphaned"
    );
}

/// Bad parent values warn and run as a top-level run; they never stop it.
#[test]
fn invalid_parent_values_warn_and_never_stop_the_run() {
    for (pid, start, why) in [
        (Some("0"), Some("1"), "pid 0"),
        (Some("1"), Some("1"), "pid 1"),
        (Some("abc"), Some("1"), "non-numeric pid"),
        (Some("4242"), None, "start time missing"),
    ] {
        let fx = Fx::new(RECORDING_RUNNER);
        let mut c = fx.cmd(None);
        if let Some(pid) = pid {
            c.env("AMPLIHACK_PARENT_RECIPE_PID", pid);
        }
        if let Some(start) = start {
            c.env("AMPLIHACK_PARENT_RECIPE_STARTTIME", start);
        }
        let out = output_within("the run to exit", &mut c);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{why}: stderr:\n{stderr}");
        assert!(fx.runner_pid().is_some(), "{why}: the runner must run");
        assert!(
            stderr.contains("AMPLIHACK_PARENT_RECIPE_PID"),
            "{why}: a warning names the bad variable:\n{stderr}"
        );
        assert!(fx.orphan_record().is_none(), "{why}: not orphaned");
    }
}
