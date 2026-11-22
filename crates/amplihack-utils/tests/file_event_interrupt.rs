//! Issue #2909 (design C3): `Waiter::with_interrupt`, a wait that also
//! polls one caller-supplied readable descriptor, so a signal handler that
//! writes to a pipe ends a long wait at once. Without it a rotator waiting
//! up to `--interval` could not meet `rotate stop`'s ten-second limit.
//!
//! Contract:
//! - `Waiter::with_interrupt(&[Watch], pid, BorrowedFd<'_>) -> io::Result<Waiter>`
//!   duplicates the descriptor (close-on-exec) so the waiter owns its copy.
//! - A readable interrupt ends `wait` early with `Woke::Deadline` (no new
//!   variant) and the waiter drains the descriptor, so the next wait does not
//!   spin on the same byte.
//! - Inotify events and the interrupt can be ready together; neither is lost.
//! - `Waiter::new` is unchanged.
//!
//! No sleeps: every event happens first, and the deadlines are either now
//! (zero timeout) or far enough away that returning early is the evidence.

#![cfg(target_os = "linux")]

use amplihack_utils::file_event::{SWAP_MASK, Waiter, Watch, Woke};
use amplihack_utils::secure_files::write_private_atomic;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn private_dir() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    t
}

fn watch(dir: &std::path::Path) -> Vec<Watch> {
    vec![Watch {
        dir: dir.to_path_buf(),
        name: Some("ledger.json"),
        mask: SWAP_MASK,
    }]
}

fn waiter(dir: &std::path::Path, rx: &UnixStream) -> Waiter {
    Waiter::with_interrupt(&watch(dir), std::process::id(), rx.as_fd()).expect("with_interrupt")
}

#[test]
fn a_readable_interrupt_ends_a_long_wait_early() {
    let d = private_dir();
    let (mut tx, rx) = UnixStream::pair().unwrap();
    let mut w = waiter(d.path(), &rx);
    tx.write_all(&[1]).unwrap();
    let far = Duration::from_secs(600);
    let start = Instant::now();
    let woke = w.wait(start + far).unwrap();
    assert_eq!(woke, Woke::Deadline);
    assert!(
        start.elapsed() < far,
        "the wait returned before its deadline"
    );
}

#[test]
fn the_interrupt_is_drained_so_the_next_wait_does_not_spin() {
    let d = private_dir();
    let (mut tx, rx) = UnixStream::pair().unwrap();
    rx.set_nonblocking(true).unwrap();
    let mut w = waiter(d.path(), &rx);
    tx.write_all(&[1, 1, 1]).unwrap();
    assert_eq!(w.wait(Instant::now()).unwrap(), Woke::Deadline);
    let mut probe = [0u8; 8];
    let err = (&rx).read(&mut probe).expect_err("nothing left to read");
    assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
}

#[test]
fn a_file_event_and_an_interrupt_ready_together_are_both_seen() {
    let d = private_dir();
    let (mut tx, rx) = UnixStream::pair().unwrap();
    let mut w = waiter(d.path(), &rx);
    write_private_atomic(d.path(), "ledger.json", b"{}").unwrap();
    tx.write_all(&[1]).unwrap();
    let first = w.wait(Instant::now()).unwrap();
    let second = w.wait(Instant::now()).unwrap();
    assert!(
        first == Woke::File(0) || second == Woke::File(0),
        "the ledger event was lost: {first:?} {second:?}"
    );
}

#[test]
fn without_an_interrupt_a_quiet_waiter_still_reaches_its_deadline() {
    let d = private_dir();
    let (_tx, rx) = UnixStream::pair().unwrap();
    let mut w = waiter(d.path(), &rx);
    assert_eq!(w.wait(Instant::now()).unwrap(), Woke::Deadline);
}

#[test]
fn the_waiters_copy_of_the_interrupt_is_not_inherited_by_a_child() {
    let d = private_dir();
    let (_tx, rx) = UnixStream::pair().unwrap();
    let socket = fs::read_link(format!(
        "/proc/self/fd/{}",
        std::os::fd::AsRawFd::as_raw_fd(&rx)
    ))
    .unwrap()
    .to_string_lossy()
    .into_owned();
    let _w = waiter(d.path(), &rx);
    let out = Command::new("sh")
        .args(["-c", "ls -l /proc/self/fd"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&out.stdout);
    assert!(
        !listing.contains(&socket),
        "child inherited {socket}: {listing}"
    );
}

#[test]
fn a_closed_interrupt_peer_is_dropped_rather_than_polled_for_ever() {
    let d = private_dir();
    let (mut tx, rx) = UnixStream::pair().unwrap();
    let mut w = waiter(d.path(), &rx);
    tx.write_all(&[1]).unwrap();
    drop(tx);
    // The byte written before the close is still reported. The deadline is
    // far off, so returning at all is the evidence: a wait that discarded
    // the byte would sit here until it passed.
    let far = Duration::from_secs(600);
    let start = Instant::now();
    assert_eq!(w.wait(start + far).unwrap(), Woke::Deadline);
    assert!(
        start.elapsed() < far,
        "the pending interrupt was not reported"
    );
    // A stream socket whose peer has closed reports `POLLIN | POLLHUP` for
    // ever, drained or not, so a waiter that kept the descriptor would
    // return from every later wait at once. The deadline is the evidence:
    // a `poll(2)` with nothing ready cannot return before it.
    let short = Duration::from_millis(200);
    let start = Instant::now();
    assert_eq!(w.wait(start + short).unwrap(), Woke::Deadline);
    assert!(
        start.elapsed() >= short,
        "the wait returned early: the closed peer is still polled"
    );
}

#[test]
fn a_closed_interrupt_peer_leaves_the_watches_working() {
    let d = private_dir();
    let (tx, rx) = UnixStream::pair().unwrap();
    let mut w = waiter(d.path(), &rx);
    drop(tx);
    assert_eq!(w.wait(Instant::now()).unwrap(), Woke::Deadline);
    write_private_atomic(d.path(), "ledger.json", b"{}").unwrap();
    assert_eq!(
        w.wait(Instant::now()).unwrap(),
        Woke::File(0),
        "the ledger watch still wakes after the interrupt went"
    );
}
