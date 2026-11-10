//! Issue #2095 (D9): unit tests for `lifetime.rs`, the test-only lifetime
//! pipe that ends a test-spawned rotator with the test process that started
//! it.
//!
//! Registered from the bottom of `lifetime.rs` as
//! `#[cfg(test)] #[path = "tests_lifetime.rs"] mod tests;`, so `super` is the
//! `lifetime` module.
//!
//! The contract these tests fix:
//!
//! - `LIFETIME_FD_ENV` is `AMPLIHACK_TEST_ROTATOR_LIFETIME_FD`.
//! - `armed_fd(var, guard, inherited_fifo)` is a pure function. It returns
//!   `Some(fd)` only when `var` is valid UTF-8 that parses, untrimmed, as an
//!   `i32` of at least 3; `guard` is exactly `"1"`; and `inherited_fifo(fd)`
//!   is true. Otherwise it returns `None`.
//! - `inherited_fifo(fd)` is true only for an open FIFO whose `FD_CLOEXEC`
//!   flag is clear (an fd inherited across `execve`).
//! - `watch_pipe(reader, stop)` starts a named thread that reads into a buffer of
//!   at most 64 bytes, ignores the data, retries on `Interrupted`, and sets
//!   `stop` on end of file or any other error.
//! - `arm(stop)` reads the process environment and does nothing, silently,
//!   when the variable is absent.
//!
//! No test sleeps, and none sets a process-wide environment variable: every
//! ordering is a thread join or a pipe end being dropped.

use super::{LIFETIME_FD_ENV, arm, armed_fd, inherited_fifo, watch_pipe};
use std::cell::Cell;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn os(s: &str) -> Option<&OsStr> {
    Some(OsStr::new(s))
}

/// A predicate that accepts any fd, so a table case is decided by the
/// variable and the guard alone.
fn any_fd(_: RawFd) -> bool {
    true
}

fn cloexec(fd: RawFd) -> bool {
    // SAFETY: F_GETFD only reads the flags of an fd this test owns.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert_ne!(flags, -1, "F_GETFD: {}", io::Error::last_os_error());
    flags & libc::FD_CLOEXEC != 0
}

fn clear_cloexec(fd: RawFd) {
    // SAFETY: F_SETFD on an fd this test owns; it changes only its flags.
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, 0) };
    assert_ne!(rc, -1, "F_SETFD: {}", io::Error::last_os_error());
}

#[test]
fn the_variable_cannot_be_stripped_by_spawn() {
    assert_eq!(LIFETIME_FD_ENV, "AMPLIHACK_TEST_ROTATOR_LIFETIME_FD");
    // `daemon::spawn` removes every `AMPLIHACK_ROTATE*` variable.
    assert!(!LIFETIME_FD_ENV.starts_with("AMPLIHACK_ROTATE"));
}

#[test]
fn a_valid_variable_guard_and_fifo_arm_the_given_fd() {
    assert_eq!(armed_fd(os("7"), os("1"), any_fd), Some(7));
    assert_eq!(
        armed_fd(os("3"), os("1"), any_fd),
        Some(3),
        "3 is the lowest"
    );
    assert_eq!(armed_fd(os("100"), os("1"), any_fd), Some(100));
}

#[test]
fn the_fifo_check_is_asked_about_the_parsed_fd() {
    let asked = Cell::new(None);
    let got = armed_fd(os("42"), os("1"), |fd| {
        asked.set(Some(fd));
        true
    });
    assert_eq!(got, Some(42));
    assert_eq!(asked.get(), Some(42));
}

#[test]
fn nothing_arms_without_every_condition() {
    let bad_utf8 = OsStr::from_bytes(b"\xff7");
    let cases: &[(&str, Option<&OsStr>, Option<&OsStr>)] = &[
        ("variable absent", None, os("1")),
        ("variable empty", os(""), os("1")),
        ("variable not a number", os("x"), os("1")),
        ("variable negative", os("-1"), os("1")),
        ("variable is stdin", os("0"), os("1")),
        ("variable is stderr", os("2"), os("1")),
        ("variable with a leading space", os(" 5"), os("1")),
        ("variable with a trailing newline", os("5\n"), os("1")),
        ("variable overflows i32", os("99999999999"), os("1")),
        ("variable not UTF-8", Some(bad_utf8), os("1")),
        ("guard absent", os("7"), None),
        ("guard empty", os("7"), os("")),
        ("guard 0", os("7"), os("0")),
        ("guard true", os("7"), os("true")),
        ("guard with a space", os("7"), os(" 1")),
    ];
    for (what, var, guard) in cases {
        assert_eq!(armed_fd(*var, *guard, any_fd), None, "{what} must not arm");
    }
    assert_eq!(
        armed_fd(os("7"), os("1"), |_| false),
        None,
        "an fd that is not an inherited FIFO must not arm"
    );
}

#[test]
fn the_fifo_check_is_not_reached_when_the_guard_fails() {
    let asked = Cell::new(false);
    let got = armed_fd(os("7"), None, |_| {
        asked.set(true);
        true
    });
    assert_eq!(got, None);
    assert!(!asked.get(), "no fd is inspected without the test guard");
}

#[test]
fn a_closed_fd_is_not_an_inherited_fifo() {
    // Numbers that cannot be open, never a dropped File's number, which a
    // parallel test could reuse.
    assert!(!inherited_fifo(i32::MAX));
    assert!(!inherited_fifo(-1));
}

#[test]
fn a_regular_file_is_not_an_inherited_fifo() {
    let file = tempfile::tempfile().unwrap();
    // Cleared, so only the file type can refuse it.
    clear_cloexec(file.as_raw_fd());
    assert!(!inherited_fifo(file.as_raw_fd()));
}

#[test]
fn a_pipe_opened_in_this_process_is_not_inherited() {
    let (reader, _writer) = io::pipe().unwrap();
    assert!(cloexec(reader.as_raw_fd()), "std opens pipes close-on-exec");
    assert!(!inherited_fifo(reader.as_raw_fd()));
}

#[test]
fn a_pipe_with_close_on_exec_clear_is_an_inherited_fifo() {
    let (reader, _writer) = io::pipe().unwrap();
    clear_cloexec(reader.as_raw_fd());
    assert!(inherited_fifo(reader.as_raw_fd()));
}

#[test]
fn the_write_end_is_close_on_exec() {
    // The harness holds the write end; a child that inherited it would keep
    // the pipe open after the test process died.
    let (_reader, writer) = io::pipe().unwrap();
    assert!(cloexec(writer.as_raw_fd()));
}

#[test]
fn dropping_the_write_end_sets_stop() {
    let (reader, writer) = io::pipe().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let handle = watch_pipe(reader, stop.clone()).unwrap();
    // The write end is open, so the reader cannot have seen end of file.
    assert!(!stop.load(Ordering::SeqCst));
    drop(writer);
    handle.join().expect("the watch thread does not panic");
    assert!(stop.load(Ordering::SeqCst), "end of file sets stop");
}

#[test]
fn data_on_the_pipe_never_sets_stop() {
    let (reader, writer) = io::pipe().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let handle = watch_pipe(reader, stop.clone()).unwrap();
    // Four times a 64 KiB pipe buffer: the writer can only finish once the
    // watch thread has read most of it, so it has handled many reads.
    let writer = std::thread::spawn(move || {
        let mut writer = writer;
        let chunk = [0x5a_u8; 4096];
        for _ in 0..64 {
            writer.write_all(&chunk).unwrap();
        }
        writer
    })
    .join()
    .expect("the writer thread does not panic");
    assert!(!stop.load(Ordering::SeqCst), "data is ignored, not a stop");
    drop(writer);
    handle.join().expect("the watch thread does not panic");
    assert!(stop.load(Ordering::SeqCst));
}

/// A reader that replays a script of results and records each buffer size.
struct Scripted {
    script: Vec<io::Result<usize>>,
    calls: Arc<std::sync::Mutex<Vec<usize>>>,
}

impl Read for Scripted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.calls.lock().unwrap().push(buf.len());
        if self.script.is_empty() {
            // A watcher that read past its stop condition would get here.
            panic!("read after the script ended");
        }
        self.script.remove(0)
    }
}

fn run_script(script: Vec<io::Result<usize>>) -> (bool, Vec<usize>) {
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let reader = Scripted {
        script,
        calls: calls.clone(),
    };
    watch_pipe(reader, stop.clone())
        .unwrap()
        .join()
        .expect("the watch thread does not panic");
    let calls = calls.lock().unwrap().clone();
    (stop.load(Ordering::SeqCst), calls)
}

#[test]
fn an_interrupted_read_is_retried_not_a_stop() {
    let (stopped, calls) = run_script(vec![
        Err(io::ErrorKind::Interrupted.into()),
        Ok(3),
        Err(io::ErrorKind::Interrupted.into()),
        Ok(0),
    ]);
    assert!(stopped, "end of file after the retries sets stop");
    assert_eq!(calls.len(), 4, "each interrupted read is retried");
}

#[test]
fn any_other_read_error_sets_stop() {
    let (stopped, calls) = run_script(vec![Ok(1), Err(io::Error::other("gone"))]);
    assert!(stopped);
    assert_eq!(calls.len(), 2, "the thread ends at the first error");
}

#[test]
fn reads_use_a_small_fixed_buffer() {
    let (_, calls) = run_script(vec![Ok(0)]);
    assert!(
        calls.iter().all(|&n| n > 0 && n <= 64),
        "buffer sizes: {calls:?}"
    );
}

#[test]
fn arm_without_the_variable_does_nothing() {
    assert!(
        std::env::var_os(LIFETIME_FD_ENV).is_none(),
        "the unit test process must not carry {LIFETIME_FD_ENV}"
    );
    let stop = Arc::new(AtomicBool::new(false));
    arm(&stop);
    assert!(!stop.load(Ordering::SeqCst));
}
