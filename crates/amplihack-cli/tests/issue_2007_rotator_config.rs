//! Issue #2007: the `rotator.*` keys, end to end through the real binary.
//!
//! Every run uses the rotate `Fixture` (temporary HOME and Claude profile
//! root, fake `claude`, `AMPLIHACK_TEST_HOME_GUARD` set) and writes the user
//! file `<HOME>/.amplihack/config.toml` with an explicit mode, so the
//! runner's umask cannot change the outcome. The Codex test uses a fake
//! `codex app-server` that answers every account with a credit balance; no
//! login file is read and no account is contacted. Nothing here sleeps,
//! retries or measures wall-clock time; waits are on exit status and on the
//! rotator's pid file and process.
//!
//! The contract is docs/reference/user-config.md ("Rotator keys") and
//! docs/reference/profile-command.md ("Thresholds in force"):
//!
//! - precedence: flag, then `AMPLIHACK_ROTATE_*`, then the user file key,
//!   then the default (85, 90, 600, 3000);
//! - `profile usage` prints `thresholds: session-at <n>% (<origin>), ...`
//!   and `--summary-json` gains a `thresholds` object; `--json` is
//!   unchanged;
//! - a user file group or others can write fails closed: the rotator
//!   refuses with the whole-file fix and no path, `profile usage` still
//!   exits 0 with `error` on every threshold.

#![cfg(target_os = "linux")]

use crate::support::exec_stub::write_executable;
use crate::support::fake_proc::FakeProc;
use crate::support::profile_rotate::{Fixture, alive, assert_no_secrets, text};
use crate::support::wait::output_within;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const WHOLE_FILE_FIX: &str = "fix the user config file first";

/// `<HOME>/.amplihack/config.toml` holding `body`, at `mode`.
fn user_config(f: &Fixture, body: &str, mode: u32) -> PathBuf {
    let dir = f.home.join(".amplihack");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let file = dir.join("config.toml");
    // Keep whatever `profile add` saved (the profile root) above our keys.
    let mut text = fs::read_to_string(&file).unwrap_or_default();
    text.push('\n');
    text.push_str(body);
    fs::write(&file, text).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
    file
}

fn run(c: &mut Command, what: &str) -> Output {
    let out = output_within(what, c);
    assert_no_secrets(&out);
    out
}

fn ok(c: &mut Command, what: &str) -> Output {
    let out = run(c, what);
    assert!(
        out.status.success(),
        "{what} failed: {}\nstdout: {}\nstderr: {}",
        out.status,
        text(&out.stdout),
        text(&out.stderr)
    );
    out
}

fn summary_json(f: &Fixture, extra_env: &[(&str, &str)]) -> serde_json::Value {
    let mut c = f.cmd(&["profile", "usage", "--summary-json"]);
    for (k, v) in extra_env {
        c.env(k, v);
    }
    let out = ok(&mut c, "`profile usage --summary-json`");
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("--summary-json is not JSON ({e}): {}", text(&out.stdout)))
}

/// The `invalid rotate setting` lines of `stderr`. Other lines (the launcher's
/// own notes) may name the temporary HOME and are not part of the contract.
fn setting_errors(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("invalid rotate setting"))
        .collect();
    assert!(
        !lines.is_empty(),
        "no `invalid rotate setting` line:\n{stderr}"
    );
    lines.join("\n")
}

fn thresholds_line(stdout: &str) -> &str {
    stdout
        .lines()
        .find(|l| l.starts_with("thresholds: "))
        .unwrap_or_else(|| panic!("no thresholds line in:\n{stdout}"))
}

// --- config list and set ------------------------------------------------------

#[test]
fn config_list_shows_the_four_keys_in_schema_order_with_defaults() {
    let f = Fixture::new();
    let out = ok(&mut f.cmd(&["config", "list"]), "`config list`");
    let stdout = text(&out.stdout);
    let want = [
        "rotator.session_at = 85",
        "rotator.week_at = 90",
        "rotator.interval = 600",
        "rotator.codex_credit_floor = 3000",
    ];
    let at: Vec<usize> = want
        .iter()
        .map(|w| {
            stdout
                .lines()
                .position(|l| l.trim() == *w)
                .unwrap_or_else(|| panic!("{w:?} missing from:\n{stdout}"))
        })
        .collect();
    assert!(at.windows(2).all(|p| p[1] == p[0] + 1), "{at:?}\n{stdout}");
}

#[test]
fn config_set_rejects_out_of_range_and_non_integer_values_with_exit_2() {
    let f = Fixture::new();
    for (key, value) in [
        ("rotator.session_at", "101"),
        ("rotator.session_at", "abc"),
        ("rotator.week_at", "0"),
        ("rotator.interval", "86401"),
        ("rotator.codex_credit_floor", "100000001"),
    ] {
        let out = run(
            &mut f.cmd(&["config", "set", key, value]),
            "`config set` of a bad value",
        );
        assert_eq!(out.status.code(), Some(2), "{key}={value}");
        assert!(text(&out.stderr).contains(key), "{}", text(&out.stderr));
    }
    let out = ok(
        &mut f.cmd(&["config", "get", "rotator.session_at"]),
        "`config get`",
    );
    assert_eq!(text(&out.stdout).trim(), "85", "nothing was written");
}

#[test]
fn config_set_then_get_round_trips_with_origin_user() {
    let f = Fixture::new();
    ok(
        &mut f.cmd(&["config", "set", "rotator.session_at", "70"]),
        "`config set`",
    );
    let out = ok(
        &mut f.cmd(&["config", "get", "rotator.session_at", "--show-origin"]),
        "`config get --show-origin`",
    );
    let s = text(&out.stdout);
    assert!(s.contains("70"), "{s}");
    assert!(s.to_lowercase().contains("user"), "{s}");
}

// --- one-shot rotate: precedence --------------------------------------------

/// `work` is live at 77% session: above 70 and 75, below 80 and 85. The
/// other logins are close enough (60%) that no upgrade swap applies (the
/// upgrade margin is 30 points), so only the session point decides.
fn at_77(f: &Fixture) {
    f.usage_direct("work", 77, 20);
    f.usage_direct("spare", 60, 20);
    f.usage_direct("extra", 60, 20);
}

fn would_swap(f: &Fixture, args: &[&str], env: &[(&str, &str)]) -> bool {
    let mut full = vec!["profile", "rotate", "--dry-run"];
    full.extend_from_slice(args);
    let mut c = f.cmd(&full);
    for (k, v) in env {
        c.env(k, v);
    }
    let out = ok(&mut c, "`profile rotate --dry-run`");
    text(&out.stdout)
        .lines()
        .any(|l| l.starts_with("would swap work->"))
}

#[test]
fn one_shot_uses_the_default_85_with_nothing_set() {
    let f = Fixture::new();
    at_77(&f);
    assert!(!would_swap(&f, &[], &[]), "77% is under the default 85");
}

#[test]
fn one_shot_uses_rotator_session_at_70_from_the_user_file() {
    let f = Fixture::new();
    at_77(&f);
    user_config(&f, "[rotator]\nsession_at = 70\n", 0o600);
    assert!(would_swap(&f, &[], &[]), "77% is over the configured 70");
}

#[test]
fn the_session_at_flag_beats_the_user_file() {
    let f = Fixture::new();
    at_77(&f);
    user_config(&f, "[rotator]\nsession_at = 70\n", 0o600);
    assert!(
        !would_swap(&f, &["--session-at", "80"], &[]),
        "--session-at 80 wins over rotator.session_at 70"
    );
}

#[test]
fn the_variable_beats_the_user_file_when_no_flag_is_given() {
    let f = Fixture::new();
    at_77(&f);
    user_config(&f, "[rotator]\nsession_at = 80\n", 0o600);
    assert!(
        would_swap(&f, &[], &[("AMPLIHACK_ROTATE_SESSION_AT", "75")]),
        "AMPLIHACK_ROTATE_SESSION_AT=75 wins over rotator.session_at 80"
    );
}

#[test]
fn a_project_file_cannot_move_the_session_point() {
    let f = Fixture::new();
    at_77(&f);
    let project = f.work_dir.join(".amplihack");
    fs::create_dir_all(f.work_dir.join(".git")).unwrap();
    fs::write(
        f.work_dir.join(".git").join("HEAD"),
        "ref: refs/heads/main\n",
    )
    .unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("config.toml"), "[rotator]\nsession_at = 10\n").unwrap();
    assert!(!would_swap(&f, &[], &[]), "the project file is ignored");
    let t = &summary_json(&f, &[])["thresholds"];
    assert_eq!(t["session_at"]["value"], 85, "{t}");
    assert_eq!(t["session_at"]["origin"], "default", "{t}");
}

#[test]
fn a_hand_written_out_of_range_value_refuses_and_names_the_key() {
    let f = Fixture::new();
    at_77(&f);
    user_config(&f, "[rotator]\nsession_at = 101\n", 0o600);
    let out = run(
        &mut f.cmd(&["profile", "rotate", "--dry-run"]),
        "`profile rotate --dry-run` with a bad key",
    );
    assert!(!out.status.success());
    let err = setting_errors(&text(&out.stderr));
    assert!(
        err.contains("invalid rotate setting rotator.session_at"),
        "{err}"
    );
    assert!(!err.contains(&f.home.display().to_string()), "{err}");
}

// --- fail closed on an unsafe user file ---------------------------------------

#[test]
fn a_group_writable_user_file_stops_the_rotator_without_quoting_the_path() {
    let f = Fixture::new();
    at_77(&f);
    user_config(&f, "[rotator]\nsession_at = 70\n", 0o660);
    let out = run(
        &mut f.cmd(&["profile", "rotate", "--dry-run"]),
        "`profile rotate --dry-run` with a group-writable file",
    );
    assert!(!out.status.success(), "fails closed, never the defaults");
    let err = setting_errors(&text(&out.stderr));
    assert!(err.contains(WHOLE_FILE_FIX), "{err}");
    assert!(!err.contains(&f.home.display().to_string()), "{err}");
}

#[test]
fn a_group_writable_user_file_leaves_profile_usage_working_with_errors() {
    let f = Fixture::new();
    user_config(&f, "[rotator]\nsession_at = 70\n", 0o660);
    let v = summary_json(&f, &[]);
    let t = &v["thresholds"];
    for k in ["session_at", "week_at", "interval", "codex_credit_floor"] {
        assert!(t[k]["value"].is_null(), "{k}: {t}");
        let e = t[k]["error"].as_str().unwrap_or_else(|| panic!("{k}: {t}"));
        assert!(e.contains(WHOLE_FILE_FIX), "{k}: {e}");
        assert!(!e.contains(&f.home.display().to_string()), "{k}: {e}");
    }
}

// --- profile usage ------------------------------------------------------------

#[test]
fn profile_usage_prints_the_default_thresholds_line() {
    let f = Fixture::new();
    let out = ok(&mut f.cmd(&["profile", "usage"]), "`profile usage`");
    let stdout = text(&out.stdout);
    assert_eq!(
        thresholds_line(&stdout),
        "thresholds: session-at 85% (default), week-at 90% (default, review reserve 10%), interval 600s (default), codex credit floor 3000 (default)"
    );
}

#[test]
fn profile_usage_shows_config_and_env_origins() {
    let f = Fixture::new();
    user_config(
        &f,
        "[rotator]\nsession_at = 70\nweek_at = 80\ncodex_credit_floor = 5000\n",
        0o600,
    );
    let mut c = f.cmd(&["profile", "usage"]);
    c.env("AMPLIHACK_ROTATE_INTERVAL", "120");
    let out = ok(&mut c, "`profile usage`");
    let stdout = text(&out.stdout);
    assert_eq!(
        thresholds_line(&stdout),
        "thresholds: session-at 70% (config), week-at 80% (config, review reserve 20%), interval 120s (env), codex credit floor 5000 (config)"
    );
}

#[test]
fn summary_json_has_the_thresholds_object() {
    let f = Fixture::new();
    user_config(&f, "[rotator]\nsession_at = 70\n", 0o600);
    let v = summary_json(&f, &[]);
    assert_eq!(
        v["thresholds"],
        serde_json::json!({
            "session_at": { "value": 70, "origin": "config" },
            "week_at": { "value": 90, "origin": "default", "review_reserve": 10 },
            "interval": { "value": 600, "origin": "default" },
            "codex_credit_floor": { "value": 3000, "origin": "default" }
        })
    );
}

#[test]
fn json_output_stays_an_array_without_thresholds() {
    let f = Fixture::new();
    user_config(&f, "[rotator]\nsession_at = 70\n", 0o600);
    let out = ok(
        &mut f.cmd(&["profile", "usage", "--json"]),
        "`profile usage --json`",
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v.is_array(), "--json is unchanged: {v}");
    assert!(!text(&out.stdout).contains("thresholds"), "{v}");
}

// --- Codex floor --------------------------------------------------------------

/// Answers every account with used-up rate limits and 4200 credits, so the
/// account counts only while the floor is below 4200.
const FAKE_CODEX: &str = r#"#!/bin/sh
[ "$1" = "app-server" ] || exit 64
IFS= read -r init || exit 65
printf '%s\n' '{"id":1,"result":{"userAgent":"fake-codex/0.0"}}'
IFS= read -r note || exit 66
IFS= read -r req || exit 67
printf '%s\n' '{"id":2,"result":{"rateLimits":{"credits":{"hasCredits":true,"balance":"4200"},"planType":"team"}}}'
exec cat >/dev/null
"#;

fn codex_usage(f: &Fixture) -> String {
    let base = f.home.parent().unwrap().to_path_buf();
    let root = base.join("codex-profiles");
    // An account directory with no login files: the fake never reads one.
    fs::create_dir_all(root.join("hhale")).unwrap();
    let bin = base.join("codex-bin").join("codex");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    write_executable(&bin, FAKE_CODEX, 0o755);
    let mut c = f.cmd(&["profile", "usage"]);
    c.env("CODEX_PROFILES_ROOT", &root).env("CODEX_BIN", &bin);
    text(&ok(&mut c, "`profile usage` with a fake codex").stdout)
}

fn hhale_line(stdout: &str) -> &str {
    stdout
        .lines()
        .find(|l| l.contains("hhale"))
        .unwrap_or_else(|| panic!("no hhale row in:\n{stdout}"))
}

#[test]
fn at_the_default_floor_a_credited_account_is_the_codex_pick() {
    let f = Fixture::new();
    let stdout = codex_usage(&f);
    assert!(hhale_line(&stdout).ends_with("<- next"), "{stdout}");
}

#[test]
fn a_floor_above_the_balance_drops_the_account_from_the_pick() {
    let f = Fixture::new();
    user_config(&f, "[rotator]\ncodex_credit_floor = 10000\n", 0o600);
    let stdout = codex_usage(&f);
    assert!(!hhale_line(&stdout).contains("<- next"), "{stdout}");
    assert!(!stdout.contains("<- next"), "nothing is picked: {stdout}");
    assert!(
        thresholds_line(&stdout).ends_with("codex credit floor 10000 (config)"),
        "{stdout}"
    );
}

// --- rotate start: the spawned rotator's argv --------------------------------

/// Stops the rotator `pid` when the test ends, through `rotate stop`.
struct Stop<'a> {
    f: &'a Fixture,
    pid: i32,
    _proc: &'a FakeProc,
}

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        if alive(self.pid) {
            let (out, refused) = self.f.stop_until_gone(self.pid);
            self.f.assert_stopped(&out, refused, self.pid);
        }
    }
}

/// Starts a rotator and returns its pid and its argv, read from `/proc`.
fn start<'a>(f: &'a Fixture, fake: &'a FakeProc, args: &[&str]) -> (Stop<'a>, Vec<String>) {
    f.with_proc(fake);
    let mut full = vec!["profile", "rotate", "start", "--hold", "0"];
    full.extend_from_slice(args);
    let out = ok(&mut f.cmd(&full), "`profile rotate start`");
    let stdout = text(&out.stdout);
    assert!(stdout.contains("rotate: started (pid "), "{stdout}");
    let pid: i32 = fs::read_to_string(f.rotate_dir().join("rotate.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    fake.link_own(pid as u32);
    let stop = Stop {
        f,
        pid,
        _proc: fake,
    };
    let argv: Vec<String> = fs::read(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .split(|b| *b == 0)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    assert!(
        stdout.contains("checking every "),
        "start names its interval: {stdout}"
    );
    (stop, argv)
}

fn value_after(argv: &[String], flag: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == flag)
        .and_then(|i| argv.get(i + 1).cloned())
}

#[test]
fn rotate_start_spawns_with_the_configured_interval_and_session_at() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    user_config(&f, "[rotator]\ninterval = 120\nsession_at = 70\n", 0o600);
    let fake = FakeProc::new();
    let (_stop, argv) = start(&f, &fake, &[]);
    assert_eq!(
        value_after(&argv, "--interval").as_deref(),
        Some("120"),
        "{argv:?}"
    );
    assert_eq!(
        value_after(&argv, "--session-at").as_deref(),
        Some("70"),
        "{argv:?}"
    );
    assert_eq!(
        value_after(&argv, "--week-at").as_deref(),
        Some("90"),
        "{argv:?}"
    );
}

#[test]
fn the_interval_flag_beats_the_configured_interval_at_spawn() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    user_config(&f, "[rotator]\ninterval = 120\n", 0o600);
    let fake = FakeProc::new();
    let (_stop, argv) = start(&f, &fake, &["--interval", "300"]);
    assert_eq!(
        value_after(&argv, "--interval").as_deref(),
        Some("300"),
        "{argv:?}"
    );
}

#[test]
fn rotate_start_refuses_a_group_writable_user_file_and_starts_nothing() {
    let f = Fixture::new();
    user_config(&f, "[rotator]\ninterval = 120\n", 0o660);
    let out = run(
        &mut f.cmd(&["profile", "rotate", "start", "--hold", "0"]),
        "`profile rotate start` with a group-writable file",
    );
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains(WHOLE_FILE_FIX),
        "{}",
        text(&out.stderr)
    );
    assert!(
        !f.rotate_dir().join("rotate.pid").exists(),
        "nothing started"
    );
}
