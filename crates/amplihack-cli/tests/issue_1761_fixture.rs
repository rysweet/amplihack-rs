//! Shared fixture for the issue #1761 repeat-wait and run-budget tests, on
//! top of `issue_1618_fixture`.
//!
//! - [`FakeRotator`]: a shell loop whose arguments hold
//!   `profile rotate watch` (never `--spawned`), named by the fixture
//!   profile root's rotate pid file. It runs with a cleared environment and
//!   the fixture's HOME and profile root, exits when this test process
//!   does, and is killed and reaped on drop.
//! - [`write_ledger`]: the rotate ledger with a last cycle recorded and
//!   measured at [`T`], ten minutes after `TEST_NOW`. Every wait here starts
//!   before `T` in virtual time, so with `swapped` each ends at its first
//!   30 s poll with `rotator capacity`. `T` is in the real past, which the
//!   ledger reader requires.
//! - [`home_claude`]: the step runs in `$HOME/.claude` (the fixture's), the
//!   only directory a rotator counts for. `CLAUDE_CONFIG_DIR` is set to it,
//!   never removed (#1699).
//!
//! Lines are matched with [`matches`], whose patterns take `\d+` for the
//! step id and the seconds waited.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, TEST_NOW, private_file, text};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

/// When the ledger's last cycle was recorded and measured.
pub const T: i64 = TEST_NOW + 600;
/// The rotator's live profile in [`write_ledger`].
pub const LIVE: &str = "team";

pub const RUN_ID_VAR: &str = "AMPLIHACK_RECIPE_RUN_ID";
pub const BUDGET_VAR: &str = "AMPLIHACK_USAGE_LIMIT_WAIT_BUDGET_SECS";
pub const WAITS_VAR: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAITS_PER_STEP";

pub const NOT_SET: &str = "warning: the usage-limit wait budget applies to this step only (AMPLIHACK_RECIPE_RUN_ID is not set)";
pub const NOT_VALID: &str = "warning: the usage-limit wait budget applies to this step only (AMPLIHACK_RECIPE_RUN_ID is not a valid run id)";
pub const UNUSABLE: &str = "warning: the usage-limit wait budget applies to this step only (the budget record cannot be used)";

/// The plain line after the run budget is spent.
pub fn run_budget_note(budget: u64) -> String {
    format!(
        "amplihack: this recipe run's usage-limit wait budget \
         (recipe.usage_limit_wait_budget_secs = {budget} s) is spent; not waiting again"
    )
}

/// The plain line after a step's own budget is spent (per-step fallback).
pub fn step_budget_note(budget: u64) -> String {
    format!(
        "amplihack: this step's usage-limit wait budget \
         (recipe.usage_limit_wait_budget_secs = {budget} s) is spent; not waiting again"
    )
}

/// The plain line after a step's waits are used up.
pub fn cap_note(max: u32) -> String {
    format!(
        "amplihack: this step's usage-limit waits \
         (recipe.usage_limit_max_waits_per_step = {max}) are used up; not waiting again"
    )
}

/// A fresh run id, as `amplihack recipe run` makes one.
pub fn run_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn budget_record(f: &Fixture, id: &str) -> PathBuf {
    f.rerun_dir().join(format!("wait-budget-{id}"))
}

/// The names in `$HOME/.amplihack/rerun` starting with `wait-budget-`.
pub fn budget_records(f: &Fixture) -> Vec<String> {
    let Ok(dir) = fs::read_dir(f.rerun_dir()) else {
        return Vec::new();
    };
    dir.filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("wait-budget-"))
        .collect()
}

fn private_dirs(f: &Fixture) -> PathBuf {
    let rotate = f.profiles.join(".auto-switch.d").join("rotate");
    fs::create_dir_all(&rotate).unwrap();
    for d in [rotate.parent().unwrap(), rotate.as_path()] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    rotate
}

/// The rotate ledger: live profile `live`, last cycle `outcome` at [`T`].
pub fn write_ledger(f: &Fixture, live: &str, outcome: &str) {
    let rotate = private_dirs(f);
    private_file(
        &rotate.join("ledger.json"),
        &format!(
            r#"{{"version":1,"live":"{live}","last_cycle":{{"outcome":"{outcome}","at":{T},"measured_at":{T}}}}}"#
        ),
    );
}

/// The step runs in the fixture's `$HOME/.claude`.
pub fn home_claude(f: &Fixture, c: &mut Command) {
    let dir = f.home.join(".claude");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    c.env("CLAUDE_CONFIG_DIR", &dir);
}

pub struct FakeRotator(Child);

impl FakeRotator {
    /// Starts the fake rotator and writes its pid file.
    pub fn start(f: &Fixture) -> Self {
        let child = Command::new("/bin/sh")
            .args([
                "-c",
                "while kill -0 \"$PPID\" 2>/dev/null; do sleep 1; done",
                "amplihack",
                "profile",
                "rotate",
                "watch",
            ])
            .env_clear()
            .env("HOME", &f.home)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &f.profiles)
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the fake rotator");
        let rotator = Self(child);
        rotator.await_args();
        let rotate = private_dirs(f);
        private_file(&rotate.join("rotate.pid"), &format!("{}\n", rotator.0.id()));
        rotator
    }

    /// Waits until `/proc/<pid>/cmdline` holds the rotator's arguments: a
    /// spawn returns a moment before the kernel records where they are.
    fn await_args(&self) {
        if !Path::new("/proc").is_dir() {
            return;
        }
        let cmdline = format!("/proc/{}/cmdline", self.0.id());
        for _ in 0..500 {
            if fs::read(&cmdline).is_ok_and(|c| c.split(|b| *b == 0).any(|a| a == b"watch")) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the fake rotator's arguments never appeared in {cmdline}");
    }
}

impl Drop for FakeRotator {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs `c` and returns the launch's process id with its output; retried
/// while the freshly built binary is still busy, as `Fixture::run` does.
pub fn run_with_pid(c: &mut Command) -> (u32, Output) {
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tries = 0;
    let child = loop {
        match c.spawn() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 50 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            r => break r.expect("run amplihack"),
        }
    };
    let pid = child.id();
    (pid, child.wait_with_output().expect("wait for amplihack"))
}

/// Whether `line` matches `pattern` in full, where `\d+` in the pattern
/// stands for one or more digits and every other character is literal.
pub fn matches(line: &str, pattern: &str) -> bool {
    let escaped: Vec<String> = pattern.split(r"\d+").map(regex::escape).collect();
    regex::Regex::new(&format!("^{}$", escaped.join(r"\d+")))
        .unwrap()
        .is_match(line)
}

/// Asserts that `lines` match `patterns` one for one.
pub fn assert_lines(lines: &[String], patterns: &[&str]) {
    assert_eq!(
        lines.len(),
        patterns.len(),
        "lines {lines:#?}\npatterns {patterns:#?}"
    );
    for (l, p) in lines.iter().zip(patterns) {
        assert!(matches(l, p), "{l:?} does not match {p:?}\nin {lines:#?}");
    }
}

/// The value of `key=` in `line`.
pub fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split(' ')
        .find_map(|t| t.strip_prefix(key)?.strip_prefix('='))
}

/// The seconds of each `waited=<n>s` field in `lines` that start with
/// `prefix`.
pub fn waited(lines: &[String], prefix: &str) -> Vec<u64> {
    lines
        .iter()
        .filter(|l| l.starts_with(prefix))
        .filter_map(|l| field(l, "waited"))
        .map(|v| v.strip_suffix('s').unwrap().parse().unwrap())
        .collect()
}

/// The stderr lines that start with `warning: `.
pub fn warnings(out: &Output) -> Vec<String> {
    text(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("warning: "))
        .map(str::to_string)
        .collect()
}

/// The `warning: ` lines about the usage-limit wait budget.
pub fn budget_warnings(out: &Output) -> Vec<String> {
    warnings(out)
        .into_iter()
        .filter(|l| l.contains("usage-limit wait budget"))
        .collect()
}

/// The stderr line directly after the first one that contains `event`.
pub fn line_after(out: &Output, event: &str) -> Option<String> {
    let stderr = text(&out.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    let i = lines.iter().position(|l| l.contains(event))?;
    lines.get(i + 1).map(|l| l.to_string())
}

/// The record's lines as numbers.
pub fn record_lines(path: &Path) -> Vec<u64> {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .map(|l| l.parse().unwrap())
        .collect()
}
