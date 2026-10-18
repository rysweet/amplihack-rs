//! Issue #1567: a nested `amplihack claude` keeps the login profile its parent
//! resolved, whatever the default pointer says and even after a
//! `profile switch` partway through.
//!
//! Runs the real `amplihack` binary with an isolated `HOME` and profile root.
//! A fake `claude` on `PATH` plays the outer session: it records its
//! environment, runs `amplihack profile switch`, then starts a nested
//! `amplihack claude` with no `--profile`. The nested fake `claude` records
//! its environment too, and the test compares the two.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn executable(path: &Path, body: &str) {
    crate::support::exec_stub::write_executable(path, body, 0o755);
}

/// The outer fake `claude` (first run) switches the default profile and
/// starts a nested launch; the nested one (second run) only records.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.0 (Claude Code)'; exit 0 ;;
esac
record() {
  {
    printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}"
    printf 'AMPLIHACK_CLAUDE_PROFILE=%s\n' "${AMPLIHACK_CLAUDE_PROFILE-<unset>}"
    printf 'AMPLIHACK_CLAUDE_PROFILE_ROOT=%s\n' "${AMPLIHACK_CLAUDE_PROFILE_ROOT-<unset>}"
  } > "$RECORD_DIR/$1"
}
if [ ! -f "$RECORD_DIR/outer" ]; then
  record outer
  "$AMPLIHACK_UNDER_TEST" profile switch "$SWITCH_TO" > "$RECORD_DIR/switch.log" 2>&1 || exit 41
  "$AMPLIHACK_UNDER_TEST" claude --subprocess-safe -- -p nested > "$RECORD_DIR/nested.log" 2>&1 || exit 42
  exit 0
fi
record nested
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    records: PathBuf,
    work: PathBuf,
    tools: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let root = home.join("profiles");
        let records = tmp.path().join("records");
        let work = tmp.path().join("work");
        let tools = tmp.path().join("tools");
        for d in [&home, &records, &work, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        executable(&tools.join("claude"), FAKE_CLAUDE);
        let f = Self {
            _tmp: tmp,
            home,
            root,
            records,
            work,
            tools,
        };
        for name in ["X", "Y", "Z"] {
            f.ok(&["profile", "add", name]);
        }
        f
    }

    /// A command with no inherited environment except a minimal `PATH`, so a
    /// developer's own amplihack session cannot leak a profile or session
    /// variable into the test.
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_UNDER_TEST", amplihack_bin())
            .env("RECORD_DIR", &self.records)
            // Suppress launch bootstrap (see codex_frontdoor_contract/fixture.rs).
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1");
        // Belt and braces: `env_clear` already removed these.
        for var in [
            "AMPLIHACK_CLAUDE_PROFILE",
            "AMPLIHACK_CLAUDE_PROFILE_ROOT",
            "CLAUDE_CONFIG_DIR",
            "AMPLIHACK_USE_DOCKER",
            "AMPLIHACK_SESSION_ID",
            "AMPLIHACK_SESSION_DEPTH",
            "AMPLIHACK_DEPTH",
            "AMPLIHACK_TREE_ID",
            "AMPLIHACK_MAX_DEPTH",
            "AMPLIHACK_MAX_SESSIONS",
            "AMPLIHACK_RECIPE_RUN_ID",
            "AMPLIHACK_ORCHESTRATION_BOOTSTRAP",
        ] {
            c.env_remove(var);
        }
        // Set after the removal above, which would otherwise drop it.
        c.env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root);
        c
    }

    fn ok(&self, args: &[&str]) -> Output {
        let out = self.cmd(args).output().unwrap();
        assert!(
            out.status.success(),
            "`amplihack {}` failed: {}\nstdout: {}\nstderr: {}\n{}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
            self.logs(),
        );
        out
    }

    fn logs(&self) -> String {
        ["switch.log", "nested.log"]
            .iter()
            .map(|f| {
                format!(
                    "--- {f} ---\n{}",
                    fs::read_to_string(self.records.join(f)).unwrap_or_default()
                )
            })
            .collect()
    }

    fn record(&self, which: &str) -> String {
        fs::read_to_string(self.records.join(which))
            .unwrap_or_else(|e| panic!("no {which} record ({e})\n{}", self.logs()))
    }

    fn profile_dir(&self, name: &str) -> PathBuf {
        self.root.canonicalize().unwrap().join(name)
    }

    fn assert_pinned_to(&self, which: &str, name: &str) {
        let rec = self.record(which);
        let want_dir = format!("CLAUDE_CONFIG_DIR={}\n", self.profile_dir(name).display());
        assert!(
            rec.contains(&want_dir),
            "{which} launch must run as profile {name}; record:\n{rec}\n{}",
            self.logs()
        );
        assert!(
            rec.contains(&format!("AMPLIHACK_CLAUDE_PROFILE={name}\n")),
            "{which} launch must carry AMPLIHACK_CLAUDE_PROFILE={name}; record:\n{rec}"
        );
    }
}

/// Case A: pointer Y, parent `--profile X`, outer session switches to Z.
#[test]
fn nested_launch_keeps_flag_profile_despite_pointer_and_switch() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "Y"]);

    let mut parent = f.cmd(&[
        "claude",
        "--profile",
        "X",
        "--subprocess-safe",
        "--",
        "-p",
        "outer",
    ]);
    parent.env("SWITCH_TO", "Z");
    let out = parent.output().unwrap();
    assert!(
        out.status.success(),
        "parent launch failed: {}\nstderr: {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        f.logs()
    );

    // Nested first: on the unfixed code this is the assertion that shows the
    // bug (the nested session runs as whatever the pointer now names).
    f.assert_pinned_to("nested", "X");
    f.assert_pinned_to("outer", "X");
}

/// Case B: pointer X, parent without a flag, outer session switches to Y.
#[test]
fn nested_launch_keeps_pointer_profile_resolved_by_parent_after_switch() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "X"]);

    let mut parent = f.cmd(&["claude", "--subprocess-safe", "--", "-p", "outer"]);
    parent.env("SWITCH_TO", "Y");
    let out = parent.output().unwrap();
    assert!(
        out.status.success(),
        "parent launch failed: {}\nstderr: {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        f.logs()
    );

    // The switch really happened: a fresh top-level launch would now get Y.
    let show = f.ok(&["profile", "show"]);
    assert!(String::from_utf8_lossy(&show.stdout).contains("name: Y"));

    // Nested first: on the unfixed code this is the assertion that shows the
    // bug (the nested session runs as whatever the pointer now names).
    f.assert_pinned_to("nested", "X");
    f.assert_pinned_to("outer", "X");
}

/// The root reaches the nested launch too, so it looks in the same store
/// rather than the default root. Guards the propagation path; it already
/// holds through plain inheritance and must keep holding after the fix.
#[test]
fn nested_launch_receives_the_profile_root() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "Y"]);

    let mut parent = f.cmd(&[
        "claude",
        "--profile",
        "X",
        "--subprocess-safe",
        "--",
        "-p",
        "outer",
    ]);
    parent.env("SWITCH_TO", "Z");
    let _ = parent.output().unwrap();

    let want = format!(
        "AMPLIHACK_CLAUDE_PROFILE_ROOT={}\n",
        f.root.canonicalize().unwrap().display()
    );
    for which in ["outer", "nested"] {
        let rec = f.record(which);
        assert!(rec.contains(&want), "{which} record:\n{rec}");
    }
}

/// D2: `profile run X` pins X for the sessions it starts, like `--profile X`.
#[test]
fn nested_launch_inside_profile_run_keeps_its_profile() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "Y"]);

    let mut parent = f.cmd(&["profile", "run", "X", "--", "-p", "outer"]);
    parent.env("SWITCH_TO", "Z");
    let out = parent.output().unwrap();
    assert!(
        out.status.success(),
        "`profile run` failed: {}\nstderr: {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        f.logs()
    );

    f.assert_pinned_to("nested", "X");
    f.assert_pinned_to("outer", "X");
}

/// D1: a nested `--auto` launch inside a profiled session is refused with a
/// hint instead of silently running on the default account.
#[test]
fn inherited_profile_with_auto_is_refused_with_a_hint() {
    let f = Fixture::new();
    let out = f
        .cmd(&["claude", "--auto", "--", "-p", "x"])
        .env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must fail closed; stderr: {stderr}");
    assert!(stderr.contains("AMPLIHACK_CLAUDE_PROFILE"), "{stderr}");
    assert!(
        stderr.contains("unset AMPLIHACK_CLAUDE_PROFILE and CLAUDE_CONFIG_DIR"),
        "{stderr}"
    );
    assert!(
        !f.records.join("outer").exists(),
        "claude must not be spawned"
    );
}
