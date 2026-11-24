//! Issue #3172: `lock_exclusive` is a blocking `flock(LOCK_EX)` with no
//! timeout, no retry count and no sleep. It retries only when interrupted.
//! The lock is released when the returned guard is dropped.

#![cfg(unix)]

use amplihack_utils::secure_files::lock_exclusive;
use std::fs::{self, File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::Path;

fn open(p: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(p)
        .unwrap()
}

/// Whether a non-blocking exclusive lock on a fresh descriptor would fail
/// because someone else holds the lock.
fn would_block(p: &Path) -> bool {
    let probe = open(p);
    // SAFETY: the descriptor is open and owned by `probe`.
    let r = unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if r == 0 {
        return false;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK)
}

#[test]
fn lock_exclusive_holds_the_lock_until_dropped() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("lock");
    let guard = lock_exclusive(open(&p)).unwrap();
    assert!(would_block(&p), "a second descriptor must not get the lock");
    drop(guard);
    assert!(!would_block(&p), "dropping the guard releases the lock");
}

#[test]
fn lock_exclusive_waits_for_the_holder_instead_of_giving_up() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("lock");
    let first = lock_exclusive(open(&p)).unwrap();
    let file = open(&p);
    let waiter = std::thread::spawn(move || lock_exclusive(file).map(drop));
    // The holder lets go; the waiter, however long it has already waited,
    // takes the lock and returns Ok. A timed lock could not promise this.
    drop(first);
    waiter.join().unwrap().unwrap();
    assert!(!would_block(&p));
}

#[test]
fn lock_exclusive_works_on_an_empty_and_a_written_file() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("lock");
    fs::write(&p, b"content").unwrap();
    let guard = lock_exclusive(open(&p)).unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"content", "locking writes nothing");
    drop(guard);
}
