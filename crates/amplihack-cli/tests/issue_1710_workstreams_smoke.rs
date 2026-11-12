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
const ALLOWED_GLYPHS: [char; 6] = ['█', '░', '▶', '⏸', '…', '—'];

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
            .env("AMPLIHACK_PROFILE_USAGE_PROBE", "text")
            .env("AMPLIHACK_PROFILE_USAGE_PARALLEL", FIRST_TURN.to_string())
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .stdin(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        crate::isolated_amplihack::reapply_proc_root(&mut cmd);
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
        assert!(line.chars().count() <= 120, "too wide: {line:?}");
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
    let title = text.lines().next().unwrap_or_default();
    assert!(
        title.starts_with("  ") && title.contains("workstreams — "),
        "{text}"
    );
    assert!(text.contains("\n  WORKSTREAM "), "column headings: {text}");
    assert!(!text.contains("  LOGIN "), "{text}");
    let row = text
        .lines()
        .find(|l| l.contains("#1710"))
        .unwrap_or_else(|| panic!("no row for the caller's run:\n{text}"));
    // tmux failed, so nothing is live; gh failed, so the PR is unknown.
    assert!(row.contains('⏸'), "{row}");
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
    // `claude --version` answers, so `profile usage` reaches the probe;
    // only `claude -p /usage` stalls. A stalled version check is the next
    // test.
    fx.stub(
        "claude",
        "case \"$1\" in\n  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;\nesac\nexec sleep 30",
    );
    let (out, took) = fx.run(
        &["workstreams", "status", "--logins-deadline", "2"],
        &[("AMPLIHACK_SESSION_ID", "me")],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(took < Duration::from_secs(10), "took {took:?}");
    let text = stdout(&out);
    assert_display_safe(&text);
    assert!(text.contains("  LOGIN "), "{text}");
    let row = text
        .lines()
        .find(|l| l.contains("spare"))
        .unwrap_or_else(|| panic!("no spare row:\n{text}"));
    assert!(row.contains("no reading (timeout)"), "{row}");
}

/// Found by outside-in testing: at the deadline the command exited while the
/// `claude --version` check (before any usage probe) was still running, and
/// that process was left behind under PID 1 for as long as it hung.
#[test]
fn a_stalled_claude_is_not_left_running_after_the_deadline() {
    let fx = Fx::new();
    fx.add_profile("spare");
    let pids = fx.marks.join("claude-pids");
    fx.stub(
        "claude",
        &format!("echo $$ >> '{}'\nexec sleep 30", pids.display()),
    );
    let (out, _) = fx.run(
        &["workstreams", "status", "--logins-deadline", "2"],
        &[("AMPLIHACK_SESSION_ID", "me")],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let started = fs::read_to_string(&pids).expect("the stub claude ran");
    let alive = |pid: &str| {
        fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
            s.rsplit(')')
                .next()
                .is_some_and(|r| !r.trim_start().starts_with('Z'))
        })
    };
    let until = Instant::now() + Duration::from_secs(3);
    let mut left: Vec<&str> = started.split_whitespace().collect();
    while !left.is_empty() && Instant::now() < until {
        left.retain(|p| alive(p));
        std::thread::sleep(Duration::from_millis(50));
    }
    for pid in &left {
        let _ = std::process::Command::new("kill")
            .args(["-9", pid])
            .status();
    }
    assert!(left.is_empty(), "claude still running after exit: {left:?}");
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
    for bad in ["0", "1", "61"] {
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

/// `--logins-deadline` for the two deadline tests below.
///
/// `workstreams status` gives each probe `--timeout-secs` one second under
/// the deadline, counted from that probe's own start. A probe that starts
/// in the first second of `profile usage` is therefore ended by its own
/// timeout before the deadline, which would test the probe timeout and not
/// the deadline. So the probes that must still be running at the deadline
/// are queued behind a first pool turn of [`FIRST_TURN`] probes that sleep
/// [`FIRST_TURN_SECS`] and exit. A sleep is never shorter than asked, so
/// those probes start more than a second after `profile usage` and their
/// own timeout cannot come first, however loaded the host. Load can only
/// delay their start; the deadline leaves `DEADLINE - FIRST_TURN_SECS`
/// seconds for that.
const DEADLINE: u64 = 10;
/// `AMPLIHACK_PROFILE_USAGE_PARALLEL` in [`Fx::run`]: one pool turn.
const FIRST_TURN: usize = 8;
const FIRST_TURN_SECS: u64 = 2;

/// A `claude` stub for the deadline tests. Each probe appends its login to
/// `started` (the profile name, or `home` when `CLAUDE_CONFIG_DIR` is
/// unset). The profiles in `quick` sleep [`FIRST_TURN_SECS`] and exit;
/// every other login starts a 600 s `sleep` in its process group, records
/// `<stub pid> <sleep pid>` in `pids`, and waits.
fn deadline_stub(fx: &Fx, quick: &str) -> (PathBuf, PathBuf) {
    let started = fx.marks.join("started");
    let pids = fx.marks.join("usage-pids");
    fx.stub(
        "claude",
        &format!(
            "case \"$1\" in\n --version) echo '2.1.301 (Claude Code)'; exit 0 ;;\nesac\n\
             name=\"${{CLAUDE_CONFIG_DIR:+${{CLAUDE_CONFIG_DIR##*/}}}}\"\n\
             name=\"${{name:-home}}\"\n\
             echo \"$name\" >> '{started}'\n\
             case \"$name\" in\n {quick}) sleep {FIRST_TURN_SECS}; exit 0 ;;\nesac\n\
             sleep 600 &\n\
             echo \"$$ $!\" >> '{pids}'\n\
             wait",
            started = started.display(),
            pids = pids.display(),
        ),
    );
    (started, pids)
}

/// The logins each probe recorded, sorted: the pool's start order is not
/// part of what these tests prove.
fn started_logins(started: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_to_string(started)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    names.sort();
    names
}

/// Only a backstop: the hung stubs sleep 600 s, so a status that waited for
/// them instead of cancelling them would take far longer. The proof that
/// the deadline cancelled them is the count of probes and the
/// `no reading within` row.
const BACKSTOP: Duration = Duration::from_secs(120);

#[test]
fn deadline_cancels_and_reaps_usage_probes_in_later_batches() {
    let fx = Fx::new();
    // Turn one: spare-00..07 sleep and exit. Turn two: spare-08..15 hang
    // until the deadline. spare-16 waits for a free worker, and none is
    // free before the deadline: turn two's own timeouts end after it.
    for n in 0..=2 * FIRST_TURN {
        fx.add_profile(&format!("spare-{n:02}"));
    }
    let (started, pids) = deadline_stub(&fx, "spare-0[0-7]");
    let (out, took) = fx.run(
        &[
            "workstreams",
            "status",
            "--logins-deadline",
            &DEADLINE.to_string(),
        ],
        &[("AMPLIHACK_SESSION_ID", "me")],
    );
    let hung = assert_usage_probes_reaped(&pids);
    let names = started_logins(&started);
    let want: Vec<String> = (0..2 * FIRST_TURN)
        .map(|n| format!("spare-{n:02}"))
        .collect();
    assert_eq!(
        names,
        want,
        "turns one and two start, the queued spare-16 never does; stdout: {}; stderr: {}",
        stdout(&out),
        stderr(&out)
    );
    assert_eq!(hung, FIRST_TURN, "every turn-two probe was cancelled");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains(&format!("no reading within {DEADLINE}s")),
        "{}",
        stdout(&out)
    );
    assert!(took < BACKSTOP, "took {took:?}");
}

#[test]
fn deadline_cancels_and_reaps_the_live_home_login_probe() {
    let fx = Fx::new();
    // Eight profiles fill the first pool turn, sleep and exit. The login
    // live at home is the next job, and is still running at the deadline.
    for n in 0..FIRST_TURN {
        fx.add_profile(&format!("spare-{n:02}"));
    }
    fx.add_profile("zz-live");
    fs::create_dir_all(fx.home.join(".claude")).unwrap();
    let rotate = fx.profiles.join(".auto-switch.d/rotate");
    fs::create_dir_all(&rotate).unwrap();
    for path in [
        fx.home.join(".claude"),
        fx.profiles.join(".auto-switch.d"),
        rotate.clone(),
    ] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for (path, body) in [
        (
            fx.home.join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"test-only"}}"#,
        ),
        (
            fx.home.join(".claude.json"),
            r#"{"oauthAccount":{"accountUuid":"home-account","emailAddress":"home@example.com"}}"#,
        ),
        (
            rotate.join("ledger.json"),
            r#"{"version":1,"live":"zz-live","account":"home-account","since":10}"#,
        ),
    ] {
        fs::write(&path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let (started, pids) = deadline_stub(&fx, "spare-0[0-7]");
    let (out, took) = fx.run(
        &[
            "workstreams",
            "status",
            "--logins-deadline",
            &DEADLINE.to_string(),
        ],
        &[("AMPLIHACK_SESSION_ID", "me")],
    );
    let hung = assert_usage_probes_reaped(&pids);
    let mut want: Vec<String> = (0..FIRST_TURN).map(|n| format!("spare-{n:02}")).collect();
    // zz-live is live in ~/.claude, so its probe runs without
    // CLAUDE_CONFIG_DIR.
    want.push("home".into());
    want.sort();
    assert_eq!(
        started_logins(&started),
        want,
        "stdout: {}; stderr: {}",
        stdout(&out),
        stderr(&out)
    );
    assert_eq!(hung, 1, "the home probe was cancelled");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains(&format!("no reading within {DEADLINE}s")),
        "{}",
        stdout(&out)
    );
    assert!(took < BACKSTOP, "took {took:?}");
}

/// Check direct children were reaped and their descendants stopped before
/// status returned. Clean up leaked helpers even when the regression fails.
fn assert_usage_probes_reaped(pids: &Path) -> usize {
    let started = fs::read_to_string(pids).expect("usage probes ran");
    let owners: Vec<_> = started
        .lines()
        .map(|line| line.split_whitespace().next().unwrap())
        .collect();
    // A killed but unreaped supervisor is also a failure: profile usage
    // must join the workers that reap these direct children before exit.
    let unreaped: Vec<_> = owners
        .iter()
        .copied()
        .filter(|pid| Path::new(&format!("/proc/{pid}")).exists())
        .collect();
    let pids: Vec<_> = started.split_whitespace().collect();
    let left: Vec<_> = pids
        .iter()
        .copied()
        .filter(|pid| {
            fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
                s.rsplit(')')
                    .next()
                    .is_some_and(|r| !r.trim_start().starts_with('Z'))
            })
        })
        .collect();
    // Clean up on failure, too, so the regression never leaks test helpers.
    for pid in &left {
        let _ = std::process::Command::new("kill")
            .args(["-9", pid])
            .status();
    }
    assert!(unreaped.is_empty(), "probes not reaped: {unreaped:?}");
    assert!(
        left.is_empty(),
        "probes alive after status returned: {left:?}"
    );
    owners.len()
}
