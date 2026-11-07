//! Issue #1745: `amplihack profile statusline` with no `--owner`, the command
//! `amplihack install` writes into `~/.claude/settings.json`. In `~/.claude`
//! it runs `~/.amplihack/.claude/tools/statusline.sh` with the same input,
//! then prints the login capacity line for the login the rotator ledgers
//! name, and records the reading when it can be trusted. In a profile's own
//! directory it runs as that profile's wrapper; anywhere else it runs the
//! script only.
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
const BARS: &str = "5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%";
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

    /// The readings home mode records, in `home-usage.json`.
    fn usage(&self) -> Option<serde_json::Value> {
        let raw = fs::read(self.root.join(".auto-switch.d").join("home-usage.json")).ok()?;
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
            // Issue #2001: `profile usage` never reads a Codex account here.
            .env(
                "CODEX_PROFILES_ROOT",
                self.home.with_file_name("no-codex-profiles"),
            )
            .env("CODEX_BIN", self.home.join("no-codex"))
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
fn a_relative_root_prints_the_bars_only() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let out = f.run(&["--root", "profiles"], "0", None, &input(34.0, 43.0));
    assert_eq!(text(&out.stdout), format!("first\n{BARS}\n"), "bars only");
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
fn a_missing_ledger_leaves_out_the_login() {
    let f = Fixture::new(Some(FIRST));
    let out = f.home_tick(&input(34.0, 43.0));
    assert_eq!(text(&out.stdout), format!("first\n{BARS}  · 0 spare\n"));
    assert_eq!(out.status.code(), Some(0));
    assert!(f.usage().is_none());
    assert!(
        !text(&out.stderr).contains(f.root.to_str().unwrap()),
        "no paths: {}",
        text(&out.stderr)
    );
}

#[test]
fn a_damaged_ledger_leaves_out_the_login() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger("not json");
    let out = f.home_tick(&input(34.0, 43.0));
    assert_eq!(text(&out.stdout), format!("first\n{BARS}  · 0 spare\n"));
    assert_eq!(out.status.code(), Some(0));
    assert!(f.usage().is_none());
}

#[test]
fn without_rate_limits_the_bars_are_left_out() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let out = f.home_tick(br#"{"session_id":"s"}"#);
    assert_eq!(text(&out.stdout), "first\n🔑 X  · 0 spare\n");
    assert!(f.usage().is_none());
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
    // No script, no root and no rate limits: no segment is left.
    let f = Fixture::new(None);
    let missing = f.home.join("no-such-root");
    let out = f.run_with(&[], "0", None, br#"{"session_id":"s"}"#, |c| {
        c.env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &missing);
    });
    assert_eq!(out.stdout, b"", "{}", text(&out.stderr));
    assert!(!missing.exists());
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn an_over_limit_reading_is_capped_at_one_hundred_by_the_bounds_check() {
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
    let missing = f.home.join("no-such-root");
    let out = f.run_with(&[], "0", None, &raw, |c| {
        c.env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &missing);
    });
    let mut want = raw.clone();
    want.extend_from_slice(format!("\n{BARS}\n").as_bytes());
    assert_eq!(text(&out.stdout), text(&want), "the input, then the bars");
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
    assert_eq!(
        text(&out.stdout),
        format!("{}\n🔑 X  · 0 spare\n", raw.len()),
        "no bars from input that is not parsed"
    );
    assert!(f.usage().is_none());
}

// ---------------------------------------------------------------------------
// Modes (D3)
// ---------------------------------------------------------------------------

#[test]
fn inside_a_profile_the_run_is_that_profiles() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let out = f.run(&[], "0", Some(&f.root.join("Y")), &input(34.0, 43.0));
    let stdout = text(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert_eq!(lines[0], "first", "statusline.sh, with nothing saved");
    assert!(
        lines[1].starts_with("🔑 Y  5h ███░░░░░░░ 34%"),
        "Y's line, not the ledger's X: {stdout}"
    );
    assert!(f.usage().is_none(), "auto-switch is off: nothing recorded");
}

#[test]
fn a_symlinked_config_dir_pointing_outside_the_root_is_passthrough() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let outside = f.home.join("elsewhere/Z");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, f.root.join("Z")).unwrap();
    let out = f.run(&[], "1", Some(&f.root.join("Z")), &input(34.0, 43.0));
    assert_eq!(text(&out.stdout), "first\n");
    assert!(f.usage().is_none());
}

#[test]
fn a_foreign_config_dir_is_passthrough() {
    let f = Fixture::new(Some(FIRST));
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let other = f.home.join("other");
    fs::create_dir_all(&other).unwrap();
    let out = f.run(&[], "1", Some(&other), &input(34.0, 43.0));
    assert_eq!(text(&out.stdout), "first\n");
    assert!(f.usage().is_none());
}

#[test]
fn claude_config_dir_naming_the_default_directory_is_home_mode() {
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
fn owner_without_root_uses_the_resolved_root() {
    let f = Fixture::new(Some(FIRST));
    f.wrap_home_command_in_x();
    let out = f.run(
        &["--owner", "X"],
        "0",
        Some(&f.root.join("X")),
        &input(34.0, 43.0),
    );
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("first\n"), "{stdout}");
    assert!(stdout.contains(LINE_34_43), "{stdout}");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn the_home_command_is_never_saved_and_never_run_as_an_original() {
    let f = Fixture::new(Some(FIRST));
    f.wrap_home_command_in_x();
    let saved = f.root.join(".auto-switch.d/statusline/X.json");
    let raw = fs::read_to_string(&saved).expect("the script is saved");
    assert!(!raw.contains("profile statusline"), "{raw}");
    // An older install saved the home command itself: it is not run, the
    // script runs in its place, and there is still one capacity line.
    let home = format!("'{}' profile statusline", amplihack_bin().to_str().unwrap());
    private(
        &saved,
        serde_json::json!({"type": "command", "command": home})
            .to_string()
            .as_bytes(),
    );
    let root = f.root.to_str().unwrap().to_string();
    let out = f.run(
        &["--root", &root, "--owner", "X"],
        "0",
        Some(&f.root.join("X")),
        &input(34.0, 43.0),
    );
    let stdout = text(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert_eq!(lines[0], "first");
    assert!(lines[1].starts_with(LINE_34_43), "{stdout}");
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
        lines[1] == "🔑 X  5h ██████████ 95%  7d █░░░░░░░░░ 10%  · 0 spare",
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

/// Crusty round 6: nothing is saved when `statusline.sh` fails its checks at
/// install, and a wrapper with no saved original prints nothing, as the
/// #1569 wrapper tests require. That lasts once the script is fixed, until
/// the `~/.claude` entry is put back in the profile and wrapped again, which
/// is what STATUSLINE.md tells the user to do.
#[test]
fn a_wrapper_with_no_saved_original_prints_nothing_until_it_is_wrapped_again() {
    let f = Fixture::new(None);
    f.wrap_home_command_in_x();
    assert!(!f.root.join(".auto-switch.d/statusline/X.json").exists());
    let root = f.root.to_str().unwrap().to_string();
    let args = ["--root", root.as_str(), "--owner", "X"];
    let x = f.root.join("X");
    let out = f.run(&args, "0", Some(&x), &input(34.0, 43.0));
    assert_eq!(text(&out.stdout), "", "{}", text(&out.stderr));
    f.script(FIRST);
    let out = f.run(&args, "0", Some(&x), &input(34.0, 43.0));
    assert_eq!(
        text(&out.stdout),
        "",
        "a fixed script alone changes nothing"
    );

    f.wrap_home_command_in_x();
    let out = f.run(&args, "0", Some(&x), &input(34.0, 43.0));
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("first\n🔑 X  5h"), "{stdout}");
}

// ---------------------------------------------------------------------------
// SR1 at run time: the home tick opens no credentials file
// ---------------------------------------------------------------------------

/// Watches `paths` with inotify for any open, so a test sees what the binary
/// actually opens rather than what its source text names. The calls a home
/// tick makes reach `rotate::identify` only through other modules, which no
/// text search follows (crusty, PR #1857 round 2).
#[cfg(target_os = "linux")]
struct OpenWatch {
    fd: i32,
    names: std::collections::HashMap<i32, PathBuf>,
}

#[cfg(target_os = "linux")]
impl OpenWatch {
    fn new(paths: &[PathBuf]) -> Self {
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: plain syscalls on a descriptor this struct owns.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(
            fd >= 0,
            "inotify_init1: {}",
            std::io::Error::last_os_error()
        );
        let mut names = std::collections::HashMap::new();
        for p in paths {
            let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
            // SAFETY: `c` is a valid NUL-terminated path for the call.
            let wd = unsafe { libc::inotify_add_watch(fd, c.as_ptr(), libc::IN_OPEN) };
            assert!(
                wd >= 0,
                "watch {}: {}",
                p.display(),
                std::io::Error::last_os_error()
            );
            names.insert(wd, p.clone());
        }
        Self { fd, names }
    }

    /// The watched paths opened since [`OpenWatch::new`].
    fn opened(&self) -> Vec<PathBuf> {
        let mut buf = [0u8; 8192];
        let mut seen = Vec::new();
        loop {
            // SAFETY: `buf` is writable for its whole length.
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                return seen;
            }
            let mut i = 0usize;
            while i < n as usize {
                // SAFETY: the kernel writes whole `inotify_event` records.
                let ev: libc::inotify_event =
                    unsafe { std::ptr::read_unaligned(buf[i..].as_ptr().cast()) };
                if ev.mask & libc::IN_OPEN != 0
                    && let Some(p) = self.names.get(&ev.wd)
                {
                    seen.push(p.clone());
                }
                i += std::mem::size_of::<libc::inotify_event>() + ev.len as usize;
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for OpenWatch {
    fn drop(&mut self) {
        // SAFETY: closes the descriptor `new` opened.
        unsafe { libc::close(self.fd) };
    }
}

/// A login for every profile as well as `~/.claude`, so any kind or login
/// lookup has a file to open.
#[cfg(target_os = "linux")]
fn logins(f: &Fixture) -> Vec<PathBuf> {
    let creds = vec![
        f.home.join(".claude/.credentials.json"),
        f.root.join("X/.credentials.json"),
        f.root.join("Y/.credentials.json"),
    ];
    for p in &creds[1..] {
        private(p, br#"{"claudeAiOauth":{}}"#);
    }
    creds
}

/// A `usage.json` entry over the default threshold, observed now.
#[cfg(target_os = "linux")]
fn over_threshold() -> serde_json::Value {
    serde_json::json!({
        "five_hour": {"used_percentage": 99.0, "resets_at": now() + HOUR},
        "seven_day": {"used_percentage": 43.0, "resets_at": now() + 3 * 86_400},
        "observed_at": now(),
        "source": "statusline",
    })
}

#[cfg(target_os = "linux")]
fn state_json(f: &Fixture, file: &str) -> Option<serde_json::Value> {
    let raw = fs::read(f.root.join(".auto-switch.d").join(file)).ok()?;
    Some(serde_json::from_slice(&raw).unwrap())
}

/// The most a profile tick could want from a login file: over the threshold,
/// with an overflow profile marked and another profile over it too. It
/// records and prints, but neither swaps nor writes the capacity file.
#[cfg(target_os = "linux")]
#[test]
fn a_profile_tick_without_owner_opens_no_credentials_file() {
    let f = Fixture::new(Some(FIRST));
    let creds = logins(&f);
    private(&f.root.join(".auto-switch"), b"enabled=on\noverflow=X\n");
    f.plant_usage(serde_json::json!({"X": over_threshold()}));
    let watch = OpenWatch::new(&creds);
    let out = f.run(&[], "1", Some(&f.root.join("Y")), &input(99.0, 43.0));
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("first\n🔑 Y"), "{stdout}");
    let usage = state_json(&f, "usage.json").unwrap();
    assert_eq!(usage["accounts"]["Y"]["five_hour"]["used_percentage"], 99.0);
    assert!(state_json(&f, "capacity.json").is_none(), "refreshed");
    assert_eq!(watch.opened(), Vec::<PathBuf>::new(), "credentials opened");
    assert!(creds[2].exists(), "no swap without a wrapper");
}

/// A hosts entry whose session has ended: `X` holds `Y`'s login and nothing
/// is running there, so recovery would hand the login back.
#[cfg(target_os = "linux")]
fn stale_host(f: &Fixture, creds: &[PathBuf]) -> Vec<u8> {
    fs::remove_file(&creds[2]).unwrap();
    let ledger = serde_json::json!({
        "version": 1,
        "hosts": {"X": {"live": "Y", "swapped_at": now() - HOUR, "pre_swap_fingerprint": null}},
    });
    let body = ledger.to_string().into_bytes();
    let dir = f.root.join(".auto-switch.d");
    fs::create_dir_all(&dir).unwrap();
    mode(&dir, 0o700);
    private(&dir.join("ledger.json"), &body);
    body
}

/// Recovery is the wrapper's: without `--owner` a stale hosts entry stays and
/// the hosted login does not move. The same tick with `--owner` does return
/// it, so this test would see a move.
#[cfg(target_os = "linux")]
#[test]
fn a_profile_tick_without_owner_moves_no_login_for_a_stale_host() {
    let f = Fixture::new(Some(FIRST));
    let creds = logins(&f);
    private(&f.root.join(".auto-switch"), b"enabled=on\n");
    let ledger = stale_host(&f, &creds);
    let hosted = fs::read(&creds[1]).unwrap();
    let y = f.root.join("Y");

    let out = f.run(&[], "1", Some(&y), &input(34.0, 43.0));
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(fs::read(&creds[1]).unwrap(), hosted, "the login moved");
    assert!(!creds[2].exists(), "the login reached its home");
    assert_eq!(
        fs::read(f.root.join(".auto-switch.d/ledger.json")).unwrap(),
        ledger,
        "the hosts entry changed"
    );

    let root = f.root.to_str().unwrap().to_string();
    f.run(
        &["--root", &root, "--owner", "Y"],
        "1",
        Some(&y),
        &input(34.0, 43.0),
    );
    assert!(creds[2].exists(), "the wrapper returns the login");
    assert_eq!(
        state_json(&f, "ledger.json").unwrap()["hosts"],
        serde_json::json!({})
    );
}

/// A swap journal means the live login is not settled: a tick without
/// `--owner` leaves the journal for the wrapper or `repair` and records
/// nothing, while still printing.
#[cfg(target_os = "linux")]
#[test]
fn a_profile_tick_without_owner_leaves_a_journal_and_records_nothing() {
    let f = Fixture::new(Some(FIRST));
    let creds = logins(&f);
    private(&f.root.join(".auto-switch"), b"enabled=on\n");
    let journal = serde_json::json!({
        "version": 1, "op": "swap", "dir": "Y", "from": "Y", "from_present": true,
        "to": "X", "phase": "J0", "started_at": now(), "pid": 1,
        "fingerprint": null, "refresh_lock_ino": null,
    })
    .to_string()
    .into_bytes();
    let dir = f.root.join(".auto-switch.d");
    fs::create_dir_all(&dir).unwrap();
    mode(&dir, 0o700);
    private(&dir.join("journal.json"), &journal);
    let before: Vec<_> = creds.iter().map(|c| fs::read(c).unwrap()).collect();

    let out = f.run(&[], "1", Some(&f.root.join("Y")), &input(34.0, 43.0));
    assert!(
        text(&out.stdout).starts_with("first\n🔑 Y"),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(fs::read(dir.join("journal.json")).unwrap(), journal);
    let after: Vec<_> = creds.iter().map(|c| fs::read(c).unwrap()).collect();
    assert_eq!(after, before, "a login file changed");
    assert!(state_json(&f, "usage.json").is_none(), "recorded mid-swap");
}

/// The `--owner` wrapper on an ordinary tick: under the threshold, with no
/// capacity notice in force and no overflow profile marked, it records the
/// reading and opens no login file.
#[cfg(target_os = "linux")]
#[test]
fn an_owner_tick_under_the_threshold_opens_no_credentials_file() {
    let f = Fixture::new(Some(FIRST));
    let creds = logins(&f);
    let root = f.root.to_str().unwrap().to_string();
    let watch = OpenWatch::new(&creds);
    let out = f.run(
        &["--root", &root, "--owner", "Y"],
        "1",
        Some(&f.root.join("Y")),
        &input(34.0, 43.0),
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let usage = state_json(&f, "usage.json").unwrap();
    assert_eq!(usage["accounts"]["Y"]["five_hour"]["used_percentage"], 34.0);
    assert_eq!(watch.opened(), Vec::<PathBuf>::new(), "credentials opened");
}

/// The recorded exception: at the threshold the `--owner` wrapper resolves
/// profile kinds, because the capacity file and the swap must tell an
/// API-key profile from a plan login (#1804). It writes the capacity notice,
/// and a later tick under the threshold clears it.
#[cfg(target_os = "linux")]
#[test]
fn an_owner_tick_at_the_threshold_resolves_kinds_and_keeps_the_notice_current() {
    let f = Fixture::new(Some(FIRST));
    let creds = logins(&f);
    f.plant_usage(serde_json::json!({"X": over_threshold()}));
    let root = f.root.to_str().unwrap().to_string();
    let args = ["--root", root.as_str(), "--owner", "Y"];
    let y = f.root.join("Y");
    let watch = OpenWatch::new(&creds);
    f.run(&args, "1", Some(&y), &input(99.0, 43.0));
    let capacity = state_json(&f, "capacity.json").expect("the notice was written");
    assert_eq!(capacity["state"], "low", "{capacity}");
    assert_ne!(watch.opened(), Vec::<PathBuf>::new(), "kinds were resolved");

    // The same second as the tick above would keep its worse reading.
    let mut earlier = over_threshold();
    earlier["observed_at"] = (now() - 60).into();
    f.plant_usage(serde_json::json!({"X": over_threshold(), "Y": earlier}));
    f.run(&args, "1", Some(&y), &input(20.0, 43.0));
    let capacity = state_json(&f, "capacity.json").unwrap();
    assert_eq!(capacity["state"], "ok", "{capacity}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_recorded_home_tick_with_auto_switch_on_opens_no_credentials_file() {
    let f = Fixture::new(Some(FIRST));
    let creds = logins(&f);
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    let watch = OpenWatch::new(&creds);
    let out = f.run(&[], "1", None, &input(34.0, 43.0));
    assert_eq!(
        text(&out.stdout),
        format!("first\n{LINE_34_43}  · 0 spare\n"),
        "stderr: {}",
        text(&out.stderr)
    );
    let usage = f.usage().expect("the tick recorded the reading");
    assert_eq!(usage["accounts"]["X"]["five_hour"]["used_percentage"], 34.0);
    assert_eq!(watch.opened(), Vec::<PathBuf>::new(), "credentials opened");
}

/// Crusty round 3: a reading taken in `~/.claude` while it holds some other
/// account is filed under the ledger's login, since nothing in home mode
/// can check the account. `profile usage` (and the rotator's `measure`,
/// which reads through the same passive step) must not report it as a
/// measurement of that login.
#[test]
fn profile_usage_never_reports_a_home_reading_as_passive() {
    let f = Fixture::new(None);
    f.prototype_ledger(r#"{"live":"X","since":1}"#);
    // Another account's figures, filed under X.
    let out = f.home_tick(&input(97.0, 98.0));
    assert!(text(&out.stdout).contains("97%"), "{}", text(&out.stdout));
    let home = f.usage().expect("the tick recorded the reading");
    assert_eq!(home["accounts"]["X"]["five_hour"]["used_percentage"], 97.0);
    assert!(!f.root.join(".auto-switch.d/usage.json").exists());

    let out = f
        .cmd(&["profile", "usage", "X", "--json"], "0")
        .env("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "3600")
        .env("AMPLIHACK_CLAUDE_BINARY_PATH", f.work.join("no-claude"))
        .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
        .env("AMPLIHACK_SKIP_REFLECTION", "1")
        .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
        .output()
        .unwrap();
    let stdout = text(&out.stdout);
    let rows: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("{e}: {stdout}\n{}", text(&out.stderr)));
    let x = rows["claude"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["profile"] == "X")
        .unwrap_or_else(|| panic!("no row for X: {stdout}"));
    assert_ne!(x["source"], "passive", "{x}");
    assert!(
        !stdout.contains("97"),
        "the home reading was reported: {stdout}"
    );
}

/// Home mode prints the capacity notice after the capacity line, reading it
/// from the capacity file the wrapper or `profile usage` wrote.
#[test]
fn a_home_tick_prints_the_capacity_notice_when_it_is_due() {
    let f = Fixture::new(Some(FIRST));
    f.rotate_ledger("X");
    let dir = f.root.join(".auto-switch.d");
    let notice = serde_json::json!({
        "version": 1,
        "state": "low",
        "threshold": 90,
        "over": ["X", "Y"],
        "earliest_reset": chrono::Utc::now() + chrono::Duration::hours(2),
        "written_at": chrono::Utc::now(),
        "writer": "statusline",
    });
    fs::create_dir_all(&dir).unwrap();
    mode(&dir, 0o700);
    private(&dir.join("capacity.json"), notice.to_string().as_bytes());
    private(&f.root.join(".auto-switch"), b"enabled=on\n");

    let stdout = text(&f.run(&[], "1", None, &input(34.0, 43.0)).stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{stdout}");
    assert_eq!(lines[0], "first");
    assert!(lines[1].starts_with("🔑 X"), "{stdout}");
    assert!(lines[2].starts_with(PREFIX_LOW), "{stdout}");
}

#[test]
fn a_home_tick_prints_no_notice_without_a_capacity_file() {
    let f = Fixture::new(Some(FIRST));
    f.rotate_ledger("X");
    private(&f.root.join(".auto-switch"), b"enabled=on\n");
    let stdout = text(&f.run(&[], "1", None, &input(34.0, 43.0)).stdout);
    assert_eq!(stdout.lines().count(), 2, "{stdout}");
}

/// A user's filter around amplihack's own command is saved by `install`. The
/// wrapper must run it, and the inner `profile statusline` must end at
/// `statusline.sh` instead of running the same original again, with or
/// without `--owner`.
#[test]
fn a_saved_filter_around_profile_statusline_runs_once_and_ends() {
    let f = Fixture::new(Some(FIRST));
    let mine = format!(
        "'{}' profile statusline | tr a-z A-Z",
        amplihack_bin().to_str().unwrap()
    );
    let settings = serde_json::json!({"statusLine": {"type": "command", "command": mine}});
    fs::write(
        f.root.join("X/settings.json"),
        serde_json::to_vec_pretty(&settings).unwrap(),
    )
    .unwrap();
    let out = f.ok(&["profile", "auto-switch", "install"], "0");
    assert!(
        text(&out.stdout).contains("wrapped existing status line"),
        "{}",
        text(&out.stdout)
    );

    let root = f.root.to_str().unwrap().to_string();
    let x = f.root.join("X");
    for args in [vec![], vec!["--root", root.as_str(), "--owner", "X"]] {
        let out = f.run(&args, "0", Some(&x), &input(34.0, 43.0));
        assert!(out.status.success(), "{}", text(&out.stderr));
        assert!(
            text(&out.stdout).starts_with("FIRST\n"),
            "{}",
            text(&out.stdout)
        );
        assert_eq!(text(&out.stdout).matches("FIRST").count(), 1, "ran twice");
    }
}
