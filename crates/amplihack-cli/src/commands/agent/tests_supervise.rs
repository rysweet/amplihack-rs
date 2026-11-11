//! Unit tests for `supervise.rs`: the loop with an injected clock and
//! sleeper (R5.9: the next pass starts `--interval` after the previous one
//! ends; `--once`; the stop flag; A16), the stoppable sleep, and the
//! supervisor lock (R5.1, E5, S18).
//!
//! API under test:
//! - `supervise::run_loop(once: bool, interval: Duration, stop: &AtomicBool,
//!   sleep: &dyn Fn(Duration, &AtomicBool) -> bool,
//!   pass: impl FnMut() -> anyhow::Result<()>) -> anyhow::Result<()>`
//! - `supervise::sleep_unless_stopped(d: Duration, stop: &AtomicBool) -> bool`:
//!   `false` when stopped, checked in 200 ms slices
//! - `supervise::lock_pid(raw: &[u8]) -> Option<u32>`: a decimal u32 and an
//!   optional newline, at most 64 bytes
//! - `supervise::held_message(pid: Option<u32>) -> String`
//! - `supervise::take_lock(dir: &AgentsDir) -> anyhow::Result<ExclusiveLock>`
//! - `store::AgentsDir::open(home: &Path) -> anyhow::Result<AgentsDir>`,
//!   `AgentsDir::supervisor_lock(&self) -> PathBuf`

use super::store::AgentsDir;
use super::supervise::*;
use std::cell::{Cell, RefCell};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_secs(60);

#[test]
fn the_interval_runs_from_the_end_of_a_pass() {
    let clock = Cell::new(0u64);
    let starts = RefCell::new(Vec::new());
    let slept = RefCell::new(Vec::new());
    let stop = AtomicBool::new(false);
    let sleep = |d: Duration, _: &AtomicBool| {
        slept.borrow_mut().push(d);
        clock.set(clock.get() + d.as_secs());
        true
    };
    let pass = || {
        starts.borrow_mut().push(clock.get());
        clock.set(clock.get() + 40);
        if starts.borrow().len() == 3 {
            stop.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    run_loop(false, INTERVAL, &stop, &sleep, pass).unwrap();
    assert_eq!(*starts.borrow(), [0, 100, 200]);
    assert_eq!(
        *slept.borrow(),
        [INTERVAL, INTERVAL],
        "no sleep once stopped"
    );
}

#[test]
fn once_makes_one_pass_and_never_sleeps() {
    let stop = AtomicBool::new(false);
    let passes = Cell::new(0);
    let sleep = |_: Duration, _: &AtomicBool| -> bool { panic!("--once must not sleep") };
    let pass = || {
        passes.set(passes.get() + 1);
        Ok(())
    };
    run_loop(true, INTERVAL, &stop, &sleep, pass).unwrap();
    assert_eq!(passes.get(), 1);
}

#[test]
fn once_returns_a_pass_error() {
    let stop = AtomicBool::new(false);
    let sleep = |_: Duration, _: &AtomicBool| true;
    let err = run_loop(true, INTERVAL, &stop, &sleep, || {
        anyhow::bail!("registry unreadable")
    })
    .unwrap_err();
    assert!(err.to_string().contains("registry unreadable"), "{err}");
}

#[test]
fn the_loop_survives_a_failed_pass_a16() {
    let stop = AtomicBool::new(false);
    let passes = Cell::new(0);
    let sleep = |_: Duration, _: &AtomicBool| true;
    let pass = || {
        passes.set(passes.get() + 1);
        if passes.get() == 3 {
            stop.store(true, Ordering::SeqCst);
            return Ok(());
        }
        anyhow::bail!("transient")
    };
    run_loop(false, INTERVAL, &stop, &sleep, pass).expect("errors do not end the loop");
    assert_eq!(passes.get(), 3);
}

#[test]
fn a_stop_during_the_wait_ends_the_loop_with_ok() {
    let stop = AtomicBool::new(false);
    let passes = Cell::new(0);
    let sleep = |_: Duration, s: &AtomicBool| {
        s.store(true, Ordering::SeqCst);
        false
    };
    let pass = || {
        passes.set(passes.get() + 1);
        Ok(())
    };
    run_loop(false, INTERVAL, &stop, &sleep, pass).unwrap();
    assert_eq!(passes.get(), 1);
}

#[test]
fn the_sleep_returns_early_when_stopped() {
    let stop = AtomicBool::new(true);
    let t = Instant::now();
    assert!(!sleep_unless_stopped(Duration::from_secs(10), &stop));
    assert!(t.elapsed() < Duration::from_millis(500));

    let stop = AtomicBool::new(false);
    let t = Instant::now();
    assert!(sleep_unless_stopped(Duration::from_millis(50), &stop));
    assert!(t.elapsed() >= Duration::from_millis(50));

    let stop = Arc::new(AtomicBool::new(false));
    let s2 = Arc::clone(&stop);
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        s2.store(true, Ordering::SeqCst);
    });
    let t = Instant::now();
    assert!(!sleep_unless_stopped(Duration::from_secs(10), &stop));
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    setter.join().unwrap();
}

#[test]
fn lock_contents() {
    assert_eq!(lock_pid(b"4242\n"), Some(4242));
    assert_eq!(lock_pid(b"4242"), Some(4242));
    let long = "1".repeat(65);
    for bad in [
        &b""[..],
        b"\n",
        b"abc\n",
        b"-1\n",
        b"4242\n5\n",
        b" 4242\n",
        long.as_bytes(),
    ] {
        assert_eq!(lock_pid(bad), None, "{:?}", String::from_utf8_lossy(bad));
    }
    assert_eq!(
        held_message(Some(4242)),
        "another amplihack agent supervisor is running (pid 4242)"
    );
    assert_eq!(
        held_message(None),
        "another amplihack agent supervisor is running (pid unknown)"
    );
}

fn home() -> (tempfile::TempDir, AgentsDir) {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let dir = AgentsDir::open(tmp.path()).expect("a private temp home");
    (tmp, dir)
}

#[test]
fn the_lock_holds_our_pid_and_refuses_a_second_supervisor() {
    let (_t, dir) = home();
    let path = dir.supervisor_lock();
    fs::write(&path, "12345678901234567890 stale content\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let held = take_lock(&dir).expect("the first supervisor takes the lock");
    let pid = std::process::id();
    assert_eq!(fs::read_to_string(&path).unwrap(), format!("{pid}\n"));
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let err = take_lock(&dir).expect_err("a second supervisor is refused");
    assert_eq!(err.to_string(), held_message(Some(pid)));
    drop(held);
    take_lock(&dir).expect("free again once the first one exits");
}

#[test]
fn the_lock_is_not_inherited_by_children_s18() {
    let (_t, dir) = home();
    let held = take_lock(&dir).unwrap();
    // SAFETY: F_GETFD on a descriptor this lock owns.
    let flags = unsafe { libc::fcntl(held.file().as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0, "flags {flags}");
}

#[test]
fn a_symlinked_lock_file_is_refused() {
    let (tmp, dir) = home();
    let target = tmp.path().join("elsewhere");
    fs::write(&target, "").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&target, dir.supervisor_lock()).unwrap();
    let err = take_lock(&dir).expect_err("a symlink is never followed");
    assert!(err.to_string().contains("is a symlink"), "{err}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "");
}
