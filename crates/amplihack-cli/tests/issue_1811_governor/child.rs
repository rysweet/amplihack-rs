//! Process ownership and cleanup regressions for the governor fixture.

use super::Fixture;
use std::fs;
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStderr, ChildStdout, Command, Output, Stdio};
use std::time::Duration;

/// Owns the wrapper and its FIFO-blocked shell/cat descendants from spawn
/// onward, including while an assertion or readiness wait unwinds.
pub(super) struct GovernedChild {
    child: Option<Child>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    out: Vec<u8>,
    err: Vec<u8>,
}

impl GovernedChild {
    pub(super) fn spawn(cmd: &mut Command) -> Self {
        let child = cmd
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn governed claude");
        // Establish cleanup ownership before configuring the pipes.
        let mut owned = Self {
            child: Some(child),
            stdout: None,
            stderr: None,
            out: Vec::new(),
            err: Vec::new(),
        };
        let child = owned.child.as_mut().unwrap();
        owned.stdout = child.stdout.take();
        owned.stderr = child.stderr.take();
        for fd in [
            owned.stdout.as_ref().unwrap().as_raw_fd(),
            owned.stderr.as_ref().unwrap().as_raw_fd(),
        ] {
            // SAFETY: these descriptors belong to our live, owned pipes.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(
                flags >= 0,
                "read pipe flags: {}",
                io::Error::last_os_error()
            );
            let set = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
            assert!(
                set >= 0,
                "set nonblocking pipe: {}",
                io::Error::last_os_error()
            );
        }
        owned
    }

    pub(super) fn try_output(&mut self) -> Option<Output> {
        // Neither reading nor polling may block the caller's deadline check,
        // even when an exited wrapper leaves a descendant holding a pipe.
        Self::drain(&mut self.stdout, &mut self.out);
        Self::drain(&mut self.stderr, &mut self.err);
        let status = self
            .child
            .as_mut()?
            .try_wait()
            .expect("poll governed claude")?;
        if self.stdout.is_some() || self.stderr.is_some() {
            return None;
        }
        self.child.take();
        Some(Output {
            status,
            stdout: std::mem::take(&mut self.out),
            stderr: std::mem::take(&mut self.err),
        })
    }

    fn drain<R: Read>(pipe: &mut Option<R>, into: &mut Vec<u8>) {
        let Some(reader) = pipe.as_mut() else { return };
        let mut buf = [0; 8192];
        // Bound each poll even if a descendant writes continuously.
        for _ in 0..16 {
            match reader.read(&mut buf) {
                Ok(0) => {
                    *pipe = None;
                    return;
                }
                Ok(n) => into.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => panic!("read governed claude output: {e}"),
            }
        }
    }

    pub(super) fn output(self) -> Output {
        self.output_with_ceiling(crate::support::wait::CEILING)
    }

    fn output_with_ceiling(mut self, ceiling: Duration) -> Output {
        // Keep ownership through process exit AND pipe EOF. One deadline
        // covers both; Drop kills the group on a timeout in either phase.
        let mut output = None;
        crate::support::wait::wait_until_with_ceiling(
            "a governed claude to exit and close its output",
            ceiling,
            || {
                output = self.try_output();
                output.is_some()
            },
        );
        output.unwrap()
    }
}

impl Drop for GovernedChild {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            // SAFETY: spawn put this child in its own process group. The
            // negative PID targets only that group, including blocked stubs.
            unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
            let _ = child.wait();
        }
    }
}

#[test]
fn governor_output_deadline_covers_pipes_held_after_leader_exit() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::mpsc;

    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("hold");
    let path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: path is a valid, NUL-terminated name in our temporary directory.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "/bin/cat \"$1\" & exit 0", "wrapper"])
        .arg(&fifo);
    let mut child = GovernedChild::spawn(&mut cmd);
    crate::support::wait::wait_until("the wrapper to exit before its descendant", || {
        child.child.as_mut().unwrap().try_wait().unwrap().is_some()
    });
    // Duplicate the read ends so EOF after timeout also proves cleanup of
    // the descendant holding both writers, even though its leader is gone.
    let mut pipes = vec![
        fs::File::from(
            child
                .stdout
                .as_ref()
                .unwrap()
                .as_fd()
                .try_clone_to_owned()
                .unwrap(),
        ),
        fs::File::from(
            child
                .stderr
                .as_ref()
                .unwrap()
                .as_fd()
                .try_clone_to_owned()
                .unwrap(),
        ),
    ];
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let failed = catch_unwind(AssertUnwindSafe(|| {
            child.output_with_ceiling(Duration::ZERO)
        }));
        let timed_out = failed
            .err()
            .and_then(|e| e.downcast::<String>().ok())
            .is_some_and(|message| message.contains("timed out"));
        let _ = tx.send(timed_out);
    });
    assert!(
        rx.recv_timeout(crate::support::wait::CEILING)
            .expect("output polling blocked after the wrapper exited"),
        "inherited output pipes must exhaust the deadline"
    );
    crate::support::wait::wait_until("the orphaned descendant to close both output pipes", || {
        pipes.retain_mut(|pipe| match pipe.read(&mut [0; 1]) {
            Ok(0) => false,
            Ok(_) => panic!("the FIFO-blocked descendant should emit no output"),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                true
            }
            Err(e) => panic!("read descendant output: {e}"),
        });
        pipes.is_empty()
    });
}

#[test]
fn governor_failures_kill_and_reap_wrappers_and_blocked_descendants() {
    use std::io::Read;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::mpsc;
    use std::time::Duration;

    for timeout in [false, true] {
        let f = Fixture::new();
        f.readings(&[("alpha", 10.0, 20.0)]);
        f.governor_config("max_concurrency = 2");
        let (tx, rx) = mpsc::channel();
        let mut pids = Vec::new();
        let mut held_before_failure = false;
        let failed = catch_unwind(AssertUnwindSafe(|| {
            let children: Vec<_> = (0..3)
                .map(|_| {
                    let mut child = GovernedChild::spawn(
                        f.cmd(&["claude", "--", "-p", "x"])
                            .env("AMPLIHACK_GOVERNOR_WAIT", "120")
                            .env("STUB_HOLD", "1"),
                    );
                    pids.push(child.child.as_ref().unwrap().id());
                    let mut pipe = child.stdout.take().unwrap();
                    // This observer uses blocking reads; the helper normally
                    // drains this same descriptor in nonblocking mode.
                    let fd = pipe.as_raw_fd();
                    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                    assert!(flags >= 0);
                    assert_eq!(
                        unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) },
                        0
                    );
                    let tx = tx.clone();
                    std::thread::spawn(move || {
                        let result = pipe.read_to_end(&mut Vec::new());
                        let _ = tx.send(result);
                    });
                    child
                })
                .collect();
            crate::support::wait::wait_until("FIFO-held stubs before a fixture failure", || {
                f.held(&Default::default()).len() >= 2
            });
            held_before_failure = true;
            if timeout {
                crate::support::wait::wait_until_with_ceiling(
                    "an intentionally missing readiness event",
                    Duration::ZERO,
                    || false,
                );
            } else {
                assert!(
                    children.is_empty(),
                    "intentional admission assertion failure"
                );
            }
        }));
        assert!(failed.is_err(), "the failure path must unwind");
        assert!(held_before_failure, "the stubs must be held before failing");
        for pid in pids {
            // SAFETY: waitpid writes nothing with a null status pointer.
            let waited =
                unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), libc::WNOHANG) };
            assert_eq!(waited, -1, "wrapper {pid} was not reaped");
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
            // EOF requires every inherited writer (wrapper, shell and cat)
            // to close, so killing just the wrapper cannot pass this check.
            rx.recv_timeout(crate::support::wait::CEILING)
                .expect("waiting for killed governor descendants to close stdout")
                .expect("read governor stdout");
        }
    }
}
