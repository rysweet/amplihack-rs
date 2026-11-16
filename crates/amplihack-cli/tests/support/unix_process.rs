//! Unix process ownership and FIFO handshakes for test fixtures.

#![cfg(unix)]
#![allow(dead_code)]

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;

/// The test's own ends of a FIFO that fixture shells block on with builtin
/// reads. [`FifoRelease::release`] writes one line per read.
struct FifoRelease {
    writer: std::fs::File,
    reader: std::fs::File,
    readers: usize,
}

impl FifoRelease {
    fn open(path: &Path, readers: usize) -> Self {
        assert!((1..=512).contains(&readers), "invalid FIFO reader count");
        // Separate nonblocking opens are portable Unix FIFO semantics; no
        // O_RDWR assumption and no blocking open before the worker starts.
        let reader = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .expect("open supervisor FIFO reader");
        let writer = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .expect("open supervisor FIFO writer");
        Self {
            writer,
            reader,
            readers,
        }
    }

    /// Clones of both ends for the worker thread, so release lines written
    /// before its fixture's first read stay buffered.
    fn keepalive(&self) -> (std::fs::File, std::fs::File) {
        (
            self.reader
                .try_clone()
                .expect("clone supervisor FIFO reader"),
            self.writer
                .try_clone()
                .expect("clone supervisor FIFO writer"),
        )
    }

    fn release(&mut self) {
        // One line per builtin read. Tree fixtures must open their FIFO fd
        // before spawning descendants, so late readers retain the release.
        // The private FIFO is empty until this write, well below PIPE_BUF.
        if let Err(e) = self.writer.write_all(&vec![b'\n'; self.readers]) {
            eprintln!("releasing a test's FIFO readers failed: {e}");
        }
    }
}

/// Supervise work whose fixture blocks on builtin FIFO reads. Only drop
/// releases the reads, so cleanup cannot turn a timeout regression into a
/// passing test. Keep this guard through assertions about surviving children.
pub struct FifoWorker<T> {
    result: mpsc::Receiver<T>,
    worker: Option<JoinHandle<()>>,
    fifo: Option<FifoRelease>,
}

impl<T: Send + 'static> FifoWorker<T> {
    pub fn spawn(path: &Path, readers: usize, work: impl FnOnce() -> T + Send + 'static) -> Self {
        Self::spawn_with(Some((path, readers)), work)
    }

    /// [`FifoWorker::spawn`] with the FIFO optional: with `None` there is
    /// nothing to release, and drop only waits for the work.
    pub fn spawn_with(
        fifo: Option<(&Path, usize)>,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Self {
        let fifo = fifo.map(|(path, readers)| FifoRelease::open(path, readers));
        let keepalive = fifo.as_ref().map(FifoRelease::keepalive);
        let (tx, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            // Preserve buffered release lines even if unwinding releases the
            // FIFO before this worker has started its fixture's first read.
            let _keepalive = keepalive;
            let _ = tx.send(work());
        });
        Self {
            result,
            worker: Some(worker),
            fifo,
        }
    }

    pub fn wait(&mut self) -> T {
        self.wait_with_ceiling(super::wait::CEILING)
    }

    pub fn wait_with_ceiling(&mut self, ceiling: std::time::Duration) -> T {
        let result = self
            .result
            .recv_timeout(ceiling)
            .expect("hung: fixture worker did not return before the ceiling");
        self.worker
            .take()
            .unwrap()
            .join()
            .expect("fixture worker panicked");
        result
    }
}

impl<T> Drop for FifoWorker<T> {
    fn drop(&mut self) {
        if let Some(fifo) = &mut self.fifo {
            fifo.release();
        }
        if let Some(worker) = self.worker.take() {
            // Cleanup is independent of product timeout/termination: released
            // shells exit naturally. Wait before the caller restores its env
            // or removes its tempdir. Never panic again during unwinding.
            match self.result.recv_timeout(super::wait::CEILING) {
                Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = worker.join();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    eprintln!("hung: fixture worker did not return after fixture cleanup");
                }
            }
        }
    }
}

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
    fn fifo_worker_ceiling_releases_runner_and_descendant_before_restoring_env() {
        let dir = tempfile::tempdir().unwrap();
        let path = fifo(dir.path());
        let runner_marker = dir.path().join("runner-done");
        let child_marker = dir.path().join("child-done");
        let failure = catch_unwind(|| {
            let (worker_path, runner_marker, child_marker) =
                (path.clone(), runner_marker.clone(), child_marker.clone());
            let mut worker = FifoWorker::spawn(&path, 2, move || {
                let group = ProcessGroup::new();
                let mut runner = Command::new("/bin/sh")
                    .args([
                        "-c",
                        "exec 3<\"$1\"; (read _hold <&3; : >\"$2\") & read _hold <&3; wait; : >\"$3\"",
                        "fixture",
                    ])
                    .args([worker_path, child_marker, runner_marker])
                    .process_group(group.id())
                    .spawn()
                    .unwrap();
                assert!(
                    super::super::wait::wait_child("released runner tree", &mut runner).success()
                );
            });
            // No product timeout or termination code runs in this worker.
            // Even release before its first open must reach both reads.
            worker.wait_with_ceiling(Duration::ZERO);
        });
        let message = failure.unwrap_err().downcast::<String>().unwrap();
        assert!(message.contains("hung: fixture worker"), "{message}");
        assert!(
            runner_marker.exists(),
            "cleanup must finish before unwinding returns"
        );
        assert!(
            child_marker.exists(),
            "cleanup must also release the descendant"
        );
    }

    #[test]
    fn fifo_worker_drop_releases_a_survivor_after_worker_returned() {
        let dir = tempfile::tempdir().unwrap();
        let path = fifo(dir.path());
        let ready = dir.path().join("ready");
        let group = ProcessGroup::new();
        let mut worker = FifoWorker::spawn(&path, 1, || ());
        let mut survivor = Command::new("/bin/sh")
            .args(["-c", "exec 3<\"$1\"; : >\"$2\"; read _hold <&3", "survivor"])
            .args([&path, &ready])
            .process_group(group.id())
            .spawn()
            .unwrap();
        super::super::wait::wait_until("survivor to open its FIFO", || ready.exists());
        worker.wait();
        assert!(survivor.try_wait().unwrap().is_none());
        drop(worker);
        assert!(super::super::wait::wait_child("released survivor", &mut survivor).success());
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
