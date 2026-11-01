//! Issue #1803: a rotator under a service manager owns rotation, setting
//! changes are traced, and a paused rotator reports an exhausted live login.
//!
//! Every process runs against the temporary HOME and root of the rotate
//! fixture, with a cleared environment, so `INVOCATION_ID` and
//! `AMPLIHACK_RECIPE_RUN_ID` are set only where a test sets them and nothing
//! inherits `AMPLIHACK_CLAUDE_PROFILE_ROOT` or a real HOME. Each rotator is
//! killed before its test ends.
//!
//! The launch gates are tested in `rotate/tests_owner.rs`: the fixture sets
//! the test guard, which ends a launch before those gates.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(30);
const ALERT: &str = "ALERT: auto-switch is off while the live login is exhausted";
const STATUS_ALERT: &str = "alert: auto-switch is off while the live login is exhausted";
/// Above `PID_MAX_LIMIT`: never a real process.
const FAKE_PID: u32 = 4_200_100;

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < WAIT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A foreground `rotate watch`, killed on drop.
struct Watch(Child);

impl Watch {
    fn pid(&self) -> u32 {
        self.0.id()
    }

    /// SIGTERM, as a service manager's stop sends.
    fn stop(&mut self) {
        // SAFETY: plain kill(2) of this test's own unreaped child.
        unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
        let start = Instant::now();
        while self.0.try_wait().unwrap().is_none() {
            assert!(start.elapsed() < WAIT, "watch ignored SIGTERM");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn is_running(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn watch(f: &Fixture, extra: &[&str], env: &[(&str, &str)]) -> Watch {
    let mut args = vec!["profile", "rotate", "watch", "--interval", "1"];
    args.extend_from_slice(extra);
    let mut c = f.cmd(&args);
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in env {
        c.env(k, v);
    }
    Watch(c.spawn().unwrap())
}

/// Kills the rotator a pid file names, if a test fails before stopping it.
struct Reaper(PathBuf);

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Ok(s) = fs::read_to_string(&self.0)
            && let Ok(pid) = s.trim().parse::<i32>()
            && pid > 1
            && String::from_utf8_lossy(
                &fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default(),
            )
            .contains("rotate")
        {
            // SAFETY: plain kill(2) of a rotator this test started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

fn state_dirs(f: &Fixture) {
    let d = f.rotate_dir();
    fs::create_dir_all(&d).unwrap();
    for p in [d.clone(), d.parent().unwrap().to_path_buf()] {
        fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn auto_switch_file(f: &Fixture, body: &str) {
    private_file(&f.root.join(".auto-switch"), body);
}

fn paused(f: &Fixture) {
    auto_switch_file(f, "enabled=off\n");
}

fn marker_path(f: &Fixture) -> PathBuf {
    f.rotate_dir().join("owner.json")
}

fn stopped_owner(f: &Fixture) {
    state_dirs(f);
    private_file(
        &marker_path(f),
        &format!(r#"{{"version":1,"unit":"amplihack-rotator.service","pid":{FAKE_PID}}}"#),
    );
}

fn live_is(f: &Fixture, name: &str) {
    state_dirs(f);
    private_file(
        &f.rotate_dir().join("ledger.json"),
        &serde_json::json!({"version": 1, "live": name}).to_string(),
    );
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn cached(f: &Fixture, account: &str, source: &str, pct: f64, resets_in: i64) {
    state_dirs(f);
    let t = now();
    let body = serde_json::json!({
        "version": 1,
        "accounts": {
            account: {
                "five_hour": {"used_percentage": pct, "resets_at": t + resets_in},
                "observed_at": t - 60,
                "source": source
            }
        }
    });
    private_file(&f.root.join(".auto-switch.d/usage.json"), &body.to_string());
}

/// The live login `work` at 95% of a session that resets in an hour.
fn exhausted_live(f: &Fixture) {
    live_is(f, "work");
    cached(f, "work", "statusline", 95.0, 3600);
}

fn status_with(f: &Fixture, env: &[(&str, &str)]) -> String {
    let mut c = f.cmd(&["profile", "rotate", "status"]);
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    text(&out.stdout)
}

fn status(f: &Fixture) -> String {
    status_with(f, &[("AMPLIHACK_ROTATE_AUTOSTART", "1")])
}

fn count(log: &str, what: &str) -> usize {
    log.lines().filter(|l| l.ends_with(what)).count()
}

fn idle_lines(log: &str) -> usize {
    log.lines().filter(|l| l.contains(" idle")).count()
}

fn marker(f: &Fixture) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(marker_path(f)).unwrap()).unwrap()
}

fn changes(f: &Fixture) -> Vec<String> {
    fs::read_to_string(f.rotate_dir().join("setting-changes.log"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

#[track_caller]
fn assert_usage_error(out: &Output) {
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
}

// --- The owner marker -------------------------------------------------------

#[test]
fn a_supervised_watch_records_itself_and_the_record_outlives_it() {
    let f = Fixture::new();
    paused(&f);
    let mut w = watch(&f, &["--supervised=amplihack-test.service"], &[]);
    let pid = w.pid();
    wait_for("the marker", || marker_path(&f).exists());
    assert_eq!(
        marker(&f),
        serde_json::json!({"version": 1, "unit": "amplihack-test.service", "pid": pid})
    );
    let mode = fs::metadata(marker_path(&f)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    wait_for("the log line", || {
        f.log().contains(&format!(
            "owner: recorded amplihack-test.service (pid {pid})"
        ))
    });
    let s = status(&f);
    assert!(
        s.contains(&format!(
            "owner: amplihack-test.service (pid {pid}, running)\ndeferring: yes\n"
        )),
        "{s}"
    );

    w.stop();
    assert!(marker_path(&f).exists(), "the marker stays");
    let s = status(&f);
    assert!(
        s.contains(&format!(
            "owner: amplihack-test.service (pid {pid}, not running)\ndeferring: yes\n"
        )),
        "{s}"
    );
}

#[test]
fn invocation_id_records_the_owner_without_storing_its_value() {
    let f = Fixture::new();
    paused(&f);
    let w = watch(&f, &[], &[("INVOCATION_ID", "0123456789abcdef")]);
    wait_for("the marker", || marker_path(&f).exists());
    let m = marker(&f);
    assert_eq!(m["pid"], w.pid());
    assert!(m["unit"].as_str().unwrap().ends_with(".service"), "{m}");
    let raw = fs::read_to_string(marker_path(&f)).unwrap();
    assert!(!raw.contains("0123456789abcdef"), "{raw}");
    assert!(!f.log().contains("0123456789abcdef"));
}

#[test]
fn a_plain_watch_records_no_owner() {
    let f = Fixture::new();
    paused(&f);
    let _w = watch(&f, &[], &[("INVOCATION_ID", "")]);
    wait_for("the idle line", || f.log().contains("idle"));
    assert!(!marker_path(&f).exists());
}

#[test]
fn a_spawned_watch_records_no_owner_even_with_invocation_id() {
    let f = Fixture::new();
    paused(&f);
    state_dirs(&f);
    let _w = watch(&f, &["--spawned"], &[("INVOCATION_ID", "abc")]);
    wait_for("the idle line", || f.log().contains("idle"));
    assert!(!marker_path(&f).exists());
}

#[test]
fn a_rotator_started_from_inside_a_service_records_no_owner() {
    let f = Fixture::new();
    paused(&f);
    let _reaper = Reaper(f.rotate_dir().join("rotate.pid"));
    let mut c = f.cmd(&["profile", "rotate", "start", "--interval", "1"]);
    let out = c.env("INVOCATION_ID", "abc").output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    wait_for("the idle line", || f.log().contains("idle"));
    assert!(!marker_path(&f).exists(), "spawn drops INVOCATION_ID");
    f.ok(&["profile", "rotate", "stop"]);
}

#[test]
fn a_watch_that_refuses_to_start_claims_nothing() {
    let f = Fixture::new();
    paused(&f);
    let first = watch(&f, &[], &[]);
    let rotate_pid = f.rotate_dir().join("rotate.pid");
    wait_for("the first rotator", || {
        fs::read_to_string(&rotate_pid).is_ok_and(|p| p.trim() == first.pid().to_string())
    });
    let out = f.run(&[
        "profile",
        "rotate",
        "watch",
        "--interval",
        "1",
        "--supervised=second.service",
    ]);
    assert!(!out.status.success(), "already running");
    assert!(!marker_path(&f).exists());
}

#[test]
fn a_marker_that_cannot_be_written_does_not_stop_the_rotator() {
    let f = Fixture::new();
    paused(&f);
    state_dirs(&f);
    fs::create_dir(marker_path(&f)).unwrap();
    let mut w = watch(&f, &["--supervised=a.service"], &[]);
    wait_for("the failure line", || {
        f.log().contains("owner: not recorded (")
    });
    wait_for("the idle line", || f.log().contains("idle"));
    assert!(w.is_running());
}

// --- The grammar ------------------------------------------------------------

#[test]
fn a_bad_unit_fails_before_anything_is_written() {
    let f = Fixture::new();
    paused(&f);
    let out = f.run(&["profile", "rotate", "watch", "--supervised=rotator"]);
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert!(
        err.contains("invalid rotate setting --supervised \"rotator\": expected a unit name ending in .service"),
        "{err}"
    );
    assert!(!f.rotate_dir().join("rotate.pid").exists());
    assert!(!marker_path(&f).exists());
}

#[test]
fn a_unit_with_no_name_before_the_suffix_is_rejected() {
    let f = Fixture::new();
    paused(&f);
    let out = f.run(&["profile", "rotate", "watch", "--supervised=.service"]);
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert!(
        err.contains("invalid rotate setting --supervised \".service\": expected a unit name ending in .service"),
        "{err}"
    );
    assert!(!f.rotate_dir().join("rotate.pid").exists());
    assert!(!marker_path(&f).exists());
}

#[test]
fn the_flags_usage_errors_exit_two() {
    let f = Fixture::new();
    for args in [
        &["profile", "rotate", "watch", "--supervised", "--spawned"][..],
        &["profile", "rotate", "start", "--supervised"],
        &["profile", "rotate", "watch", "--supervised", "30"],
    ] {
        assert_usage_error(&f.run(args));
    }
    assert!(!marker_path(&f).exists());
}

// --- `rotate status` ---------------------------------------------------------

#[test]
fn status_without_a_marker_has_no_owner_lines() {
    let f = Fixture::new();
    let s = status(&f);
    assert!(!s.contains("owner:"), "{s}");
    assert!(!s.contains("deferring:"), "{s}");
}

#[test]
fn status_puts_the_owner_lines_after_the_autostart_line() {
    let f = Fixture::new();
    stopped_owner(&f);
    let s = status(&f);
    let want = format!(
        "rotate: stopped\nautostart: on (env)\n\
         owner: amplihack-rotator.service (pid {FAKE_PID}, not running)\n\
         deferring: yes\npid: -\n"
    );
    assert!(s.contains(&want), "{s}");
}

#[test]
fn status_says_when_autostart_is_off() {
    let f = Fixture::new();
    stopped_owner(&f);
    let s = status_with(&f, &[]);
    assert!(s.contains("autostart: off (env)\n"), "{s}");
    assert!(s.contains("deferring: no (autostart off)\n"), "{s}");
}

#[test]
fn status_reports_a_damaged_marker_by_path() {
    let f = Fixture::new();
    state_dirs(&f);
    private_file(&marker_path(&f), "{");
    let s = status(&f);
    assert!(
        s.contains(&format!(
            "owner: unreadable marker {}\ndeferring: yes\n",
            marker_path(&f).display()
        )),
        "{s}"
    );
}

// --- The setting change log ---------------------------------------------------

#[test]
fn autostart_changes_are_logged_and_reads_are_not() {
    let f = Fixture::new();
    f.ok(&["profile", "rotate", "autostart", "off"]);
    f.ok(&["profile", "rotate", "autostart", "on"]);
    f.ok(&["profile", "rotate", "autostart"]);
    let lines = changes(&f);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains(" autostart set=off was=missing pid="),
        "{lines:?}"
    );
    assert!(
        lines[1].contains(" autostart set=on was=off pid="),
        "{lines:?}"
    );
    for l in &lines {
        assert!(l.contains(" ppid="), "{l}");
        assert!(l.contains(" exe="), "{l}");
    }
}

#[test]
fn auto_switch_changes_are_logged_and_a_no_op_is_not() {
    let f = Fixture::new();
    f.ok(&["profile", "auto-switch", "on"]);
    f.ok(&["profile", "auto-switch", "on"]);
    f.ok(&["profile", "auto-switch", "off"]);
    let lines = changes(&f);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains(" auto-switch enabled=on was=missing pid="),
        "{lines:?}"
    );
    assert!(
        lines[1].contains(" auto-switch enabled=off was=on pid="),
        "{lines:?}"
    );
}

#[test]
fn an_unwritable_change_log_warns_and_the_command_succeeds() {
    let f = Fixture::new();
    state_dirs(&f);
    fs::create_dir(f.rotate_dir().join("setting-changes.log")).unwrap();
    let out = f.ok(&["profile", "rotate", "autostart", "off"]);
    let err = text(&out.stderr);
    assert!(
        err.contains("warning: setting change not logged: "),
        "{err}"
    );
    assert_eq!(text(&out.stdout), "autostart: off (setting)\n");
}

// --- The exhausted-login alert ------------------------------------------------

#[test]
fn a_paused_rotator_alerts_every_pass_and_idles_once() {
    let f = Fixture::new();
    paused(&f);
    exhausted_live(&f);
    let _w = watch(&f, &[], &[]);
    wait_for("two ALERTs", || count(&f.log(), ALERT) >= 2);
    let log = f.log();
    assert_eq!(idle_lines(&log), 1, "{log}");
    assert!(!log.contains("work"), "the alert names no account: {log}");
}

#[test]
fn a_paused_rotator_under_the_threshold_stays_quiet() {
    let f = Fixture::new();
    paused(&f);
    live_is(&f, "work");
    cached(&f, "work", "statusline", 50.0, 3600);
    let _w = watch(&f, &[], &[]);
    wait_for("the idle line", || f.log().contains("idle"));
    std::thread::sleep(Duration::from_millis(2500));
    let log = f.log();
    assert_eq!(count(&log, ALERT), 0, "{log}");
    assert_eq!(idle_lines(&log), 1, "{log}");
}

#[test]
fn a_paused_rotator_uses_its_own_thresholds() {
    let f = Fixture::new();
    paused(&f);
    live_is(&f, "work");
    cached(&f, "work", "statusline", 50.0, 3600);
    let _w = watch(&f, &["--session-at", "40"], &[]);
    wait_for("an ALERT", || count(&f.log(), ALERT) >= 1);
}

#[test]
fn a_paused_rotator_with_no_cached_data_stays_quiet() {
    let f = Fixture::new();
    paused(&f);
    let _w = watch(&f, &[], &[]);
    wait_for("the idle line", || f.log().contains("idle"));
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(count(&f.log(), ALERT), 0);
}

#[test]
fn status_alerts_when_auto_switch_was_turned_off_with_no_rotator() {
    let f = Fixture::new();
    paused(&f);
    exhausted_live(&f);
    let s = status(&f);
    assert_eq!(s.lines().filter(|l| *l == STATUS_ALERT).count(), 1, "{s}");
}

#[test]
fn status_alerts_for_the_variable_and_for_a_damaged_setting() {
    let f = Fixture::new();
    exhausted_live(&f);
    let s = status_with(&f, &[("AMPLIHACK_CLAUDE_AUTO_SWITCH", "0")]);
    assert!(s.contains(STATUS_ALERT), "the variable: {s}");
    auto_switch_file(&f, "not a setting\n");
    let s = status(&f);
    assert!(s.contains(STATUS_ALERT), "a damaged file: {s}");
}

#[test]
fn status_counts_an_unexpired_limit_record_and_not_an_expired_one() {
    let f = Fixture::new();
    paused(&f);
    live_is(&f, "work");
    cached(&f, "work", "usage_limit_failure", 100.0, 3600);
    assert!(status(&f).contains(STATUS_ALERT));
    cached(&f, "work", "usage_limit_failure", 100.0, -60);
    assert!(!status(&f).contains(STATUS_ALERT));
}

#[test]
fn no_alert_with_auto_switch_on_or_with_no_setting_at_all() {
    let f = Fixture::new();
    exhausted_live(&f);
    let s = status(&f);
    assert!(
        !s.contains(STATUS_ALERT),
        "no setting: the rotator rotates: {s}"
    );
    auto_switch_file(&f, "enabled=on\n");
    let s = status(&f);
    assert!(!s.contains(STATUS_ALERT), "auto-switch on: {s}");
}

#[test]
fn no_alert_under_the_threshold_or_without_cached_data() {
    let f = Fixture::new();
    paused(&f);
    assert!(!status(&f).contains(STATUS_ALERT), "no data");
    live_is(&f, "work");
    cached(&f, "work", "statusline", 50.0, 3600);
    assert!(!status(&f).contains(STATUS_ALERT), "under the threshold");
}
