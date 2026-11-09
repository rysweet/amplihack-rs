//! Issue #2018: the fixture shared by `issue_2018_step_lifetime`,
//! `issue_2018_step_records` and `issue_2018_steps_cli`.
//!
//! Runs the real binary through `isolated(..)` with a temporary `HOME`,
//! profile root and `CLAUDE_CONFIG_DIR`, the capacity governor off, and a
//! stub agent named by `AMPLIHACK_CLAUDE_BINARY_PATH` that answers
//! `--version` for the launcher's pre-flight check (as in
//! `issue_1567_nested_profile.rs`). Nothing here reads or writes the real
//! `~/.claude`, `~/.amplihack` or profile root, and no `LBUG_*` variable is
//! set.
//!
//! The stub agent writes its pid by rename, then acts on `AGENT_MODE`:
//! - unset: `exec cat` on the FIFO `$RECORD_DIR/hold`, which nothing opens
//!   for writing, so the agent blocks until a signal ends it, and the
//!   recorded pid stays the agent's;
//! - `exit`: exits 0 at once;
//! - `gate`: blocks opening the FIFO `$RECORD_DIR/release` until a writer
//!   opens it ([`Fx::release`]), writes `$RECORD_DIR/agent.finished`, then
//!   exits 0.
//!
//! The `Drop` guard SIGKILLs every pid a stub or the fixture recorded that is
//! still alive, after checking that it is greater than 1.
#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::proc_watch::{alive, read_pid_file, wait_for_fs};
use serde_json::Value;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};

pub const STUB_AGENT: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
echo $$ > "$RECORD_DIR/agent.pid.tmp.$$" && mv -f "$RECORD_DIR/agent.pid.tmp.$$" "$RECORD_DIR/agent.pid"
case "$AGENT_MODE" in
  exit) exit 0 ;;
  gate)
    read gate < "$RECORD_DIR/release" || :
    : > "$RECORD_DIR/agent.finished"
    exit 0 ;;
esac
exec cat "$RECORD_DIR/hold"
"#;

/// The stand-in runner: records its pid, starts the launcher as its direct
/// child (stdin `/dev/null`, stdout and stderr to a log file), records the
/// launcher's pid, then waits on it, as `recipe-runner-rs` waits on an agent
/// step it forked from its main thread. `$STEP_ARGS` is split on purpose.
pub const STUB_RUNNER: &str = r#"#!/bin/sh
echo $$ > "$RECORD_DIR/runner.pid.tmp" && mv -f "$RECORD_DIR/runner.pid.tmp" "$RECORD_DIR/runner.pid"
"$AMPLIHACK_UNDER_TEST" claude --subprocess-safe -- $STEP_ARGS </dev/null >>"$RECORD_DIR/launcher.log" 2>&1 &
echo $! > "$RECORD_DIR/launcher.pid.tmp" && mv -f "$RECORD_DIR/launcher.pid.tmp" "$RECORD_DIR/launcher.pid"
wait
"#;

/// Agent arguments with no `-p`: the usage-limit supervisor wraps only a
/// print launch, so the launcher is the agent's direct parent.
pub const UNSUPERVISED: &str = "--issue-2018-step";

/// A print launch. With `AMPLIHACK_NONINTERACTIVE=1` and the usage-limit
/// wait on, the supervisor wraps it: runner, supervisor, attempt child,
/// agent (D2, D4).
pub const SUPERVISED: &str = "-p issue-2018-step";

pub struct Fx {
    _tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub profiles: PathBuf,
    pub config: PathBuf,
    pub records: PathBuf,
    pub work: PathBuf,
    pub tools: PathBuf,
}

impl Fx {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let fx = Self {
            home: root.join("home"),
            profiles: root.join("profiles"),
            config: root.join("claude-config"),
            records: root.join("records"),
            work: root.join("work"),
            tools: root.join("tools"),
            _tmp: tmp,
        };
        for dir in [
            &fx.home,
            &fx.config,
            &fx.records,
            &fx.work,
            &fx.tools,
            &fx.profiles,
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::set_permissions(&fx.profiles, fs::Permissions::from_mode(0o700)).unwrap();
        for fifo in [fx.release_fifo(), fx.records.join("hold")] {
            mkfifo(&fifo);
        }
        crate::support::exec_stub::write_executable(&fx.tools.join("claude"), STUB_AGENT, 0o755);
        crate::support::exec_stub::write_executable(&fx.tools.join("runner"), STUB_RUNNER, 0o755);
        fx
    }

    /// The environment every process in a test gets, on top of `isolated`.
    fn configure(&self, c: &mut Command) {
        c.current_dir(&self.work)
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.profiles)
            .env("CLAUDE_CONFIG_DIR", &self.config)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_UNDER_TEST", amplihack_bin())
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_GOVERNOR", "off")
            .stdin(Stdio::null());
    }

    /// `amplihack <args>`, isolated and configured, not yet started.
    pub fn amplihack(&self, args: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        self.configure(&mut c);
        c.args(args);
        c
    }

    /// An unsupervised `amplihack claude` step, with `AMPLIHACK_RECIPE_RUN_ID`
    /// set to `run_id` or removed, not yet started.
    pub fn step(&self, run_id: Option<&str>) -> Command {
        let mut c = self.amplihack(&["claude", "--subprocess-safe", "--", UNSUPERVISED]);
        match run_id {
            Some(id) => c.env("AMPLIHACK_RECIPE_RUN_ID", id),
            None => c.env_remove("AMPLIHACK_RECIPE_RUN_ID"),
        };
        c
    }

    /// Starts `cmd` (from [`Self::step`]) with stdout and stderr to the log
    /// file, so stdout is never a terminal, and records its pid for `Drop`.
    pub fn spawn_logged(&self, mut cmd: Command) -> Child {
        let log = fs::File::create(self.records.join("launcher.log")).unwrap();
        cmd.stdout(log.try_clone().unwrap()).stderr(log);
        let child = cmd.spawn().expect("start amplihack claude");
        fs::write(self.records.join("launcher.pid"), child.id().to_string()).unwrap();
        child
    }

    /// The stand-in runner, starting one headless step with `step_args`
    /// under `run_id`.
    pub fn runner(&self, run_id: &str, step_args: &str) -> Child {
        self.runner_cmd(run_id, step_args)
            .spawn()
            .expect("start the stand-in runner")
    }

    /// [`Self::runner`], configured but not yet started.
    pub fn runner_cmd(&self, run_id: &str, step_args: &str) -> Command {
        let mut c = crate::isolated_amplihack::isolated(self.tools.join("runner"));
        self.configure(&mut c);
        c.env("AMPLIHACK_RECIPE_RUN_ID", run_id)
            .env("STEP_ARGS", step_args)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        c
    }

    /// A new FIFO named `name` in the record directory.
    pub fn fifo(&self, name: &str) -> PathBuf {
        let path = self.records.join(name);
        mkfifo(&path);
        path
    }

    /// `amplihack recipe steps <args>`, run to completion.
    pub fn steps(&self, args: &[&str]) -> Output {
        let mut all = vec!["recipe", "steps"];
        all.extend_from_slice(args);
        self.amplihack(&all)
            .output()
            .expect("run amplihack recipe steps")
    }

    /// `amplihack recipe steps --format json <args>`, which must succeed,
    /// parsed as an array.
    pub fn steps_json(&self, args: &[&str]) -> Vec<Value> {
        let mut all = vec!["--format", "json"];
        all.extend_from_slice(args);
        let out = self.steps(&all);
        assert!(
            out.status.success(),
            "recipe steps {all:?} failed: {}\nstderr:\n{}",
            out.status,
            text(&out.stderr)
        );
        let value: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{}", text(&out.stdout)));
        value.as_array().expect("a JSON array").clone()
    }

    pub fn pid(&self, name: &str) -> i32 {
        read_pid_file(&self.records.join(name))
    }

    /// Records `pid` under `name` so that `Drop` ends it if a test fails.
    pub fn remember(&self, name: &str, pid: i32) {
        fs::write(self.records.join(name), pid.to_string()).unwrap();
    }

    fn release_fifo(&self) -> PathBuf {
        self.records.join("release")
    }

    /// Lets a `gate` agent exit. The open blocks until the agent is reading
    /// the FIFO, so it returns at the moment the agent is released.
    pub fn release(&self) {
        fs::OpenOptions::new()
            .write(true)
            .open(self.release_fifo())
            .expect("open the release FIFO");
    }

    /// Whether a `gate` agent ran to the end after its release.
    pub fn agent_finished(&self) -> bool {
        self.records.join("agent.finished").exists()
    }

    pub fn log(&self) -> String {
        fs::read_to_string(self.records.join("launcher.log")).unwrap_or_default()
    }

    pub fn root(&self) -> PathBuf {
        self.home
            .join(".amplihack")
            .join("runtime")
            .join("recipe-steps")
    }

    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.root().join(run_id)
    }

    /// Names in `run_dir(run_id)`, sorted, or none when it does not exist.
    pub fn entries(&self, run_id: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.run_dir(run_id))
            .map(|d| {
                d.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Waits for the step's record and returns its path, failing unless the
    /// run-id directory then holds exactly one file. Call it after the
    /// agent's pid file exists. The wait ends on the directory change that
    /// creates the record.
    pub fn the_record(&self, run_id: &str) -> PathBuf {
        wait_for_fs(&self.run_dir(run_id), || {
            self.entries(run_id).iter().any(|n| n.ends_with(".json"))
        });
        let entries = self.entries(run_id);
        assert_eq!(entries.len(), 1, "one record per running step: {entries:?}");
        self.run_dir(run_id).join(&entries[0])
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        // A gate agent that is waiting at the FIFO is released; without a
        // reader the open fails at once instead of blocking.
        let _ = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(self.release_fifo());
        for name in ["agent.pid", "attempt.pid", "launcher.pid", "runner.pid"] {
            if let Some(pid) = fs::read_to_string(self.records.join(name))
                .ok()
                .and_then(|text| text.trim().parse::<i32>().ok())
                .filter(|pid| *pid > 1)
                && alive(pid)
            {
                // SAFETY: a process this test's own stub or fixture started.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

fn mkfifo(path: &std::path::Path) {
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path inside a fixture's temp directory.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
}

pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The `state` of each `recipe steps` JSON entry.
pub fn states(entries: &[Value]) -> Vec<&str> {
    entries
        .iter()
        .map(|e| e["state"].as_str().unwrap())
        .collect()
}

/// Sends `sig` to `pid`, which must be greater than 1.
pub fn signal(pid: i32, sig: i32) {
    assert!(pid > 1, "refusing to signal pid {pid}");
    // SAFETY: a process this test started, checked above to be above 1.
    let rc = unsafe { libc::kill(pid, sig) };
    assert_eq!(rc, 0, "kill({pid}, {sig})");
}

pub fn sigkill(pid: i32) {
    signal(pid, libc::SIGKILL);
}
