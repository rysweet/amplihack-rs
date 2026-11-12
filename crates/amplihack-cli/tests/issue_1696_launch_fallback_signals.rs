//! Shared attempt supervision must retain its stop behaviour on a fallback.
//! A private socket signals trap readiness and completion; no clock or probes.
#![cfg(unix)]

use super::issue_1696_fixture::{Fixture, Preserved};
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Output, Stdio};

/// Only our child's newly created process group may be killed on a failure.
struct Running(Option<Child>);
impl Running {
    fn finish(mut self) -> Output {
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            // SAFETY: process_group(0) gave this child its own group.
            unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
            let _ = child.wait();
        }
    }
}

#[test]
fn stop_signals_end_direct_and_supervised_fallbacks_without_quota_retries() {
    for entry in ["claude", "launch"] {
        for supervise in [false, true] {
            for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                // The same assertions exercise shared supervision with an
                // ordinary default and with the new lease fallback.
                for fallback in [false, true] {
                    let f = Fixture::new();
                    if fallback {
                        f.seed_hosted_lease();
                    } else {
                        f.seed_usage(&[("X", 10.0, 10.0)]);
                    }
                    let expected = if fallback { "Z" } else { "X" };
                    let before = Preserved::capture(&f);
                    let (events, notify) = UnixStream::pair().unwrap();
                    let notify_fd = notify.as_raw_fd();
                    let mut cmd = if supervise {
                        let mut cmd =
                            f.cmd(&[entry, "--subprocess-safe", "--", "-p", "stop contract"]);
                        cmd.env("AMPLIHACK_USAGE_LIMIT_WAIT", "1");
                        cmd
                    } else {
                        f.launch(entry)
                    };
                    cmd.env("SIGNAL_CONTRACT", "1")
                        .process_group(0)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());
                    // SAFETY: async-signal-safe descriptor operations only.
                    // Keep notify alive until spawn has consumed pre_exec.
                    unsafe {
                        cmd.pre_exec(move || {
                            if libc::dup2(notify_fd, 9) < 0 || libc::fcntl(9, libc::F_SETFD, 0) < 0
                            {
                                return Err(std::io::Error::last_os_error());
                            }
                            Ok(())
                        });
                    }
                    let running = Running(Some(cmd.spawn().unwrap()));
                    drop(notify);
                    let mut events = BufReader::new(events);
                    let mut line = String::new();
                    events.read_line(&mut line).unwrap();
                    assert_eq!(line, "ready\n", "Claude must install its traps first");
                    let pid = running.0.as_ref().unwrap().id() as libc::pid_t;
                    // SIGINT follows terminal delivery to the whole group.
                    // TERM/HUP exercise the supervisor's forwarding worker.
                    let target = if sig == libc::SIGINT { -pid } else { pid };
                    // SAFETY: the target is our live child or its own group.
                    assert_eq!(unsafe { libc::kill(target, sig) }, 0);
                    line.clear();
                    events.read_line(&mut line).unwrap();
                    assert_eq!(line, "stopped\n", "signal never reached Claude");
                    let out = running.finish();
                    assert_eq!(
                        out.status.code(),
                        Some(0),
                        "existing launcher stop semantics: {out:?}"
                    );
                    assert_eq!(
                        f.attempts(),
                        [expected],
                        "cancellation must prevent retries"
                    );
                    before.assert_unchanged();
                }
            }
        }
    }
}
