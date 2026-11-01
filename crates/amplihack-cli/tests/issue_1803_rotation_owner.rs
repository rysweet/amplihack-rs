//! Issue #1803: the rotation owner marker, the change log and the
//! exhausted-while-off alert, through the built binary.
//!
//! Every command runs through `support::profile_rotate::Fixture`, which
//! clears the environment, uses a temporary HOME and profile root, and sets
//! the test guard, so nothing touches the real `~/.claude`, profile root or
//! tmux server. `INVOCATION_ID` and `AMPLIHACK_RECIPE_RUN_ID` reach a child
//! only when a test sets them. Every `watch` started here runs with
//! auto-switch off and `--interval 3600`, so it probes nothing, and is killed
//! by a drop guard if the test fails.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

const CLEAR_HINT: &str = "run 'amplihack profile rotate owner clear' to release";

fn owner_file(f: &Fixture) -> PathBuf {
    f.rotate_dir().join("owner")
}

fn change_lines(f: &Fixture) -> Vec<String> {
    f.log()
        .lines()
        .filter(|l| l.contains(" change: "))
        .map(String::from)
        .collect()
}

fn wait_for(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A `rotate watch` child, killed and reaped on drop.
struct Watch(Option<Child>);

impl Watch {
    fn pid(&self) -> u32 {
        self.0.as_ref().unwrap().id()
    }

    /// SIGTERM, then the exit status and output, waiting at most 15 s.
    fn stop(mut self) -> Output {
        let mut child = self.0.take().unwrap();
        // SAFETY: plain kill(2) of a child this test started.
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(15) {
                let _ = child.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        child.wait_with_output().unwrap()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn spawn_watch(f: &Fixture, extra: &[&str], env: &[(&str, &str)]) -> Watch {
    let mut args = vec!["profile", "rotate", "watch", "--interval", "3600"];
    args.extend_from_slice(extra);
    let mut cmd = f.cmd(&args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    Watch(Some(child))
}

/// Auto-switch off, so a `watch` idles.
fn paused_fixture() -> Fixture {
    let f = Fixture::new();
    f.ok(&["profile", "auto-switch", "off"]);
    f
}

fn write_marker(f: &Fixture, body: &str) {
    fs::create_dir_all(f.rotate_dir()).unwrap();
    for d in [f.root.join(".auto-switch.d"), f.rotate_dir()] {
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    private_file(&owner_file(f), body);
}

/// The `rotate status` lines.
fn status_lines(f: &Fixture) -> Vec<String> {
    text(&f.ok(&["profile", "rotate", "status"]).stdout)
        .lines()
        .map(String::from)
        .collect()
}

// --- The change log --------------------------------------------------------------

#[test]
fn autostart_off_on_on_logs_two_changes() {
    let f = Fixture::new();
    f.ok(&["profile", "rotate", "autostart", "off"]);
    f.ok(&["profile", "rotate", "autostart", "on"]);
    f.ok(&["profile", "rotate", "autostart", "on"]);
    let lines = change_lines(&f);
    assert_eq!(lines.len(), 2, "{lines:#?}");
    let re = regex::Regex::new(
        r#"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z change: rotate-autostart (\S+) -> (\S+) by pid=\d+ ppid=\d+ parent="[^"]*"$"#,
    )
    .unwrap();
    let caps: Vec<(String, String)> = lines
        .iter()
        .map(|l| {
            let c = re.captures(l).unwrap_or_else(|| panic!("format: {l}"));
            (c[1].to_string(), c[2].to_string())
        })
        .collect();
    assert_eq!(
        caps,
        vec![
            ("unset".to_string(), "off".to_string()),
            ("off".to_string(), "on".to_string())
        ]
    );
    for l in &lines {
        assert_log_grammar(l, true);
    }
}

#[test]
fn a_change_from_a_recipe_run_names_it() {
    let f = Fixture::new();
    let out = f
        .cmd(&["profile", "rotate", "autostart", "off"])
        .env("AMPLIHACK_RECIPE_RUN_ID", "x")
        .env("INVOCATION_ID", "inv-1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let lines = change_lines(&f);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].ends_with("\" recipe_run=x invocation=inv-1"),
        "{}",
        lines[0]
    );
}

#[test]
fn the_change_log_is_private() {
    let f = Fixture::new();
    f.ok(&["profile", "rotate", "autostart", "off"]);
    let m = fs::metadata(f.rotate_dir().join("rotate.log"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(m, 0o600);
}

#[test]
fn auto_switch_off_on_logs_two_changes_and_threshold_none() {
    let f = Fixture::new();
    f.ok(&["profile", "auto-switch", "off"]);
    f.ok(&["profile", "auto-switch", "on"]);
    f.ok(&["profile", "auto-switch", "on"]);
    f.ok(&["profile", "auto-switch", "threshold", "80"]);
    let lines = change_lines(&f);
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert!(
        lines[0].contains(" change: auto-switch unset -> off by pid="),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].contains(" change: auto-switch off -> on by pid="),
        "{}",
        lines[1]
    );
}

#[test]
fn a_corrupt_auto_switch_file_is_logged_as_corrupt() {
    let f = Fixture::new();
    private_file(&f.root.join(".auto-switch"), "garbage\n");
    f.ok(&["profile", "auto-switch", "on"]);
    let lines = change_lines(&f);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(
        lines[0].contains(" change: auto-switch corrupt -> on by "),
        "{}",
        lines[0]
    );
}

// --- Status --------------------------------------------------------------------------

#[test]
fn status_shows_no_owner_straight_after_autostart() {
    let f = Fixture::new();
    let lines = status_lines(&f);
    let i = lines
        .iter()
        .position(|l| l.starts_with("autostart: "))
        .unwrap_or_else(|| panic!("{lines:#?}"));
    assert_eq!(lines[i + 1], "owner: none", "{lines:#?}");
    assert!(lines[i + 2].starts_with("pid: "), "{lines:#?}");
}

#[test]
fn status_shows_a_stopped_owner() {
    let f = Fixture::new();
    write_marker(&f, "unit=test.service\npid=4300200\nsince=1\n");
    let lines = status_lines(&f);
    assert!(
        lines.contains(&format!(
            "owner: test.service pid 4300200 (not running); autostart defers to it; {CLEAR_HINT}"
        )),
        "{lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("autostart: ") && l.ends_with(" (deferring to test.service)")),
        "{lines:#?}"
    );
}

#[test]
fn status_shows_the_exhausted_alert() {
    let f = Fixture::new();
    exhausted_while_off(&f);
    let lines = status_lines(&f);
    assert!(
        lines.contains(
            &"alert: auto-switch is off while the live login is exhausted (work: 100% used, threshold 90%)"
                .to_string()
        ),
        "{lines:#?}"
    );
}

#[test]
fn a_paused_watch_logs_the_exhausted_alert() {
    let f = Fixture::new();
    exhausted_while_off(&f);
    let w = spawn_watch(&f, &[], &[]);
    let want = "ALERT: auto-switch is off while the live login is exhausted (work: 100% used, threshold 90%)";
    wait_for("the ALERT line", Duration::from_secs(20), || {
        f.log().contains(want)
    });
    w.stop();
    let log = f.log();
    assert!(log.contains("idle: auto-switch is off"), "{log}");
}

/// Threshold 90, auto-switch off, and the live login `work` at 100%.
fn exhausted_while_off(f: &Fixture) {
    f.ok(&["profile", "auto-switch", "threshold", "90"]);
    f.ok(&["profile", "auto-switch", "off"]);
    let now = chrono::Utc::now().timestamp();
    fs::create_dir_all(f.rotate_dir()).unwrap();
    fs::set_permissions(f.rotate_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    private_file(
        &f.rotate_dir().join("ledger.json"),
        r#"{"version":1,"live":"work"}"#,
    );
    private_file(
        &f.root.join(".auto-switch.d/usage.json"),
        &format!(
            r#"{{"version":1,"accounts":{{"work":{{"five_hour":{{"used_percentage":100.0,"resets_at":{}}},"observed_at":{now},"source":"statusline"}}}}}}"#,
            now + 3600
        ),
    );
}

#[test]
fn status_has_no_exhausted_alert_while_auto_switch_is_on() {
    let f = Fixture::new();
    f.ok(&["profile", "auto-switch", "on"]);
    let lines = status_lines(&f);
    assert!(
        !lines.iter().any(|l| l.contains("live login is exhausted")),
        "{lines:#?}"
    );
}

// --- rotate owner -----------------------------------------------------------------------

#[test]
fn owner_with_no_marker_prints_none() {
    let f = Fixture::new();
    let existed = f.rotate_dir().exists();
    let out = f.ok(&["profile", "rotate", "owner"]);
    assert_eq!(text(&out.stdout), "owner: none\n");
    assert_eq!(
        f.rotate_dir().exists(),
        existed,
        "read-only: creates nothing"
    );
}

#[test]
fn owner_prints_the_marker_line_and_changes_nothing() {
    let f = Fixture::new();
    let body = "unit=test.service\npid=4300200\nsince=1\n";
    write_marker(&f, body);
    let out = f.ok(&["profile", "rotate", "owner"]);
    assert_eq!(
        text(&out.stdout),
        format!(
            "owner: test.service pid 4300200 (not running); autostart defers to it; {CLEAR_HINT}\n"
        )
    );
    assert_eq!(fs::read_to_string(owner_file(&f)).unwrap(), body);
}

#[test]
fn owner_clear_with_no_marker_prints_none() {
    let f = Fixture::new();
    let out = f.ok(&["profile", "rotate", "owner", "clear"]);
    assert_eq!(text(&out.stdout), "owner: none\n");
}

#[test]
fn owner_clear_removes_the_marker() {
    let f = Fixture::new();
    write_marker(&f, "unit=test.service\npid=4300200\nsince=1\n");
    let out = f.ok(&["profile", "rotate", "owner", "clear"]);
    assert_eq!(text(&out.stdout), "owner: cleared (was test.service)\n");
    assert!(!owner_file(&f).exists());
    let lines = status_lines(&f);
    assert!(lines.contains(&"owner: none".to_string()), "{lines:#?}");
}

#[test]
fn owner_clear_removes_an_unreadable_marker() {
    let f = Fixture::new();
    write_marker(&f, "garbage\n");
    let out = f.ok(&["profile", "rotate", "owner", "clear"]);
    assert_eq!(text(&out.stdout), "owner: cleared (was unreadable)\n");
    assert!(!owner_file(&f).exists());
}

// --- watch --supervised -----------------------------------------------------------------

#[test]
fn a_supervised_watch_writes_the_marker_and_leaves_it_on_exit() {
    let f = paused_fixture();
    let w = spawn_watch(&f, &["--supervised=test.service"], &[]);
    let pid = w.pid();
    let want = format!("unit=test.service\npid={pid}\nsince=");
    wait_for("the owner marker", Duration::from_secs(20), || {
        fs::read_to_string(owner_file(&f)).is_ok_and(|b| b.starts_with(&want))
    });
    let body = fs::read_to_string(owner_file(&f)).unwrap();
    let since = body.strip_prefix(&want).unwrap();
    assert!(
        since.ends_with('\n') && since.trim_end().parse::<i64>().is_ok(),
        "{body:?}"
    );
    let m = fs::metadata(owner_file(&f)).unwrap().permissions().mode() & 0o777;
    assert_eq!(m, 0o600);
    let status = text(&f.ok(&["profile", "rotate", "owner"]).stdout);
    assert_eq!(
        status,
        format!("owner: test.service pid {pid} (running); autostart defers to it\n")
    );
    w.stop();
    assert!(owner_file(&f).exists(), "a watch never removes the marker");
}

/// A synthetic `/proc` holding only `self/cgroup`, passed to the child as
/// `AMPLIHACK_TEST_PROC_ROOT` so the result does not depend on the cgroup
/// the test itself runs in.
fn fake_cgroup(f: &Fixture, cgroup: &str) -> String {
    let proc_root = f.tmp.path().join("fake-proc");
    fs::create_dir_all(proc_root.join("self")).unwrap();
    fs::set_permissions(&proc_root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(proc_root.join("self/cgroup"), cgroup).unwrap();
    proc_root.to_str().unwrap().to_string()
}

#[test]
fn invocation_id_in_a_service_makes_a_watch_supervised() {
    let f = paused_fixture();
    let proc_root = fake_cgroup(
        &f,
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/amplihack-rotator.service\n",
    );
    let w = spawn_watch(
        &f,
        &[],
        &[
            ("INVOCATION_ID", "abc123"),
            ("AMPLIHACK_TEST_PROC_ROOT", &proc_root),
        ],
    );
    let want = format!("pid={}\n", w.pid());
    wait_for("the owner marker", Duration::from_secs(20), || {
        fs::read_to_string(owner_file(&f)).is_ok_and(|b| b.contains(&want))
    });
    let body = fs::read_to_string(owner_file(&f)).unwrap();
    assert!(
        body.starts_with("unit=amplihack-rotator.service\n"),
        "{body:?}"
    );
}

#[test]
fn an_inherited_invocation_id_in_a_scope_writes_no_marker() {
    let f = paused_fixture();
    let proc_root = fake_cgroup(
        &f,
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-8b86.scope\n",
    );
    let w = spawn_watch(
        &f,
        &[],
        &[
            ("INVOCATION_ID", "abc123"),
            ("AMPLIHACK_TEST_PROC_ROOT", &proc_root),
        ],
    );
    wait_for("the idle line", Duration::from_secs(20), || {
        f.log().contains("idle: auto-switch is off")
    });
    assert!(
        !owner_file(&f).exists(),
        "a shell under the user manager is not a supervised service"
    );
    w.stop();
}

#[test]
fn a_spawned_watch_never_writes_the_marker() {
    let f = paused_fixture();
    let w = spawn_watch(&f, &["--spawned"], &[("INVOCATION_ID", "x")]);
    wait_for("the idle line", Duration::from_secs(20), || {
        f.log().contains("idle: auto-switch is off")
    });
    assert!(!owner_file(&f).exists(), "--spawned never claims ownership");
    w.stop();
}

#[test]
fn an_unsupervised_watch_leaves_an_existing_marker_and_notes_it() {
    let f = paused_fixture();
    let body = "unit=test.service\npid=4300200\nsince=1\n";
    write_marker(&f, body);
    let w = spawn_watch(&f, &[], &[]);
    wait_for("the idle line", Duration::from_secs(20), || {
        f.log().contains("idle: auto-switch is off")
    });
    let out = w.stop();
    assert_eq!(fs::read_to_string(owner_file(&f)).unwrap(), body);
    let err = text(&out.stderr);
    assert_eq!(
        err.matches("rotate: rotation is owned by test.service; this watch runs anyway and leaves the owner marker in place")
            .count(),
        1,
        "{err}"
    );
}

#[test]
fn a_supervised_watch_refuses_beside_a_running_rotator() {
    let f = paused_fixture();
    let first = spawn_watch(&f, &[], &[]);
    let pid_file = f.rotate_dir().join("rotate.pid");
    let want = format!("{}\n", first.pid());
    wait_for(
        "the first rotator's pid file",
        Duration::from_secs(20),
        || fs::read_to_string(&pid_file).is_ok_and(|p| p == want),
    );
    let out = f.run(&[
        "profile",
        "rotate",
        "watch",
        "--interval",
        "3600",
        "--supervised=test.service",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("already running"),
        "{}",
        text(&out.stderr)
    );
    assert!(!owner_file(&f).exists(), "no marker from a refused watch");
    first.stop();
}

#[test]
fn start_refuses_the_supervised_flag() {
    let f = Fixture::new();
    let out = f.run(&["profile", "rotate", "start", "--supervised=test.service"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("--supervised applies to rotate watch only"),
        "{}",
        text(&out.stderr)
    );
    assert!(
        !f.rotate_dir().join("rotate.pid").exists(),
        "nothing started"
    );
    assert!(!owner_file(&f).exists());
}

#[test]
fn the_unit_needs_an_equals_sign() {
    let f = Fixture::new();
    let out = f.run(&[
        "profile",
        "rotate",
        "watch",
        "--interval",
        "3600",
        "--supervised",
        "test.service",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "a bare word after --supervised is a usage error: {}",
        text(&out.stderr)
    );
    assert!(!owner_file(&f).exists());
}
