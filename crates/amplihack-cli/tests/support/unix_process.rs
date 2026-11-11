//! Unix process ownership and FIFO handshakes for test fixtures.

#![allow(dead_code)]

use std::fs::OpenOptions;
use std::io;
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Release a FIFO reader without ever blocking inside `open(2)`. A reader
/// that has exited is a fixture failure, rather than a wait for a new reader.
pub fn release_fifo(path: &Path, mut consumer_alive: impl FnMut() -> bool) {
    release_fifo_with_ceiling(path, super::wait::CEILING, &mut consumer_alive);
}

fn release_fifo_with_ceiling(
    path: &Path,
    ceiling: std::time::Duration,
    mut consumer_alive: impl FnMut() -> bool,
) {
    super::wait::wait_until_with_ceiling(
        &format!("a reader of {}", path.display()),
        ceiling,
        || {
            assert!(consumer_alive(), "FIFO consumer exited: {}", path.display());
            match OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)
            {
                Ok(writer) => {
                    drop(writer);
                    true
                }
                Err(e)
                    if e.raw_os_error() == Some(libc::ENXIO)
                        || e.kind() == io::ErrorKind::Interrupted =>
                {
                    false
                }
                Err(e) => panic!("release FIFO {}: {e}", path.display()),
            }
        },
    );
}

/// Whether a recorded consumer can still reach its FIFO read. Linux zombies
/// have exited even though `kill(pid, 0)` still succeeds for them.
pub fn process_alive(pid: libc::pid_t) -> bool {
    assert!(pid > 1, "invalid fixture pid {pid}");
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        && let Some((_, rest)) = stat.rsplit_once(") ")
        && matches!(rest.as_bytes().first(), Some(b'Z' | b'X'))
    {
        return false;
    }
    // SAFETY: signal 0 probes this positive PID without delivering a signal.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Own a process group before starting fixture work. The leader parks on
/// stdin and stays owned until drop, keeping its PID reserved even after
/// fixture commands exit. Drop kills descendants on success and panic,
/// and reaps the leader.
pub struct ProcessGroup(Child);

impl ProcessGroup {
    pub fn new() -> Self {
        Self(
            Command::new("/bin/sh")
                .args(["-c", "read _line"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .expect("spawn fixture process group"),
        )
    }

    pub fn id(&self) -> libc::pid_t {
        self.0.id() as libc::pid_t
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // SAFETY: our still-owned leader created this group; it cannot have
        // been reused for another process. Signal only that group.
        unsafe { libc::kill(-self.id(), libc::SIGKILL) };
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::time::Duration;

    fn fifo(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("hold");
        assert!(
            Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        path
    }

    #[test]
    fn fifo_without_a_reader_reaches_the_existing_wait_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let path = fifo(dir.path());
        let failure = catch_unwind(|| release_fifo_with_ceiling(&path, Duration::ZERO, || true));
        let message = failure.unwrap_err().downcast::<String>().unwrap();
        assert!(message.contains("timed out"), "{message}");
    }

    #[test]
    fn fifo_release_detects_a_consumer_exiting_between_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let path = fifo(dir.path());
        let mut attempts = 0;
        let failure = catch_unwind(AssertUnwindSafe(|| {
            release_fifo(&path, || {
                attempts += 1;
                attempts == 1
            });
        }));
        let message = failure.unwrap_err().downcast::<String>().unwrap();
        assert!(message.contains("FIFO consumer exited"), "{message}");
        assert_eq!(attempts, 2, "the nonblocking open must allow another check");
    }

    #[test]
    fn fifo_release_rejects_a_consumer_that_exited_after_readiness() {
        let dir = tempfile::tempdir().unwrap();
        let path = fifo(dir.path());
        let ready = dir.path().join("ready");
        let group = ProcessGroup::new();
        let mut consumer = Command::new("/bin/sh")
            .args(["-c", ": >\"$1\"; exit 0", "consumer"])
            .arg(&ready)
            .process_group(group.id())
            .spawn()
            .unwrap();
        super::super::wait::wait_until("consumer readiness", || ready.exists());
        assert!(super::super::wait::wait_child("consumer exit", &mut consumer).success());
        let failure = catch_unwind(AssertUnwindSafe(|| {
            release_fifo(&path, || consumer.try_wait().unwrap().is_none());
        }));
        let message = failure.unwrap_err().downcast::<String>().unwrap();
        assert!(message.contains("FIFO consumer exited"), "{message}");
    }

    #[test]
    fn fifo_release_waits_for_a_reader_and_delivers_eof() {
        let dir = tempfile::tempdir().unwrap();
        let path = fifo(dir.path());
        let group = ProcessGroup::new();
        let mut reader = None;
        release_fifo(&path, || {
            if reader.is_none() {
                reader = Some(
                    Command::new("/bin/cat")
                        .arg(&path)
                        .stdout(Stdio::null())
                        .process_group(group.id())
                        .spawn()
                        .unwrap(),
                );
            }
            reader.as_mut().unwrap().try_wait().unwrap().is_none()
        });
        assert!(
            super::super::wait::wait_child("released FIFO reader", reader.as_mut().unwrap())
                .success()
        );
    }
}
