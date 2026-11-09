//! Process liveness read from `/proc`, for the process-lifetime tests of
//! issue #2018. Linux only.
//!
//! No wait here is a timer. [`assert_gone`] blocks on a pidfd, which the
//! kernel makes readable when the process exits; [`wait_for_fs`] blocks on
//! an inotify descriptor, which the kernel makes readable when a directory
//! changes. A step that never ends or a file that never appears holds the
//! test until the harness stops it. Nothing here sends a signal.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// The fields of `/proc/<pid>/stat` after the command name, which is in
/// parentheses and may itself contain spaces and `)`.
fn stat_fields(pid: i32) -> Option<Vec<String>> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    Some(rest.split_whitespace().map(str::to_string).collect())
}

/// Alive and not a zombie.
pub fn alive(pid: i32) -> bool {
    stat_fields(pid)
        .and_then(|f| f.first().cloned())
        .is_some_and(|state| state != "Z" && state != "X")
}

/// Field 22 of `/proc/<pid>/stat` (start time in clock ticks after boot),
/// while `pid` is alive.
pub fn start_ticks(pid: i32) -> Option<u64> {
    if !alive(pid) {
        return None;
    }
    stat_fields(pid)?.get(19)?.parse().ok()
}

/// Field 4 of `/proc/<pid>/stat`, the parent pid, while `pid` is alive.
pub fn ppid(pid: i32) -> Option<i32> {
    if !alive(pid) {
        return None;
    }
    stat_fields(pid)?.get(1)?.parse().ok()
}

/// Blocks until `pid` has exited. Works for a process that is not a child
/// of this one. A zombie counts as exited, as in [`alive`].
fn wait_gone(pid: i32) {
    assert!(pid > 1, "refusing to watch pid {pid}");
    // SAFETY: pidfd_open takes a pid and flags and returns a new descriptor.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if raw < 0 {
        let err = io::Error::last_os_error();
        // No such process: it is already gone and reaped.
        assert_eq!(
            err.raw_os_error(),
            Some(libc::ESRCH),
            "pidfd_open({pid}): {err}"
        );
        return;
    }
    // SAFETY: `raw` is a descriptor this call just received and owns.
    let fd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
    let mut watch = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one valid pollfd; a negative timeout blocks until it is ready.
        if unsafe { libc::poll(&mut watch, 1, -1) } == 1 {
            return;
        }
        let err = io::Error::last_os_error();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted, "poll({pid}): {err}");
    }
}

/// Blocks until none of `pids` is alive, then returns.
pub fn assert_gone(pids: &[i32]) {
    for pid in pids {
        wait_gone(*pid);
    }
    assert!(
        pids.iter().all(|pid| !alive(*pid)),
        "processes {pids:?} must have exited"
    );
}

/// Blocks until `done()` holds, re-evaluating it only when the directory
/// tree at `hint` changes.
///
/// It watches the deepest directory that exists on the path to `hint` (or
/// `hint` itself, if it is one), so `hint` may not exist yet. After each
/// change it moves the watch deeper if a new directory has appeared. A watch
/// is added before `done()` runs, so a change between the two is queued and
/// wakes the next read instead of being missed.
pub fn wait_for_fs(hint: &Path, mut done: impl FnMut() -> bool) {
    // SAFETY: inotify_init1 takes only flags and returns a new descriptor.
    let raw = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
    assert!(raw >= 0, "inotify_init1: {}", io::Error::last_os_error());
    // SAFETY: `raw` is a descriptor this call just received and owns.
    let inotify = unsafe { OwnedFd::from_raw_fd(raw) };
    let mask = libc::IN_CREATE
        | libc::IN_MOVED_TO
        | libc::IN_CLOSE_WRITE
        | libc::IN_DELETE_SELF
        | libc::IN_MOVE_SELF;
    let mut buf = [0u8; 4096];
    loop {
        let dir = hint
            .ancestors()
            .find(|d| d.is_dir())
            .expect("the root directory exists");
        let c_dir = CString::new(dir.as_os_str().as_bytes()).expect("no NUL in a path");
        // SAFETY: a valid descriptor and NUL-terminated path.
        let watch = unsafe { libc::inotify_add_watch(inotify.as_raw_fd(), c_dir.as_ptr(), mask) };
        if watch < 0 {
            let err = io::Error::last_os_error();
            // The directory went between the check and the watch: look again.
            if err.raw_os_error() == Some(libc::ENOENT) {
                continue;
            }
            panic!("inotify_add_watch({}): {err}", dir.display());
        }
        if done() {
            return;
        }
        // SAFETY: `buf` is valid for `buf.len()` bytes; the read blocks until
        // the kernel queues an event.
        let n = unsafe { libc::read(inotify.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let err = io::Error::last_os_error();
            assert_eq!(
                err.kind(),
                io::ErrorKind::Interrupted,
                "inotify read: {err}"
            );
        }
    }
}

/// The pid in the file at `path`, once a stub has written it (stubs write
/// pid files by rename, so a file that exists is complete). Fails unless the
/// pid is greater than 1, so no caller can go on to signal init, its own
/// process group or every process.
pub fn read_pid_file(path: &Path) -> i32 {
    wait_for_fs(path, || path.is_file());
    let text =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let pid: i32 = text
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("{} holds {text:?}, not a pid: {e}", path.display()));
    assert!(pid > 1, "{} holds pid {pid}", path.display());
    pid
}
