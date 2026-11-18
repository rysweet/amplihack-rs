//! Process authority is a live, unreaped Child, never a process-name match.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::BufRead;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, ExitStatus};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    ProcessReady {
        pid: u32,
    },
    WorkReady {
        run_id: String,
        pid: u32,
        commit: String,
        dirty_path: String,
    },
}
pub fn read_event(reader: &mut impl BufRead) -> Result<Event> {
    let mut line = String::new();
    ensure!(
        reader.read_line(&mut line)? > 0 && line.ends_with('\n'),
        "event pipe ended without a complete record"
    );
    let event: Event = serde_json::from_str(&line)?;
    match &event {
        Event::ProcessReady { pid } => ensure!(*pid > 0, "invalid child PID"),
        Event::WorkReady {
            run_id,
            pid,
            commit,
            dirty_path,
        } => {
            ensure!(
                *pid > 0 && !run_id.is_empty() && !dirty_path.is_empty(),
                "incomplete work event"
            );
            ensure!(
                commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid event commit"
            );
        }
    }
    Ok(event)
}

pub struct TrackedChild {
    child: Child,
    reaped: bool,
}
impl TrackedChild {
    pub fn spawn(command: &mut Command, record: &Path) -> Result<Self> {
        let mut child = command.spawn()?;
        if let Err(error) = fs::write(record, format!("{}\n", child.id())) {
            // This exact process was just started here; no other PID is signaled.
            child.kill()?;
            child.wait()?;
            return Err(error.into());
        }
        Ok(Self {
            child,
            reaped: false,
        })
    }
    pub fn id(&self) -> u32 {
        self.child.id()
    }
    pub fn take_stdout(&mut self) -> Result<ChildStdout> {
        self.child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("child stdout is not piped or was already taken"))
    }
    pub fn interrupt_and_wait(&mut self) -> Result<ExitStatus> {
        ensure!(
            !self.reaped,
            "child has already been reaped; signaling authority is sealed"
        );
        ensure!(
            self.child.try_wait()?.is_none(),
            "child exited before interruption"
        );
        // SAFETY: this is our live, unreaped direct child, so its PID cannot be reused.
        let result = unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        ensure!(
            result == 0,
            "interrupt child: {}",
            std::io::Error::last_os_error()
        );
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }
}
impl Drop for TrackedChild {
    fn drop(&mut self) {
        if !self.reaped {
            match self.child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    if let Err(error) = self.child.kill() {
                        eprintln!("kill recorded child {}: {error}", self.child.id());
                    }
                    if let Err(error) = self.child.wait() {
                        eprintln!("reap recorded child {}: {error}", self.child.id());
                    }
                }
                Err(error) => eprintln!("inspect recorded child {}: {error}", self.child.id()),
            }
        }
    }
}

/// Wait for either an explicit Unix-pipe event connection or this child's exit.
/// There is no elapsed-time assumption and no polling interval.
#[cfg(target_os = "linux")]
pub fn accept_event(
    listener: &std::os::unix::net::UnixListener,
    child: &TrackedChild,
) -> Result<std::os::unix::net::UnixStream> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // SAFETY: pidfd_open takes a live child PID and zero flags, returning a new fd.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id(), 0) } as i32;
    ensure!(fd >= 0, "pidfd_open: {}", std::io::Error::last_os_error());
    // SAFETY: fd is newly owned and valid after the check above.
    let pidfd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut descriptors = [
        libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    // SAFETY: both fds stay alive; the array is writable for its declared length.
    let status = unsafe {
        libc::poll(
            descriptors.as_mut_ptr(),
            descriptors.len() as libc::nfds_t,
            -1,
        )
    };
    ensure!(
        status > 0,
        "event wait failed: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        descriptors[0].revents & libc::POLLIN != 0,
        "recorded child exited before its readiness event"
    );
    Ok(listener.accept()?.0)
}

impl TrackedChild {
    pub fn wait(&mut self) -> Result<ExitStatus> {
        ensure!(!self.reaped, "child has already been reaped");
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }
}

impl TrackedChild {
    pub fn take_stdin(&mut self) -> Result<std::process::ChildStdin> {
        self.child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("child stdin is not piped or was already taken"))
    }
}
