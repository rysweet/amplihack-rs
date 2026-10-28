//! Issue #1710: `amplihack workstreams status` through the real binary.
//!
//! Hermetic: `isolated(..)`, a cleared environment, a temporary HOME, state
//! directory and profile root, and stub `tmux`, `gh` and `claude` first on
//! PATH. The stubs fail or stall; the command must still finish and exit 0.
//! Nothing here talks to the user's tmux server, GitHub or Claude.

#![cfg(target_os = "linux")]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};

const SESSION: &str = "amp-default-workflow-app-20261028-090000";
const ALLOWED_GLYPHS: [char; 5] = ['█', '░', '▶', '⏸', '…'];

struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    state: PathBuf,
    profiles: PathBuf,
    marks: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let fx = Self {
            home: root.join("home"),
            bin: root.join("bin"),
            state: root.join("state"),
            profiles: root.join("profiles"),
            marks: root.join("marks"),
            _tmp: tmp,
        };
        for d in [&fx.home, &fx.bin, &fx.state, &fx.marks] {
            fs::create_dir_all(d).unwrap();
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fx.stub("tmux", "exit 1");
        fx.stub("gh", "exit 1");
        fx.stub(
            "claude",
            &format!("touch '{}/claude-ran'\nexec sleep 30", fx.marks.display()),
        );
        fx
    }

    fn stub(&self, name: &str, body: &str) {
        crate::support::exec_stub::write_executable(
            &self.bin.join(name),
            format!("#!/bin/sh\n{body}\n"),
            0o755,
        );
    }

    fn log_dir(&self) -> PathBuf {
        let d = self.home.join(".amplihack").join("recipe-logs");
        fs::create_dir_all(&d).unwrap();
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
        d
    }

    /// A run record as `recipe run --detach` writes it.
    fn record(&self, session: &str, parent: &str) {
        let body = serde_json::json!({
            "version": 1,
            "session": session,
            "parent": parent,
            "started_at": "2026-10-28T09:00:00Z",
            "recipe": "default-workflow",
            "issue_number": 1710,
            "pr_number": null,
            "repo": "alice/app",
            "cwd": self.home.to_str().unwrap(),
            "outcome": null,
        });
        let dir = self.log_dir();
        let p = dir.join(format!("{session}.run.json"));
        fs::write(&p, body.to_string()).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(dir.join(format!("{session}.log")), "started\n").unwrap();
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> (Output, Duration) {
        let mut cmd = crate::isolated_amplihack::isolated(amplihack_bin());
        cmd.args(args)
            .current_dir(&self.home)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("TMPDIR", std::env::temp_dir())
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.home)
            .env("AMPLIHACK_STATE_DIR", &self.state)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.profiles)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .stdin(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let start = Instant::now();
        let out = cmd.output().unwrap();
        (out, start.elapsed())
    }

    fn add_profile(&self, name: &str) {
        let (out, _) = self.run(&["profile", "add", name], &[]);
        assert!(
            out.status.success(),
            "profile add: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_display_safe(text: &str) {
    assert!(!text.contains('\x1b'), "ESC in output:\n{text}");
    for line in text.lines() {
        assert!(line.chars().count() <= 100, "too wide: {line:?}");
        assert!(
            line.chars()
                .all(|c| (' '..='~').contains(&c) || ALLOWED_GLYPHS.contains(&c)),
            "unexpected character in {line:?}"
        );
    }
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

#[test]
fn status_without_logins_renders_plain_text_and_exits_zero() {
    let fx = Fx::new();
    fx.record(SESSION, "session:me");
    let (out, took) = fx.run(
        &["workstreams", "status", "--no-logins"],
        &[("AMPLIHACK_SESSION_ID", "me")],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(took < Duration::from_secs(20), "took {took:?}");
    let text = stdout(&out);
    assert_display_safe(&text);
    assert!(text.starts_with("WORKSTREAMS\n"), "{text}");
    assert!(!text.contains("LOGINS"), "{text}");
    let row = text
        .lines()
        .find(|l| l.contains("#1710"))
        .unwrap_or_else(|| panic!("no row for the caller's run:\n{text}"));
    // tmux failed, so nothing is live; gh failed, so the PR is unknown.
    assert!(row.starts_with('⏸'), "{row}");
    assert!(row.contains("#?"), "{row}");
}

#[test]
fn no_logins_never_starts_a_usage_probe() {
    let fx = Fx::new();
    fx.add_profile("spare");
    let (out, _) = fx.run(&["workstreams", "status", "--no-logins", "--all"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        !exists(&fx.marks.join("claude-ran")),
        "--no-logins must not run claude"
    );
}

#[test]
fn a_stalled_probe_shows_no_reading_within_the_deadline() {
    let fx = Fx::new();
    fx.add_profile("spare");
    let (out, took) = fx.run(
        &["workstreams", "status", "--logins-deadline", "1"],
        &[("AMPLIHACK_SESSION_ID", "me")],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(took < Duration::from_secs(10), "took {took:?}");
    let text = stdout(&out);
    assert_display_safe(&text);
    assert!(text.contains("LOGINS"), "{text}");
    let row = text
        .lines()
        .find(|l| l.contains("spare"))
        .unwrap_or_else(|| panic!("no spare row:\n{text}"));
    assert!(row.contains("no reading"), "{row}");
}

#[test]
fn without_an_identity_the_hint_is_printed_and_other_runs_are_hidden() {
    let fx = Fx::new();
    fx.record(SESSION, "session:someone-else");
    let (out, _) = fx.run(&["workstreams", "status", "--no-logins"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("no session id found; run with --all to see every run on this host"),
        "{text}"
    );
    assert!(!text.contains("#1710"), "{text}");

    let (out, _) = fx.run(&["workstreams", "status", "--no-logins", "--all"], &[]);
    assert!(stdout(&out).contains("#1710"), "{}", stdout(&out));
}

#[test]
fn the_default_manifest_adds_queued_rows() {
    let fx = Fx::new();
    let work = fx.home.join(".amplihack").join("work");
    fs::create_dir_all(&work).unwrap();
    let m = work.join("workstreams.tsv");
    fs::write(&m, "docs sweep\t1720\t-\n").unwrap();
    fs::set_permissions(&m, fs::Permissions::from_mode(0o644)).unwrap();
    let (out, _) = fx.run(&["workstreams", "status", "--no-logins"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let row = text
        .lines()
        .find(|l| l.contains("docs sweep"))
        .unwrap_or_else(|| panic!("no manifest row:\n{text}"));
    assert!(row.contains("queued") && row.contains("0%"), "{row}");
}

#[test]
fn a_missing_or_malformed_explicit_manifest_exits_one() {
    let fx = Fx::new();
    let missing = fx.home.join("nope.tsv");
    let (out, _) = fx.run(
        &[
            "workstreams",
            "status",
            "--no-logins",
            "--manifest",
            missing.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));

    let bad = fx.home.join("bad.tsv");
    fs::write(&bad, "# c\nok\t1\t-\nbad\t2\n").unwrap();
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o600)).unwrap();
    let (out, _) = fx.run(
        &[
            "workstreams",
            "status",
            "--no-logins",
            "--manifest",
            bad.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(":3: expected 3 tab-separated fields, found 2"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_deadline_out_of_range_is_a_usage_error() {
    let fx = Fx::new();
    for bad in ["0", "61"] {
        let (out, _) = fx.run(&["workstreams", "status", "--logins-deadline", bad], &[]);
        assert_eq!(out.status.code(), Some(2), "--logins-deadline {bad}");
    }
}

#[test]
fn the_command_never_changes_the_log_directory_mode() {
    let fx = Fx::new();
    fx.record(SESSION, "session:me");
    let dir = fx.log_dir();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o750)).unwrap();
    let (out, _) = fx.run(&["workstreams", "status", "--no-logins", "--all"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o750
    );
}
