//! Shared fixture for the issue #1760 `amplihack agent` tests.
//!
//! Every run goes through `isolated(..)` with `PATH=<tools>:/usr/bin:/bin`,
//! so the fake `tmux`, `git` and `claude` in `issue_1760_fakes.rs` come
//! first. `HOME`, `~/.claude`, the profile root (`AMPLIHACK_CLAUDE_PROFILE_ROOT`),
//! `TMUX_TMPDIR` and a synthetic `/proc` (`AMPLIHACK_TEST_PROC_ROOT`, honoured
//! only with the home guard that `isolated` sets) all lie under one temp
//! directory. No real `~/.claude`, `~/.amplihack`, profile root, tmux server
//! or `/proc` entry of another process is read or written.
//!
//! The socket at `socket` is a real Unix socket file nobody listens on, so
//! the S12 checks pass; the fake tmux never connects to it. Whether a tmux
//! session exists is the fake's state, set with `tmux_session`.
//!
//! More `Fx` methods: registry, log and marker files in
//! `issue_1760_fixture_files.rs`; the fakes in `issue_1760_fakes.rs`; the
//! synthetic `/proc` in `issue_1760_fake_proc.rs`.

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub const CONV_A: &str = "0b6c3f2e-8d4a-4f1b-9c7e-2a5d6e8f1a3b";
pub const CONV_B: &str = "6f3b9d2e-4a7c-4e1b-8c5d-2e9a7f1b4d36";
pub const CONV_C: &str = "3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57";
pub const CONV_D: &str = "e7b1d4c9-2a6f-4e3b-8d5c-9f1a7e2b6d48";
/// A `launch_id` for entries the tests write themselves.
pub const LAUNCH_ID: &str = "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d";
/// The pid of the synthetic `/proc/self` (the `amplihack` under test).
pub const SELF_PID: u32 = 999_999;

pub struct Fx {
    _tmp: tempfile::TempDir,
    /// Canonical temp root; every path a run may touch lies under it.
    pub root: PathBuf,
    pub home: PathBuf,
    /// `HOME/.claude`, mode 0700.
    pub claude_dir: PathBuf,
    /// The profile root (`AMPLIHACK_CLAUDE_PROFILE_ROOT`), mode 0700.
    pub profiles: PathBuf,
    pub tools: PathBuf,
    pub proc_root: PathBuf,
    /// Records of the fake `git` and `claude`.
    pub records: PathBuf,
    /// State and call log of the fake `tmux`.
    pub tmux_state: PathBuf,
    /// `TMUX_TMPDIR`.
    pub tmux_tmpdir: PathBuf,
    /// `TMUX_TMPDIR/tmux-<uid>`, mode 0700.
    pub socket_dir: PathBuf,
    /// `socket_dir/default`, a real Unix socket file.
    pub socket: PathBuf,
    /// Working directory of every run, and parent of `dir(..)`.
    pub work: PathBuf,
    pub uid: u32,
}

impl Fx {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let j = |p: &str| root.join(p);
        let fx = Self {
            home: j("home"),
            claude_dir: j("home/.claude"),
            profiles: j("profiles"),
            tools: j("tools"),
            proc_root: j("proc"),
            records: j("records"),
            tmux_state: j("tmux-state"),
            tmux_tmpdir: j("tt"),
            socket_dir: j(&format!("tt/tmux-{uid}")),
            socket: j(&format!("tt/tmux-{uid}/default")),
            work: j("work"),
            uid,
            root,
            _tmp: tmp,
        };
        for d in [&fx.home, &fx.records, &fx.tmux_state, &fx.work, &fx.tools] {
            fs::create_dir_all(d).unwrap();
        }
        for d in [
            &fx.claude_dir,
            &fx.profiles,
            &fx.proc_root,
            &fx.tmux_tmpdir,
            &fx.socket_dir,
        ] {
            fs::create_dir_all(d).unwrap();
            chmod(d, 0o700);
        }
        assert!(
            fx.socket.as_os_str().len() <= 107,
            "socket path {} is over 107 bytes; set a shorter TMPDIR",
            fx.socket.display()
        );
        fx.make_socket();
        fx.install_fakes();
        fx.set_self_parent(1);
        fx
    }

    /// `amplihack <args>` with only the fixture's environment.
    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env("PATH", format!("{}:/usr/bin:/bin", self.tools.display()))
            .env("HOME", &self.home)
            .env("TMUX_TMPDIR", &self.tmux_tmpdir)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.profiles)
            .env("AMPLIHACK_TEST_PROC_ROOT", &self.proc_root)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .stdin(Stdio::null());
        c
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.output(&mut self.cmd(args))
    }

    /// Runs `c`, retrying while a stub being written makes exec fail.
    pub fn output(&self, c: &mut Command) -> Output {
        let mut tries = 0;
        loop {
            match c.output() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 50 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
                r => return r.expect("run amplihack"),
            }
        }
    }

    /// `agent supervise --once` plus `extra`.
    pub fn supervise_once(&self, extra: &[&str]) -> Output {
        let mut args = vec!["agent", "supervise", "--once"];
        args.extend(extra);
        self.run(&args)
    }

    /// A new launch directory `work/<name>`.
    pub fn dir(&self, name: &str) -> PathBuf {
        let d = self.work.join(name);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// `agent register NAME --session S --window main --dir D --conversation C`
    /// plus `extra`. With no `$TMUX`, no socket is recorded.
    pub fn register(
        &self,
        name: &str,
        session: &str,
        dir: &Path,
        conv: &str,
        extra: &[&str],
    ) -> Output {
        let d = dir.display().to_string();
        let mut args = vec!["agent", "register", name, "--session", session];
        args.extend(["--window", "main", "--dir", &d, "--conversation", conv]);
        args.extend(extra);
        self.run(&args)
    }
}

pub fn chmod(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A UTC registry timestamp, `%Y-%m-%dT%H:%M:%SZ`.
pub fn ts(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// The inverse of `ts`.
pub fn parse_ts(s: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ")
        .unwrap_or_else(|e| panic!("{s:?} is not a registry timestamp: {e}"))
        .and_utc()
        .timestamp()
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Fails with both streams unless `o` exited with `code`. A binary that does
/// not know `agent` (a usage error) never passes as an expected exit 2.
pub fn expect_code(o: &Output, code: i32) {
    let err = stderr(o);
    assert!(
        !err.contains("unrecognized subcommand")
            && !err.contains("unexpected argument '--supervise'"),
        "`amplihack agent` / `claude --supervise` is not implemented:\n{err}"
    );
    assert_eq!(
        o.status.code(),
        Some(code),
        "expected exit {code}\nstdout:\n{}\nstderr:\n{err}",
        stdout(o)
    );
}

/// Polls `f` every 50 ms until it holds or `secs` pass.
pub fn wait_until(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    f()
}

/// The canonical path of the `amplihack` binary, as `current_exe()` gives it.
pub fn exe() -> String {
    fs::canonicalize(amplihack_bin())
        .unwrap()
        .display()
        .to_string()
}
