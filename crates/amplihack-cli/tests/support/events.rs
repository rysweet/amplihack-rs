//! Event waits for tests that drive child processes (issue #1954).
//!
//! The rule for amplihack-rs tests: no fixed waits, no polling against a
//! clock and no wall-clock windows. A child says it is ready by writing a line to a FIFO,
//! and the test blocks on reading it. A child that must wait for the test
//! blocks on reading a second FIFO, and the test writes to it.
//!
//! The protocol every child script follows (see [`SCRIPT_PREAMBLE`]):
//!
//! - The test opens the readiness FIFO with [`open_ready`] *before* it
//!   spawns the child, read-write. A read-write open of a FIFO never blocks
//!   on Linux, and because the test holds the read end from the start,
//!   nothing the child writes is lost, however early the child exits.
//! - The child opens the same FIFO read-write on fd 9 as its first action,
//!   and sets its `EXIT` trap straight after, as its second. The trap writes
//!   `exit:<status>` to fd 9, so a child that fails before it is ready still
//!   wakes the test, which panics with the status and the tail of the
//!   child's stderr instead of hanging.
//! - It opens its "go" FIFO read-write on fd 8 *before* it says it is
//!   ready, so a [`release`] that follows the ready line always finds a
//!   reader and the line it writes is never lost.
//!
//! Limits: a child killed by `SIGKILL` from outside the test, or one whose
//! fd 9 open fails, never runs its trap, and the test waits for ever. Only
//! the test's own drop guard sends `SIGKILL`, and it does so after the wait.
//! Under the no-timer rule this is accepted.
//!
//! `std` and `libc` only. There is no timeout parameter anywhere here.

#![cfg(unix)]

use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The first lines of every child script that uses this protocol. It
/// expects `$1` = work dir, `$2` = readiness FIFO, `$3` = go FIFO.
pub const SCRIPT_PREAMBLE: &str = r#"work="$1"
exec 9<>"$2"
trap 'st=$?; echo "exit:$st" >&9' EXIT
exec 8<>"$3"
"#;

/// Creates a FIFO at `path` with mode 0600. Panics with the errno.
pub fn mkfifo(path: &Path) {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .unwrap_or_else(|_| panic!("FIFO path has a NUL byte: {}", path.display()));
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call.
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    if rc != 0 {
        panic!(
            "mkfifo {} failed: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }
}

/// The last `lines` lines of the file at `path`, or a note that it is
/// missing.
pub fn tail(path: &Path, lines: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let all: Vec<&str> = text.lines().collect();
            all[all.len().saturating_sub(lines)..].join("\n")
        }
        Err(e) => format!("<{}: {e}>", path.display()),
    }
}

/// Opens the readiness FIFO `fifo` read-write. Call it before spawning the
/// child: the open never blocks, and the returned handle keeps every line
/// the child writes, even one written by a child that has already exited.
pub fn open_ready(fifo: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(fifo)
        .unwrap_or_else(|e| panic!("open {} read-write: {e}", fifo.display()))
}

/// Blocks until the child writes a line to `ready` (the handle from
/// [`open_ready`] on `fifo`), then returns it without the newline.
///
/// Panics when the line is `exit:<n>` (the child's `EXIT` trap fired before
/// it was ready), and shows the tail of `stderr_log`.
pub fn await_line(ready: File, fifo: &Path, stderr_log: &Path) -> String {
    let mut line = String::new();
    let n = BufReader::new(ready)
        .read_line(&mut line)
        .unwrap_or_else(|e| panic!("read {}: {e}", fifo.display()));
    if n == 0 {
        // The handle holds a writer of its own, so end of file cannot
        // happen; this is only a guard.
        panic!(
            "{}: unexpected end of file; stderr:\n{}",
            fifo.display(),
            tail(stderr_log, 40)
        );
    }
    let line = line.trim_end_matches('\n').to_string();
    if let Some(status) = line.strip_prefix("exit:") {
        panic!(
            "the child exited (status {status}) before it was ready; stderr:\n{}",
            tail(stderr_log, 40)
        );
    }
    line
}

/// Writes `msg` and a newline to `fifo`, whose reader is a child blocked on
/// it. The open does not block: with no reader (the child has exited) it
/// fails with `ENXIO`, and this panics instead of waiting for one.
pub fn release(fifo: &Path, msg: &str) {
    let mut file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(fifo)
        .unwrap_or_else(|e| {
            panic!(
                "open {} for writing: {e} (has its reader exited?)",
                fifo.display()
            )
        });
    file.write_all(format!("{msg}\n").as_bytes())
        .unwrap_or_else(|e| panic!("write {}: {e}", fifo.display()));
}
