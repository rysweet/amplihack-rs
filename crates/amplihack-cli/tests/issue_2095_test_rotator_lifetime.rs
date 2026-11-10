//! Issue #2095: a rotator started by a test ends with that test's process,
//! however the process ends, and a rotator started without the test-only
//! lifetime pipe still outlives its starter as before.
//!
//! Each outer test runs this binary again as a helper (`--exact <helper>
//! --ignored`) with `TMPDIR` inside the outer test's own temporary
//! directory. The helper builds a `Fixture`, starts a rotator, waits for the
//! rotator's watch loop to begin, prints what the outer test needs and
//! blocks on stdin. The outer test SIGKILLs the helper by its own PID, so no
//! destructor in the helper runs, and then observes the rotator through a
//! pidfd it opened while the helper was still alive.
//!
//! Every wait is an event: a pipe line, a child reap, an inotify event or
//! `poll(-1)` on a pidfd. Nothing sleeps and no wait has a deadline. No
//! process is signalled by name or by a pid read from a file: the helper is
//! killed through its `Child`, and the rotator only through its pidfd.

#![cfg(target_os = "linux")]

use crate::isolated_amplihack::{PROFILE_ROOT_ENV, isolated};
use crate::support::profile_rotate::*;
use crate::support::test_path::test_path;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Set to `1` by the outer test in the helper's environment.
const HELPER_ENV: &str = "AMPLIHACK_TEST_ISSUE_2095_HELPER";
/// Written by the outer test in the directory it passes as the helper's
/// `TMPDIR`; the helper refuses to run without it.
const OUTER_MARK: &str = ".issue-2095-outer";
/// The rotator's log line while auto-switch is off.
const IDLE: &str = "idle: auto-switch is off";

// ---------------------------------------------------------------- events

/// Blocks until `cond` holds, re-checking after each inotify event in `dir`.
/// Panics, naming `what`, if the process behind `watcher` exits first, so a
/// rotator that dies cannot leave this waiting for ever.
fn wait_event(dir: &Path, watcher: &OwnedFd, what: &str, mut cond: impl FnMut() -> bool) {
    // SAFETY: inotify_init1 takes flags and returns a new fd or -1.
    let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    assert!(
        raw >= 0,
        "inotify_init1: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: the call just returned this fd, which nothing else owns.
    let ino = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut path = dir.as_os_str().as_bytes().to_vec();
    path.push(0);
    let mask = libc::IN_CREATE | libc::IN_MODIFY | libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO;
    // SAFETY: a valid inotify fd and a NUL-terminated path.
    let wd = unsafe { libc::inotify_add_watch(ino.as_raw_fd(), path.as_ptr().cast(), mask) };
    assert!(
        wd >= 0,
        "inotify_add_watch {}: {}",
        dir.display(),
        std::io::Error::last_os_error()
    );
    // The watch exists before the first check, so no change is missed.
    loop {
        if cond() {
            return;
        }
        let mut fds = [
            libc::pollfd {
                fd: ino.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: watcher.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: two valid pollfds for fds kept open above.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            assert_eq!(e.kind(), std::io::ErrorKind::Interrupted, "poll: {e}");
            continue;
        }
        if fds[1].revents & libc::POLLIN != 0 {
            assert!(cond(), "{what}: the rotator exited first");
            return;
        }
        let mut buf = [0u8; 4096];
        // Drain the queue; the fd is non-blocking.
        // SAFETY: reads into a buffer of the given length.
        while unsafe { libc::read(ino.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    }
}

/// Whether `fd` in process `pid` is close-on-exec, from
/// `/proc/<pid>/fdinfo/<fd>`; `None` when the fd is not open there.
fn cloexec_in(pid: i32, fd: RawFd) -> Option<bool> {
    let info = fs::read_to_string(format!("/proc/{pid}/fdinfo/{fd}")).ok()?;
    let flags = info.lines().find_map(|l| l.strip_prefix("flags:"))?.trim();
    let flags = i64::from_str_radix(flags, 8).ok()?;
    Some(flags & i64::from(libc::O_CLOEXEC) != 0)
}

/// SIGKILLs the rotator through its pidfd when a test fails before it is
/// gone, so a failing test leaks nothing. A no-op once it has exited.
struct KillOnDrop<'a>(&'a OwnedFd);

impl Drop for KillOnDrop<'_> {
    fn drop(&mut self) {
        if !pidfd_exited(self.0, false) {
            let _ = pidfd_signal(self.0, libc::SIGKILL);
            pidfd_exited(self.0, true);
        }
    }
}

// ---------------------------------------------------------------- helper

/// What a helper reports once its rotator's watch loop has begun.
struct Helper {
    child: Child,
    watcher: i32,
    pid_file: PathBuf,
    home: PathBuf,
    root: PathBuf,
    lifetime_fd: Option<RawFd>,
    /// The helper's stdout, kept open so a later write by it never fails.
    _stdout: std::io::Lines<BufReader<std::process::ChildStdout>>,
    /// Dropped last: holds the helper's whole fixture.
    outer: tempfile::TempDir,
}

impl Helper {
    /// SIGKILLs the helper by its own PID and reaps it.
    fn kill_and_reap(&mut self) {
        self.child.kill().expect("SIGKILL the helper");
        let status = self.child.wait().expect("reap the helper");
        assert!(!status.success(), "the helper was killed, not finished");
    }

    fn err_tail(&self) -> String {
        let err = fs::read(self.outer.path().join("helper.err")).unwrap_or_default();
        let start = err.len().saturating_sub(4096);
        String::from_utf8_lossy(&err[start..]).into_owned()
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs the ignored test `name` of this module as a helper and reads its
/// report from stdout.
fn run_helper(name: &str) -> Helper {
    let outer = tempfile::tempdir().expect("the outer test's directory");
    fs::write(outer.path().join(OUTER_MARK), b"").unwrap();
    let err = fs::File::create(outer.path().join("helper.err")).unwrap();
    let exe = std::env::current_exe().expect("the running test binary");
    let mut child = Command::new(&exe)
        .args([
            "--exact",
            &test_path(module_path!(), name),
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HELPER_ENV, "1")
        .env("TMPDIR", outer.path())
        .env_remove(LIFETIME_FD_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(err)
        .spawn()
        .unwrap_or_else(|e| panic!("run {name} in {}: {e}", exe.display()));
    let stdout = child.stdout.take().unwrap();
    let (mut watcher, mut pid_file, mut home, mut root, mut fd) = (None, None, None, None, None);
    let mut lines = BufReader::new(stdout).lines();
    while watcher.is_none()
        || pid_file.is_none()
        || home.is_none()
        || root.is_none()
        || fd.is_none()
    {
        let Some(Ok(line)) = lines.next() else {
            let status = child.wait().expect("reap the helper");
            let tail = fs::read_to_string(outer.path().join("helper.err")).unwrap_or_default();
            panic!("{name} ended ({status}) before reporting its rotator:\n{tail}");
        };
        // libtest prints `test <name> ... ` on the line the first report
        // follows, so a key may start anywhere in the line.
        let Some((key, value)) = ["WATCHER", "PIDFILE", "HOME", "ROOT", "LIFETIME_FD"]
            .iter()
            .find_map(|k| Some((*k, line.split_once(&format!("{k} "))?.1)))
        else {
            continue;
        };
        match key {
            "WATCHER" => watcher = value.parse::<i32>().ok(),
            "PIDFILE" => pid_file = Some(PathBuf::from(value)),
            "HOME" => home = Some(PathBuf::from(value)),
            "ROOT" => root = Some(PathBuf::from(value)),
            "LIFETIME_FD" => fd = Some(value.parse::<RawFd>().ok()),
            _ => {}
        }
    }
    let watcher = watcher.unwrap();
    assert!(watcher > 1, "a real watcher pid");
    let canon = outer.path().canonicalize().unwrap();
    for (what, p) in [
        ("pid file", pid_file.as_ref().unwrap()),
        ("home", home.as_ref().unwrap()),
        ("root", root.as_ref().unwrap()),
    ] {
        assert!(
            p.is_absolute() && (p.starts_with(outer.path()) || p.starts_with(&canon)),
            "the helper's {what} lies inside the outer test's directory"
        );
    }
    Helper {
        child,
        watcher,
        pid_file: pid_file.unwrap(),
        home: home.unwrap(),
        root: root.unwrap(),
        lifetime_fd: fd.unwrap(),
        _stdout: lines,
        outer,
    }
}

/// The helper body: refuses to run outside an outer test, starts a rotator
/// from an armed or unarmed fixture, waits for its watch loop to begin,
/// reports and blocks on stdin until killed.
fn helper(armed: bool) {
    if std::env::var_os(HELPER_ENV).as_deref() != Some("1".as_ref()) {
        return;
    }
    let tmp = std::env::temp_dir();
    assert!(
        tmp.join(OUTER_MARK).is_file(),
        "a helper runs only inside an outer test's directory"
    );
    let f = if armed {
        Fixture::new()
    } else {
        Fixture::unarmed()
    };
    assert!(f.home.starts_with(&tmp) || f.home.starts_with(tmp.canonicalize().unwrap()));
    // Below every threshold, so the rotator never swaps.
    f.usage("work", 10, 10);
    f.ok(&[
        "profile",
        "rotate",
        "start",
        "--interval",
        "1",
        "--hold",
        "0",
    ]);
    let pid_file = f.rotate_dir().join("rotate.pid");
    let pid: i32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(pid > 1);
    let pidfd = pidfd_open(pid).expect("the rotator is running");
    assert!(
        environ_has(pid, &[&home_entry(&f.home)]),
        "the pid file names this fixture's rotator"
    );
    // `watch` writes its gate record after it has armed, naming itself.
    let gate = f.rotate_dir().join("gate.json");
    wait_event(&f.rotate_dir(), &pidfd, "the gate record", || {
        fs::read(&gate)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .is_some_and(|v| v["pid"].as_i64() == Some(i64::from(pid)))
    });
    let mut out = std::io::stdout().lock();
    writeln!(out, "WATCHER {pid}").unwrap();
    writeln!(out, "PIDFILE {}", pid_file.display()).unwrap();
    writeln!(out, "HOME {}", f.home.display()).unwrap();
    writeln!(out, "ROOT {}", f.root.display()).unwrap();
    match f.lifetime_fd() {
        Some(fd) => writeln!(out, "LIFETIME_FD {fd}").unwrap(),
        None => writeln!(out, "LIFETIME_FD -").unwrap(),
    }
    out.flush().unwrap();
    drop(out);
    // Blocks until the outer test kills this process. If the outer test
    // went away instead, stdin closes and the rotator is stopped normally.
    let mut rest = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut rest);
    let (out, refused) = f.stop_until_gone(pid);
    f.assert_stopped(&out, refused, pid);
}

#[test]
#[ignore = "run by a_killed_test_takes_its_rotator_with_it in a child process"]
fn helper_armed() {
    helper(true);
}

#[test]
#[ignore = "run by an_unarmed_rotator_outlives_its_starter in a child process"]
fn helper_unarmed() {
    helper(false);
}

// ---------------------------------------------------------------- tests

/// D7: SIGKILL the test process that started a rotator from an armed
/// fixture, and the rotator exits through its normal stop path.
#[test]
fn a_killed_test_takes_its_rotator_with_it() {
    let mut h = run_helper("helper_armed");
    let fd = h.lifetime_fd.expect("an armed fixture reports its fd");
    let pidfd = pidfd_open(h.watcher).expect("the rotator runs while its test does");
    assert!(
        environ_has(h.watcher, &[&home_entry(&h.home)]),
        "the reported pid is the helper's rotator, so it is safe to signal"
    );
    let _kill = KillOnDrop(&pidfd);
    assert_eq!(
        cloexec_in(h.watcher, fd),
        Some(true),
        "the rotator armed: it holds the read end and made it close-on-exec, \
         so its probes do not hold it. `Some(false)` means it never armed: \
         check that {LIFETIME_FD_ENV} still matches `lifetime::LIFETIME_FD_ENV` \
         and does not start with AMPLIHACK_ROTATE, which `daemon::spawn` \
         strips\nhelper stderr:\n{}",
        h.err_tail()
    );
    assert!(h.pid_file.exists());

    h.kill_and_reap();

    assert!(pidfd_exited(&pidfd, true), "the rotator exits");
    assert!(
        !h.pid_file.exists(),
        "the rotator left through its normal stop path, which removes its pid file"
    );
    let log = fs::read_to_string(h.pid_file.with_file_name("rotate.log")).unwrap_or_default();
    for line in log.lines() {
        assert_log_grammar(line, true);
    }
}

/// D8: without the lifetime pipe nothing ties a rotator to the test that
/// started it. After that test is SIGKILLed and reaped, the rotator still
/// follows a change made afterwards, so its watch loop still runs.
#[test]
fn an_unarmed_rotator_outlives_its_starter() {
    let mut h = run_helper("helper_unarmed");
    assert_eq!(h.lifetime_fd, None, "an unarmed fixture has no lifetime fd");
    let pidfd = pidfd_open(h.watcher).expect("the rotator runs while its test does");
    assert!(
        environ_has(h.watcher, &[&home_entry(&h.home)]),
        "the reported pid is the helper's rotator, so it is safe to signal"
    );
    let _kill = KillOnDrop(&pidfd);
    let log_path = h.pid_file.with_file_name("rotate.log");
    let idles = || {
        fs::read_to_string(&log_path)
            .unwrap_or_default()
            .matches(IDLE)
            .count()
    };
    let before = idles();

    h.kill_and_reap();

    // Made after the reap: only a watch loop that is still running can log
    // that auto-switch is now off.
    let mut off = isolated(amplihack_bin());
    off.args(["profile", "auto-switch", "off"])
        .current_dir(h.outer.path())
        .env("HOME", &h.home)
        .env(PROFILE_ROOT_ENV, &h.root)
        .env("TMPDIR", h.outer.path())
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
        .env("AMPLIHACK_SKIP_REFLECTION", "1");
    // A short command that exits on its own: wait on its exit, with no ceiling.
    let out = off.output().expect("run `profile auto-switch off`");
    assert_no_secrets(&out);
    assert!(
        out.status.success(),
        "`profile auto-switch off` failed: {}\n{}",
        out.status,
        text(&out.stderr)
    );
    let rotate_dir = h.pid_file.parent().unwrap().to_path_buf();
    wait_event(
        &rotate_dir,
        &pidfd,
        "the rotator to go idle after its starter was killed",
        || idles() > before,
    );

    assert!(
        !pidfd_exited(&pidfd, false),
        "the unarmed rotator outlives its starter"
    );
    assert!(h.pid_file.exists(), "and keeps its pid file");

    // Clean-up through the normal stop path, by pidfd.
    pidfd_signal(&pidfd, libc::SIGTERM).expect("SIGTERM the rotator");
    assert!(pidfd_exited(&pidfd, true));
    assert!(!h.pid_file.exists(), "SIGTERM removes the pid file");
}
