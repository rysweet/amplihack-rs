//! Linux test readiness without observation windows or timed retries.
#![cfg(target_os = "linux")]
use std::ffi::CString;
use std::fs;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub struct LogEvents(OwnedFd);
impl LogEvents {
    pub fn before_start(directory: &Path) -> Self {
        fs::create_dir_all(directory).unwrap();
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        // SAFETY: inotify_init1 has no pointer arguments; adopt a successful fd once.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(raw >= 0, "{}", std::io::Error::last_os_error());
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let path = CString::new(directory.as_os_str().as_bytes()).unwrap();
        // SAFETY: fd is live and path is a valid C string.
        assert!(
            unsafe {
                libc::inotify_add_watch(
                    fd.as_raw_fd(),
                    path.as_ptr(),
                    libc::IN_CREATE | libc::IN_MODIFY | libc::IN_MOVED_TO | libc::IN_CLOSE_WRITE,
                )
            } >= 0
        );
        Self(fd)
    }
    pub fn until(&self, pid: i32, log: impl Fn() -> String, predicate: impl Fn(&str) -> bool) {
        // SAFETY: pidfd_open takes a pid and flags, returning a new owned fd.
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        assert!(raw >= 0, "rotator exited before readiness: {}", log());
        let pidfd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
        loop {
            if predicate(&log()) {
                return;
            }
            let mut fds = [
                libc::pollfd {
                    fd: self.0.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: pidfd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: live descriptors and a writable two-element array. No deadline.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if n < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                panic!("log readiness failed: {error}");
            }
            let mut bytes = [0u8; 16384];
            // SAFETY: writable buffer and a live non-blocking inotify fd.
            let read =
                unsafe { libc::read(self.0.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
            if read < 0 {
                let error = std::io::Error::last_os_error();
                assert!(
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ),
                    "{error}"
                );
            }
            assert!(
                fds[1].revents == 0 || predicate(&log()),
                "rotator exited before expected event: {}",
                log()
            );
        }
    }
}

pub struct ExitEvent(OwnedFd);
impl ExitEvent {
    pub fn open(pid: i32) -> Self {
        // SAFETY: pidfd_open returns a new descriptor for the named test child.
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        assert!(
            raw >= 0,
            "cannot subscribe to test daemon exit: {}",
            std::io::Error::last_os_error()
        );
        Self(unsafe { OwnedFd::from_raw_fd(raw as i32) })
    }
    pub fn wait(self) {
        loop {
            let mut fd = libc::pollfd {
                fd: self.0.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: fd is live, the poll record is writable and there is no deadline.
            let n = unsafe { libc::poll(&mut fd, 1, -1) };
            if n > 0 {
                return;
            }
            let error = std::io::Error::last_os_error();
            assert_eq!(error.kind(), std::io::ErrorKind::Interrupted, "{error}");
        }
    }
}
