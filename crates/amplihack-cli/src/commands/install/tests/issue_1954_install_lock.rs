//! Issue #1954 (D6, S17, S19-S21): the install lock in `crate::install_lock`.
//!
//! Contract: the flock on `~/.amplihack/.install.lock`, re-entrant on the
//! owning thread only, blocking with no timeout, opened `O_NOFOLLOW`,
//! `O_CLOEXEC`, mode 0600 and never truncated.
//!
//! ```text
//! fn lock_path(home) -> PathBuf
//! fn acquire() / acquire_at(path) -> anyhow::Result<InstallLockGuard>
//! fn with_install_lock(f) -> anyhow::Result<T>
//! fn held_by_current_thread() -> bool
//! fn wait_message(path) -> String
//! ```
//!
//! Does not compile until `install_lock.rs` exists; register this file in
//! `tests/mod.rs` (under `#[cfg(unix)]`) in the same commit.

use crate::install_lock;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, TryRecvError};
use std::thread::JoinHandle;

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        Self { _temp: temp, home }
    }
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

#[test]
fn lock_wait_message_is_exact() {
    assert_eq!(
        install_lock::wait_message(Path::new("/home/u/.amplihack/.install.lock")),
        "⏳ waiting for another amplihack install to finish \
         (lock: /home/u/.amplihack/.install.lock)"
    );
    assert_eq!(
        install_lock::lock_path(Path::new("/home/u")),
        PathBuf::from("/home/u/.amplihack/.install.lock")
    );
}

// --------------------------------------------------------- install lock (D6)

fn lock_file(fx: &Fixture) -> PathBuf {
    install_lock::lock_path(&fx.home)
}

fn have_flock() -> bool {
    Command::new("flock")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn lock_nested_acquire_on_one_thread() {
    let fx = Fixture::new();
    let path = lock_file(&fx);
    let outer = install_lock::acquire_at(&path).unwrap();
    assert!(install_lock::held_by_current_thread());
    let inner = install_lock::acquire_at(&path).expect("re-entrant on the owning thread");
    drop(inner);
    assert!(install_lock::held_by_current_thread(), "outer still held");
    drop(outer);
    assert!(!install_lock::held_by_current_thread());
}

#[test]
fn lock_file_is_0600() {
    let fx = Fixture::new();
    let path = lock_file(&fx);
    let guard = install_lock::acquire_at(&path).unwrap();
    assert_eq!(mode(&path) & 0o777, 0o600);
    drop(guard);
}

/// What the contending thread reports, in order. No timers: the order is
/// read from the channel (S19: the lock is per thread, not per process).
#[derive(Debug, PartialEq, Eq)]
enum LockEvent {
    Attempting,
    Acquired,
}

/// The next event from the contending thread. If the thread died without
/// sending one, its own panic is raised here instead of a bare `RecvError`.
fn next_event(rx: &mpsc::Receiver<LockEvent>, thread: &mut Option<JoinHandle<()>>) -> LockEvent {
    match rx.recv() {
        Ok(event) => event,
        Err(_) => match thread.take().expect("thread joined once").join() {
            Err(payload) => std::panic::resume_unwind(payload),
            Ok(()) => panic!("the contending thread exited without sending an event"),
        },
    }
}

#[test]
fn lock_second_thread_blocks() {
    let fx = Fixture::new();
    let path = lock_file(&fx);
    let guard = install_lock::acquire_at(&path).unwrap();
    let (tx, rx) = mpsc::channel();
    let mut thread = Some({
        let path = path.clone();
        std::thread::spawn(move || {
            tx.send(LockEvent::Attempting).unwrap();
            let g = install_lock::acquire_at(&path).unwrap();
            assert!(install_lock::held_by_current_thread());
            tx.send(LockEvent::Acquired).unwrap();
            drop(g);
        })
    });

    // The thread is about to block on the lock.
    assert_eq!(next_event(&rx, &mut thread), LockEvent::Attempting);
    // While this thread holds it, no other process can take it either, and
    // the contending thread has not reported `Acquired`.
    if have_flock() {
        let other = Command::new("flock")
            .arg("-n")
            .arg(&path)
            .arg("true")
            .status()
            .unwrap();
        assert!(!other.success(), "the lock is held");
    }
    assert_eq!(
        rx.try_recv(),
        Err(TryRecvError::Empty),
        "another thread of the same process must wait while the lock is held"
    );
    assert!(install_lock::held_by_current_thread());

    drop(guard);
    assert_eq!(
        next_event(&rx, &mut thread),
        LockEvent::Acquired,
        "it acquires once released"
    );
    thread.take().unwrap().join().unwrap();
}

#[test]
fn lock_second_process_blocks() {
    if !have_flock() {
        eprintln!("skipped: no flock(1)");
        return;
    }
    let fx = Fixture::new();
    let path = lock_file(&fx);
    let guard = install_lock::acquire_at(&path).unwrap();
    let held = Command::new("flock")
        .arg("-n")
        .arg(&path)
        .arg("true")
        .status()
        .unwrap();
    assert!(!held.success(), "another process cannot take the lock");
    drop(guard);
    let free = Command::new("flock")
        .arg("-n")
        .arg(&path)
        .arg("true")
        .status()
        .unwrap();
    assert!(free.success(), "released on drop");
}

#[cfg(target_os = "linux")]
#[test]
fn post_install_child_can_take_lock() {
    if !have_flock() {
        eprintln!("skipped: no flock(1)");
        return;
    }
    let fx = Fixture::new();
    let path = lock_file(&fx);
    let guard = install_lock::acquire_at(&path).unwrap();
    // O_CLOEXEC: a child started while the lock is held does not inherit it.
    let fds = Command::new("/bin/sh")
        .args(["-c", "ls -l /proc/$$/fd"])
        .output()
        .unwrap();
    let fds = String::from_utf8_lossy(&fds.stdout);
    assert!(
        !fds.contains(".install.lock"),
        "lock fd leaked to a child:\n{fds}"
    );
    // update/check.rs drops the lock before it spawns the post-install child.
    drop(guard);
    let child = Command::new("flock")
        .arg("-n")
        .arg(&path)
        .arg("true")
        .status()
        .unwrap();
    assert!(child.success());
}

#[test]
fn lock_open_refuses_symlinked_lock_path() {
    let fx = Fixture::new();
    let path = lock_file(&fx);
    let target = fx.home.join("victim");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(&target, &path).unwrap();
    assert!(install_lock::acquire_at(&path).is_err(), "O_NOFOLLOW (S17)");
    assert!(!target.exists(), "the symlink target is never created");
    assert!(!install_lock::held_by_current_thread());
}

#[test]
fn lock_open_does_not_truncate() {
    let fx = Fixture::new();
    let path = lock_file(&fx);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "keep").unwrap();
    set_mode(&path, 0o600);
    drop(install_lock::acquire_at(&path).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "keep");
}

#[test]
fn with_install_lock_nests_on_one_thread() {
    let fx = Fixture::new();
    let _env = super::issue_1526_implicit_downgrade::IsolatedHome::new(&fx.home);
    let value = install_lock::with_install_lock(|| {
        assert!(install_lock::held_by_current_thread());
        install_lock::with_install_lock(|| Ok(7))
    })
    .unwrap();
    assert_eq!(value, 7);
    assert!(!install_lock::held_by_current_thread());
    assert!(lock_file(&fx).is_file());
}
