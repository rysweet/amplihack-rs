//! Issue #1745: `amplihack profile statusline` with no `--owner`, the command
//! `amplihack install` writes into `~/.claude/settings.json`. It runs
//! `~/.amplihack/.claude/tools/statusline.sh` with the same input, then prints
//! the login capacity line for the login the rotator ledger names, and
//! records the reading for that login.
//!
//! Runs the real `amplihack` binary through `isolated(..)` with a temp
//! `HOME`, a temp profile root and a fake `statusline.sh`. Nothing touches the
//! real `~/.claude`, `~/.amplihack` or profile root.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const HOUR: i64 = 3600;
const LINE_34_43: &str = "🔑 X  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%";
const PREFIX_LOW: &str = "⚠ all Claude accounts ≥90% · next reset ";
/// The first line the fake `statusline.sh` prints.
const FIRST: &str = "#!/bin/sh\ncat >/dev/null\nprintf 'first\\n'\n";

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn mode(path: &Path, m: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(m)).unwrap();
}

fn private(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    mode(path, 0o600);
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Status-line input like Claude Code's, with the given usage.
fn input(five: f64, seven: f64) -> Vec<u8> {
    serde_json::json!({
        "session_id": "s",
        "model": {"id": "m"},
        "rate_limits": {
            "five_hour": {"used_percentage": five, "resets_at": now() + HOUR},
            "seven_day": {"used_percentage": seven, "resets_at": now() + 3 * 86_400},
        }
    })
    .to_string()
    .into_bytes()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    work: PathBuf,
}

impl Fixture {
    /// Profiles `X` and `Y` under a private root, neither logged in, and a
    /// login in `~/.claude`. With `script`, a private `statusline.sh` with
    /// that body. No ledger yet.
    fn new(script: Option<&str>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let work = tmp.path().join("work");
        for d in [&home, &work] {
            fs::create_dir_all(d).unwrap();
        }
        mode(&home, 0o700);
        let mut f = Self {
            root: home.join("profiles"),
            _tmp: tmp,
            home,
            work,
        };
        for n in ["X", "Y"] {
            f.ok(&["profile", "add", n], "0");
        }
        f.root = f.root.canonicalize().unwrap();
        private(&f.home.join(".claude/.credentials.json"), b"{}");
        if let Some(body) = script {
            f.script(body);
        }
        f
    }

    fn script(&self, body: &str) -> PathBuf {
        let p = self.home.join(".amplihack/.claude/tools/statusline.sh");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        mode(&p, 0o700);
        for d in [
            ".amplihack",
            ".amplihack/.claude",
            ".amplihack/.claude/tools",
        ] {
            mode(&self.home.join(d), 0o700);
        }
        p
    }

    fn prototype_ledger(&self, body: &str) {
        let p = self.root.join(".rotator/ledger.json");
        private(&p, body.as_bytes());
        mode(p.parent().unwrap(), 0o700);
    }

    /// `profile rotate`'s ledger, without an `account`.
    fn rotate_ledger(&self, live: &str) {
        let p = self.root.join(".auto-switch.d/rotate/ledger.json");
        private(
            &p,
            serde_json::json!({"version": 1, "live": live, "since": 10})
                .to_string()
                .as_bytes(),
        );
        mode(&self.root.join(".auto-switch.d"), 0o700);
        mode(p.parent().unwrap(), 0o700);
    }

    fn usage(&self) -> Option<serde_json::Value> {
        let raw = fs::read(self.root.join(".auto-switch.d/usage.json")).ok()?;
        Some(serde_json::from_slice(&raw).unwrap())
    }

    fn plant_usage(&self, accounts: serde_json::Value) {
        let dir = self.root.join(".auto-switch.d");
        fs::create_dir_all(&dir).unwrap();
        mode(&dir, 0o700);
        private(
            &dir.join("usage.json"),
            serde_json::json!({"version": 1, "accounts": accounts})
                .to_string()
                .as_bytes(),
        );
    }

    fn cmd(&self, args: &[&str], toggle: &str) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("TZ", "UTC")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", toggle)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root);
        c
    }

    fn ok(&self, args: &[&str], toggle: &str) -> Output {
        let out = self.cmd(args, toggle).output().unwrap();
        assert!(
            out.status.success(),
            "`amplihack {}` failed: {}\n{}",
            args.join(" "),
            text(&out.stdout),
            text(&out.stderr)
        );
        out
    }

    /// One run of `profile statusline` with `args`, `CLAUDE_CONFIG_DIR` set
    /// to `config_dir` when given, and `stdin` as input.
    fn run(&self, args: &[&str], toggle: &str, config_dir: Option<&Path>, stdin: &[u8]) -> Output {
        self.run_with(args, toggle, config_dir, stdin, |_| {})
    }

    fn run_with(
        &self,
        args: &[&str],
        toggle: &str,
        config_dir: Option<&Path>,
        stdin: &[u8],
        adjust: impl FnOnce(&mut Command),
    ) -> Output {
        let mut full = vec!["profile", "statusline"];
        full.extend_from_slice(args);
        let mut c = self.cmd(&full, toggle);
        if let Some(d) = config_dir {
            c.env("CLAUDE_CONFIG_DIR", d);
        }
        adjust(&mut c);
        let mut child = c
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pipe = child.stdin.take().unwrap();
        let bytes = stdin.to_vec();
        let writer = std::thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        });
        let out = child.wait_with_output().unwrap();
        writer.join().unwrap();
        out
    }

    /// The `~/.claude` command: no arguments, no `CLAUDE_CONFIG_DIR`, with
    /// auto-switch off.
    fn home_tick(&self, stdin: &[u8]) -> Output {
        self.run(&[], "0", None, stdin)
    }

    /// Writes the no-owner command into profile `X`'s settings, as the
    /// shared settings keys copy it, and installs the wrapper around it.
    fn wrap_home_command_in_x(&self) {
        let cmd = format!("'{}' profile statusline", amplihack_bin().to_str().unwrap());
        let settings = serde_json::json!({"statusLine": {"type": "command", "command": cmd}});
        fs::write(
            self.root.join("X/settings.json"),
            serde_json::to_vec_pretty(&settings).unwrap(),
        )
        .unwrap();
        let out = self.ok(&["profile", "auto-switch", "install"], "0");
        assert!(
            !text(&out.stdout).contains("stale"),
            "the home command is an original, not a stale wrapper: {}",
            text(&out.stdout)
        );
    }
}

// ---------------------------------------------------------------------------
// No-owner mode: the owner
// ---------------------------------------------------------------------------

#[test]
fn the_prototype_ledger_names_the_owner_and_the_reading_is_recorded() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let out = f.home_tick(&input(34.0, 43.0));
    assert_eq!(
        text(&out.stdout),
        format!("first\n{LINE_34_43}  · 0 spare\n"),
        "stderr: {}",
        text(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0));
    let usage = f
        .usage()
        .expect("the reading was recorded with auto-switch off");
    assert_eq!(usage["accounts"]["X"]["five_hour"]["used_percentage"], 34.0);
    assert_eq!(usage["accounts"]["X"]["seven_day"]["used_percentage"], 43.0);
}

#[test]
fn the_rotate_ledger_names_the_owner() {
    let f = Fixture::new(Some(FIRST));
    f.rotate_ledger("X");
    let out = f.home_tick(&input(34.0, 43.0));
    let stdout = text(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert_eq!(lines[0], "first");
    assert!(lines[1].starts_with("🔑 X"), "{stdout}");
    assert!(f.usage().unwrap()["accounts"].get("X").is_some());
}

#[test]
fn an_explicit_absolute_root_is_used() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let root = f.root.to_str().unwrap().to_string();
    let out = f.run_with(&["--root", &root], "0", None, &input(34.0, 43.0), |c| {
        c.env_remove("AMPLIHACK_CLAUDE_PROFILE_ROOT");
    });
    assert!(
        text(&out.stdout).contains(LINE_34_43),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn a_relative_root_prints_the_first_line_only() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let out = f.run(&["--root", "profiles"], "0", None, &input(34.0, 43.0));
    assert_eq!(text(&out.stdout), "first\n");
    assert_eq!(out.status.code(), Some(0));
    assert!(f.usage().is_none());
}

#[test]
fn a_spare_profile_is_counted() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    f.plant_usage(serde_json::json!({"Y": {
        "five_hour": {"used_percentage": 20.0, "resets_at": now() + HOUR},
        "seven_day": {"used_percentage": 30.0, "resets_at": now() + 3 * 86_400},
        "observed_at": now() - 60,
        "source": "statusline",
    }}));
    let out = f.home_tick(&input(34.0, 43.0));
    assert_eq!(
        text(&out.stdout),
        format!("first\n{LINE_34_43}  · 1 spare\n")
    );
}

// ---------------------------------------------------------------------------
// No-owner mode: failures give less output
// ---------------------------------------------------------------------------

#[test]
fn a_missing_ledger_prints_the_first_line_only() {
    let f = Fixture::new(Some(FIRST));
    let out = f.home_tick(&input(34.0, 43.0));
    assert_eq!(text(&out.stdout), "first\n");
    assert_eq!(out.status.code(), Some(0));
    assert!(f.usage().is_none());
    assert!(
        !text(&out.stderr).contains(f.root.to_str().unwrap()),
        "no paths: {}",
        text(&out.stderr)
    );
}

#[test]
fn a_damaged_ledger_prints_the_first_line_only() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger("not json");
    let out = f.home_tick(&input(34.0, 43.0));
    assert_eq!(text(&out.stdout), "first\n");
    assert_eq!(out.status.code(), Some(0));
    assert!(f.usage().is_none());
}

#[test]
fn without_rate_limits_only_the_first_line_is_printed() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let out = f.home_tick(br#"{"session_id":"s"}"#);
    assert_eq!(text(&out.stdout), "first\n");
}

#[test]
fn a_missing_script_prints_only_the_capacity_line() {
    let f = Fixture::new(None);
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let out = f.home_tick(&input(34.0, 43.0));
    assert_eq!(text(&out.stdout), format!("{LINE_34_43}  · 0 spare\n"));
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn a_world_writable_script_is_not_run() {
    let f = Fixture::new(Some(FIRST));
    let p = f.script("#!/bin/sh\nprintf 'unsafe\\n'\n");
    mode(&p, 0o707);
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let out = f.home_tick(&input(34.0, 43.0));
    assert!(
        !text(&out.stdout).contains("unsafe"),
        "{}",
        text(&out.stdout)
    );
    assert!(
        text(&out.stdout).starts_with("🔑 X"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn nothing_at_all_prints_nothing_and_exits_zero() {
    let f = Fixture::new(None);
    let out = f.home_tick(&input(34.0, 43.0));
    assert_eq!(out.stdout, b"");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn an_over_limit_reading_shows_one_hundred() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let out = f.home_tick(&input(130.0, 43.0));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("🔑 X  5h ██████████ 100%  7d "), "{stdout}");
    assert_eq!(out.status.code(), Some(0));
}

// ---------------------------------------------------------------------------
// No-owner mode: the script's exit code and input
// ---------------------------------------------------------------------------

#[test]
fn the_script_exit_code_passes_through_after_the_capacity_line() {
    let f = Fixture::new(Some("#!/bin/sh\ncat >/dev/null\nprintf 'first'\nexit 3\n"));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let out = f.home_tick(&input(34.0, 43.0));
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(
        text(&out.stdout),
        format!("first\n{LINE_34_43}  · 0 spare\n"),
        "a missing newline is added before the capacity line"
    );
}

#[test]
fn the_script_gets_the_same_input() {
    let f = Fixture::new(Some("#!/bin/sh\ncat\n"));
    let raw = input(34.0, 43.0);
    let out = f.home_tick(&raw);
    assert_eq!(out.stdout, raw, "no ledger: the script's output alone");
}

#[test]
fn input_over_one_mib_reaches_the_script_but_records_nothing() {
    let f = Fixture::new(Some("#!/bin/sh\nwc -c | tr -d ' '\n"));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let mut raw = input(34.0, 43.0);
    raw.pop();
    raw.extend_from_slice(b",\"pad\":\"");
    raw.extend(std::iter::repeat_n(b'x', (1 << 20) + 1));
    raw.extend_from_slice(b"\"}");
    let out = f.home_tick(&raw);
    assert_eq!(text(&out.stdout), format!("{}\n", raw.len()));
    assert!(f.usage().is_none());
}

// ---------------------------------------------------------------------------
// The inner-run rule
// ---------------------------------------------------------------------------

#[test]
fn inside_a_profile_only_the_first_line_is_printed() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let out = f.run(&[], "0", Some(&f.root.join("Y")), &input(34.0, 43.0));
    assert_eq!(text(&out.stdout), "first\n");
    assert!(f.usage().is_none());
}

#[test]
fn claude_config_dir_naming_the_default_directory_is_not_an_inner_run() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let dot = f.home.join(".claude");
    let slash = PathBuf::from(format!("{}/", dot.display()));
    for dir in [dot, slash] {
        let out = f.run(&[], "0", Some(&dir), &input(34.0, 43.0));
        assert!(
            text(&out.stdout).contains(LINE_34_43),
            "{}: {}",
            dir.display(),
            text(&out.stdout)
        );
    }
}

// ---------------------------------------------------------------------------
// Inside profiles: the wrapper around the no-owner command
// ---------------------------------------------------------------------------

#[test]
fn the_wrapper_prints_the_original_output_then_the_capacity_line() {
    let f = Fixture::new(Some(FIRST));
    f.wrap_home_command_in_x();
    let root = f.root.to_str().unwrap().to_string();
    let out = f.run(
        &["--root", &root, "--owner", "X"],
        "0",
        Some(&f.root.join("X")),
        &input(34.0, 43.0),
    );
    let stdout = text(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "one capacity line, never two: {stdout}");
    assert_eq!(lines[0], "first");
    assert!(lines[1].starts_with(LINE_34_43), "{stdout}");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn owner_without_root_is_refused() {
    let f = Fixture::new(Some(FIRST));
    f.wrap_home_command_in_x();
    let out = f.run(
        &["--owner", "X"],
        "0",
        Some(&f.root.join("X")),
        &input(34.0, 43.0),
    );
    assert_eq!(text(&out.stdout), "", "stdout stays empty");
    assert!(
        text(&out.stderr).contains("status line disabled"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(f.usage().is_none(), "nothing recorded");
}

#[test]
fn the_wrapper_puts_the_warning_after_the_capacity_line() {
    let f = Fixture::new(Some(FIRST));
    f.wrap_home_command_in_x();
    let root = f.root.to_str().unwrap().to_string();
    // Y is over too, so X's reading puts every account over the threshold.
    f.plant_usage(serde_json::json!({"Y": {
        "five_hour": {"used_percentage": 95.0, "resets_at": now() + HOUR},
        "seven_day": {"used_percentage": 95.0, "resets_at": now() + 3 * 86_400},
        "observed_at": now() - 60,
        "source": "statusline",
    }}));
    let out = f.run(
        &["--root", &root, "--owner", "X"],
        "1",
        Some(&f.root.join("X")),
        &input(95.0, 10.0),
    );
    let stdout = text(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{stdout}");
    assert_eq!(lines[0], "first");
    assert!(
        lines[1] == "🔑 X  5h █████████░ 95%  7d █░░░░░░░░░ 10%  · 0 spare",
        "{stdout}"
    );
    assert!(lines[2].starts_with(PREFIX_LOW), "{stdout}");
}

#[test]
fn a_wrapper_outside_its_session_adds_no_capacity_line() {
    let f = Fixture::new(Some(FIRST));
    f.wrap_home_command_in_x();
    let root = f.root.to_str().unwrap().to_string();
    let out = f.run(
        &["--root", &root, "--owner", "X"],
        "0",
        None,
        &input(34.0, 43.0),
    );
    assert!(!text(&out.stdout).contains("🔑"), "{}", text(&out.stdout));
}
