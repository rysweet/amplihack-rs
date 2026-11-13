//! Issue #1592: `profile rotate start`, `status` and `stop`, and `watch`
//! following the auto-switch toggle. Every daemon is started against a
//! temporary HOME and stopped (or killed) before the test ends.

#![cfg(target_os = "linux")]

use crate::support::fake_proc::FakeProc;
use crate::support::profile_rotate::*;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
#[path = "support/rotator_events.rs"]
mod rotator_events;
use rotator_events::{ExitEvent, LogEvents};

/// Kills whatever pid the fixture's pid file names when a test fails, so no
/// rotator outlives its temporary HOME. It also owns the test's fake process
/// table (issue #2042), so the tree outlives every command that reads it.
struct Reaper(
    /// The fixture's pid file.
    PathBuf,
    /// The fake process table every command of the test reads; dropped
    /// with the reaper, after the last command.
    FakeProc,
);

/// A fake process table for `f`'s commands, owned by the returned reaper.
fn reaper(f: &Fixture) -> Reaper {
    let r = Reaper(pid_file(f), FakeProc::new());
    f.with_proc(&r.1);
    r
}

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Ok(s) = fs::read_to_string(&self.0)
            && let Ok(pid) = s.trim().parse::<i32>()
            && pid > 1
        {
            let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            if String::from_utf8_lossy(&cmdline).contains("rotate") {
                // SAFETY: plain kill(2) of a process this test started.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

struct Foreign(Child);

/// A rotator started by `start` in this test, killed on drop if still one.
struct KillPid(i32);

impl Drop for KillPid {
    fn drop(&mut self) {
        let cmdline = fs::read(format!("/proc/{}/cmdline", self.0)).unwrap_or_default();
        if String::from_utf8_lossy(&cmdline).contains("rotate") {
            // SAFETY: plain kill(2) of a process this test started.
            unsafe { libc::kill(self.0, libc::SIGKILL) };
        }
    }
}

impl Drop for Foreign {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn pid_file(f: &Fixture) -> PathBuf {
    f.rotate_dir().join("rotate.pid")
}

/// Status without its first line, which is always the root (issue #1592).
fn status(f: &Fixture) -> String {
    let out = text(&f.ok(&["profile", "rotate", "status"]).stdout);
    let (root, rest) = out.split_once('\n').unwrap_or_default();
    assert!(root.starts_with("root: "), "{out}");
    rest.to_string()
}

/// Production start remains asynchronous. The test separately subscribes to
/// the child's readiness event, with pidfd exit readiness as a failure signal.
fn start(f: &Fixture, extra: &[&str]) -> (Reaper, i32) {
    let reaper = reaper(f);
    let events = LogEvents::before_start(&f.rotate_dir());
    let mut args = vec![
        "profile",
        "rotate",
        "start",
        "--interval",
        "1",
        "--hold",
        "0",
    ];
    args.extend_from_slice(extra);
    let out = f.ok(&args);
    assert_no_secrets(&out);
    let stdout = text(&out.stdout);
    assert!(stdout.contains("rotate: started (pid "), "{stdout}");
    assert!(!stdout.contains("checking every"), "{stdout}");
    let pid: i32 = fs::read_to_string(pid_file(f))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    reaper.1.link_own(pid as u32);
    events.until(pid, || f.log(), |log| log.contains("watch: ready;"));
    assert!(
        f.probe_envs().is_empty(),
        "start-up speculatively probed logins"
    );
    (reaper, pid)
}

fn observations(f: &Fixture) -> amplihack_utils::usage_observation::ObservationStore {
    use amplihack_utils::usage_observation::{Evidence, ObservationStore, SessionWindow};
    let store = ObservationStore::open(&f.root).unwrap();
    for (name, session, week) in [
        ("work", 10.0, 40.0),
        ("spare", 10.0, 20.0),
        ("extra", 50.0, 50.0),
    ] {
        let binding = store
            .lock()
            .unwrap()
            .authorised_login(name, &format!("uuid-{name}"))
            .unwrap();
        let mut guard = store.lock().unwrap();
        if name == "work" {
            guard.set_location(&binding, &f.home_login()).unwrap();
        }
        let capture = guard
            .capture_new(&binding, chrono::Utc::now().timestamp())
            .unwrap();
        guard
            .publish(
                &capture,
                &Evidence {
                    session_window: SessionWindow::Present,
                    session_used: Some(session),
                    week_used: Some(week),
                    ..Evidence::default()
                },
            )
            .unwrap();
    }
    store
}

fn stop_rotator(f: &Fixture, pid: i32) -> (std::process::Output, bool) {
    let exit = ExitEvent::open(pid);
    let out = f.run(&["profile", "rotate", "stop"]);
    assert!(out.status.success(), "stop failed: {}", text(&out.stderr));
    exit.wait();
    (out, false)
}

#[test]
fn start_status_and_stop() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    assert!(
        status(&f).starts_with("rotate: stopped\n"),
        "{}",
        status(&f)
    );

    let (_reaper, pid) = start(&f, &[]);
    assert!(!f.log().contains(" cycle "), "baseline caused a decision");
    let s = status(&f);
    assert!(s.contains("rotate: running\n"), "{s}");
    assert!(s.contains(&format!("pid: {pid}\n")), "{s}");
    assert!(s.contains("live: work\n") || s.contains("live: -\n"), "{s}");
    assert!(s.contains("last cycle: "), "{s}");
    assert!(s.contains("log: "), "{s}");
    assert_text_has_no_secrets(&s);

    let again = f.run(&["profile", "rotate", "start", "--interval", "1"]);
    assert_eq!(again.status.code(), Some(1), "a second rotator is refused");
    assert!(text(&again.stderr).contains("amplihack profile rotate status"));

    let (out, refused) = stop_rotator(&f, pid);
    f.assert_stopped(&out, refused, pid);
    assert!(status(&f).starts_with("rotate: stopped\n"));

    for l in f.log().lines() {
        assert_log_grammar(l, true);
    }
    f.assert_each_login_once();
}

#[test]
fn the_daemon_swaps_and_logs_like_the_one_shot() {
    let f = Fixture::new();
    // Give the identity endpoint and transaction guards a resolved common
    // organisation. These are synthetic fixture identities, never credentials
    // from the host.
    for name in ["work", "spare", "extra"] {
        let mut account = account_json(name);
        account["organizationUuid"] = serde_json::json!("org-test");
        let dir = if name == "work" {
            f.home.clone()
        } else {
            f.root.join(name)
        };
        private_file(
            &dir.join(".claude.json"),
            &serde_json::json!({"oauthAccount": account}).to_string(),
        );
        private_file(
            &f.root.join(name).join(".claude.json"),
            &serde_json::json!({"oauthAccount": account}).to_string(),
        );
        private_file(
            &f.root
                .join(".auto-switch.d/expected")
                .join(format!("{name}.json")),
            &serde_json::json!({"account":format!("uuid-{name}"),"org":"org-test"}).to_string(),
        );
        login(&if name == "work" {
            f.home_login()
        } else {
            f.slot(name)
        });
    }
    let store = observations(&f);
    let (_reaper, pid) = start(&f, &[]);
    let events = LogEvents::before_start(&f.rotate_dir());
    let mut guard = store.lock().unwrap();
    let binding = guard.binding("work").unwrap().unwrap();
    let capture = guard
        .capture_new(&binding, chrono::Utc::now().timestamp())
        .unwrap();
    guard
        .publish(
            &capture,
            &amplihack_utils::usage_observation::Evidence {
                session_window: amplihack_utils::usage_observation::SessionWindow::Present,
                session_used: Some(90.0),
                week_used: Some(40.0),
                ..Default::default()
            },
        )
        .unwrap();
    drop(guard);
    events.until(
        pid,
        || f.log(),
        |log| log.contains("watch: decision 1 finished:"),
    );
    assert!(
        f.log()
            .lines()
            .any(|l| l.contains(" swap ") && l.contains("->spare live ")),
        "threshold decision failed: {}",
        f.log()
    );
    assert_eq!(f.log().matches("watch: decision 1 finished:").count(), 1);
    let (out, refused) = stop_rotator(&f, pid);
    f.assert_stopped(&out, refused, pid);
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
    assert_eq!(ino(&f.slot("work")), Some(f.live_ino));
    f.assert_each_login_once();
    for e in f.probe_envs() {
        assert!(!e.contains("AMPLIHACK_ROTATE"), "probe environment: {e}");
    }
}

#[test]
fn a_stale_pid_file_is_reported_removed_and_never_signalled() {
    let f = Fixture::new();
    fs::create_dir_all(f.rotate_dir()).unwrap();
    // A pid that is surely not running.
    fs::write(pid_file(&f), "4194300\n").unwrap();
    assert!(status(&f).starts_with("rotate: stale\n"));
    assert!(pid_file(&f).exists(), "status is read-only");
    let out = f.ok(&["profile", "rotate", "stop"]);
    assert!(text(&out.stdout).contains("rotate: removed a stale pid file"));
    assert!(!pid_file(&f).exists());

    let out = f.ok(&["profile", "rotate", "stop"]);
    assert!(text(&out.stdout).contains("rotate: not running"));
}

#[test]
fn a_foreign_process_named_in_the_pid_file_is_left_alone() {
    let f = Fixture::new();
    let mut sleeper = Foreign(
        Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let pid = sleeper.0.id() as i32;
    // Visible through the fake tree, so `status` rejects it by its cmdline.
    let guard = reaper(&f);
    guard.1.link_own(sleeper.0.id());
    fs::create_dir_all(f.rotate_dir()).unwrap();
    fs::write(pid_file(&f), format!("{pid}\n")).unwrap();
    assert!(status(&f).starts_with("rotate: stale\n"));
    f.ok(&["profile", "rotate", "stop"]);
    sleeper
        .0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"still alive\n")
        .unwrap();
    let mut reply = [0u8; 12];
    sleeper
        .0
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut reply)
        .unwrap();
    assert_eq!(&reply, b"still alive\n");
}

#[test]
fn a_rotator_that_ignores_sigterm_keeps_its_pid_file() {
    let f = Fixture::new();
    // A process that passes verification (argv[0] named `amplihack`, with
    // `profile rotate watch`) and ignores SIGTERM.
    let bash = ["/bin/bash", "/usr/bin/bash"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());
    let Some(bash) = bash else {
        return;
    };
    let mut stubborn = Foreign(
        Command::new(bash)
            .arg0("amplihack")
            .args([
                "-c",
                "trap '' TERM; printf 'ready\\n'; read -r _",
                "profile",
                "rotate",
                "watch",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let pid = stubborn.0.id() as i32;
    let guard = reaper(&f);
    guard.1.link_own(stubborn.0.id());
    let mut ready = String::new();
    BufReader::new(stubborn.0.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready\n");
    fs::create_dir_all(f.rotate_dir()).unwrap();
    fs::write(pid_file(&f), format!("{pid}\n")).unwrap();
    assert!(
        status(&f).starts_with("rotate: running\n"),
        "{}",
        status(&f)
    );

    let out = f.run(&["profile", "rotate", "stop"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "RotateDaemon after the 10 s wait"
    );
    assert!(text(&out.stderr).contains("amplihack profile rotate status"));
    assert!(pid_file(&f).exists(), "the pid file is kept");
    assert!(alive(pid));
}

#[test]
fn watch_follows_the_auto_switch_toggle() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    f.ok(&["profile", "auto-switch", "off"]);
    let guard = reaper(&f);
    let out = f.ok(&["profile", "rotate", "start", "--interval", "1"]);
    let started: u32 = fs::read_to_string(pid_file(&f))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    guard.1.link_own(started);
    assert!(
        text(&out.stderr).contains("auto-switch is off"),
        "start warns: {}",
        text(&out.stderr)
    );
    let events = LogEvents::before_start(&f.rotate_dir());
    events.until(
        started as i32,
        || f.log(),
        |log| log.contains("watch: ready;"),
    );
    assert_eq!(f.log().matches("idle: auto-switch is off").count(), 1);
    assert!(!f.log().contains("cycle "), "no probes while off");
    // Toggle events with complete evidence permit a decision without probes.
    let _observations = observations(&f);
    f.ok(&["profile", "auto-switch", "on"]);
    events.until(
        started as i32,
        || f.log(),
        |log| log.contains("watch: decision "),
    );
    assert!(
        f.probe_envs().is_empty(),
        "complete observations needed no probes"
    );
    f.ok(&["profile", "auto-switch", "off"]);
    let pid: i32 = fs::read_to_string(pid_file(&f))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let (out, refused) = stop_rotator(&f, pid);
    f.assert_stopped(&out, refused, pid);
}

#[test]
fn stop_and_status_without_a_state_directory() {
    let f = Fixture::new();
    assert!(status(&f).contains("pid: -\n"));
    let out = f.ok(&["profile", "rotate", "stop"]);
    assert!(text(&out.stdout).contains("rotate: not running"));
    assert!(!f.rotate_dir().join("rotate.pid").exists());
}

/// Two `start`s at once beside a running rotator both refuse, and the pid
/// file still names that rotator. The race between two starts with no
/// rotator yet is not checked here: the commands read a fake process table
/// (issue #2042), and the second `start` would check it before the test can
/// link the first daemon's pid. `start` and autostart take the same state
/// lock and call the same `launch_core`, and
/// `tests_autostart::sixteen_concurrent_launches_start_exactly_one_rotator`
/// covers that race with a spawner that records its process under the lock.
#[test]
fn starts_beside_a_running_rotator_refuse() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    let (_reaper, pid) = start(&f, &[]);
    let args = [
        "profile",
        "rotate",
        "start",
        "--interval",
        "1",
        "--hold",
        "0",
    ];
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| f.run(&args));
        let b = s.spawn(|| f.run(&args));
        (a.join().unwrap(), b.join().unwrap())
    });
    // Any rotator a `start` reports despite the refusal is killed when the
    // test ends, so a failure leaves no orphan behind.
    let started: Vec<KillPid> = [&a, &b]
        .iter()
        .filter_map(|o| {
            let out = text(&o.stdout);
            let pid = out
                .split("rotate: started (pid ")
                .nth(1)?
                .split(')')
                .next()?;
            Some(KillPid(pid.parse().ok()?))
        })
        .collect();
    assert!(
        started.is_empty(),
        "{}\n{}",
        text(&a.stderr),
        text(&b.stderr)
    );
    // Each refuses: "already running" once it holds the lock, or "lock
    // busy" while the other holds it.
    for out in [&a, &b] {
        assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    }
    assert_eq!(
        fs::read_to_string(pid_file(&f)).unwrap().trim(),
        pid.to_string(),
        "the running rotator keeps its pid file"
    );
    let (out, refused) = stop_rotator(&f, pid);
    f.assert_stopped(&out, refused, pid);
}

/// `rotate autostart [on|off]` and the `autostart:` status line. No rotator
/// is started: the fixture sets `AMPLIHACK_ROTATE_AUTOSTART=0`, and these
/// commands never launch Claude.
#[test]
fn rotate_autostart_setting_and_status_line() {
    let f = Fixture::new();
    let bare = |args: &[&str]| {
        let mut c = f.cmd(args);
        c.env_remove("AMPLIHACK_ROTATE_AUTOSTART");
        c.output().unwrap()
    };
    let setting = f.rotate_dir().join("autostart");

    let out = bare(&["profile", "rotate", "autostart"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "autostart: on (default)\n");
    assert!(!setting.exists(), "reading the state writes nothing");

    let out = bare(&["profile", "rotate", "autostart", "off"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "autostart: off (setting)\n");
    assert_eq!(fs::read_to_string(&setting).unwrap(), "off\n");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&setting).unwrap().permissions().mode() & 0o777,
        0o600
    );

    // `status` puts the autostart line right after `rotate:`.
    let lines: Vec<String> = text(&bare(&["profile", "rotate", "status"]).stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let i = lines
        .iter()
        .position(|l| l.starts_with("rotate: "))
        .unwrap();
    assert_eq!(
        lines.get(i + 1).map(String::as_str),
        Some("autostart: off (setting)"),
        "{lines:?}"
    );

    // The variable wins over the file, and says so when the file is written.
    assert!(
        status(&f).contains("\nautostart: off (env)\n"),
        "{}",
        status(&f)
    );
    let out = f.run(&["profile", "rotate", "autostart", "on"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "autostart: on (setting)\n");
    assert!(
        text(&out.stderr).contains("AMPLIHACK_ROTATE_AUTOSTART=0 overrides it"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&setting).unwrap(), "on\n");

    let out = bare(&["profile", "rotate", "autostart", "maybe"]);
    assert_eq!(out.status.code(), Some(2), "a usage error");
    assert_eq!(fs::read_to_string(&setting).unwrap(), "on\n");
    assert!(!pid_file(&f).exists(), "no rotator was started");
}
