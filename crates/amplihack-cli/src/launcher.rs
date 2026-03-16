//! Process launcher with managed child and graceful shutdown.
//!
//! `ManagedChild` wraps `std::process::Child` with a bounded `Drop`
//! implementation that sends SIGTERM, waits up to 3 seconds, then
//! sends SIGKILL.

use anyhow::{Context, Result};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// A child process wrapper with graceful shutdown on drop.
///
/// On drop:
/// 1. If the child already exited, do nothing.
/// 2. Send SIGTERM (Unix) or kill (Windows).
/// 3. Wait up to 3 seconds for graceful exit.
/// 4. If still alive, SIGKILL + wait.
pub struct ManagedChild {
    child: Child,
}

impl ManagedChild {
    /// Spawn a command while preserving the caller's foreground TTY.
    pub fn spawn(mut cmd: Command) -> Result<Self> {
        let child = cmd.spawn().context("failed to spawn child process")?;
        tracing::debug!(pid = child.id(), "spawned managed child");
        Ok(Self { child })
    }

    /// Non-blocking check: has the child exited?
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .context("failed to check child status")
    }

    /// Blocking wait until child exits.
    pub fn wait(&mut self) -> Result<ExitStatus> {
        self.child.wait().context("failed to wait for child")
    }

    /// Get the child's PID.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Explicitly terminate the child (SIGTERM → wait → SIGKILL).
    pub fn terminate(&mut self) {
        self.graceful_shutdown();
    }

    fn graceful_shutdown(&mut self) {
        // Already exited?
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }

        // Send SIGTERM
        #[cfg(unix)]
        {
            // SAFETY: We're sending a standard signal to a process we own.
            // The PID is valid because try_wait() above confirmed the child is still running.
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
        }

        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }

        // Wait up to 3 seconds for graceful exit
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        // Force kill
        tracing::warn!(
            pid = self.child.id(),
            "child did not exit gracefully, sending SIGKILL"
        );
        if let Err(e) = self.child.kill() {
            tracing::warn!("failed to kill child process: {e}");
        }
        if let Err(e) = self.child.wait() {
            tracing::warn!("failed to wait for child process: {e}");
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.graceful_shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::home_env_lock;

    // These tests spawn system binaries (`echo`, `sleep`, `true`, `false`, `sh`).
    // They hold `home_env_lock()` to prevent concurrent fleet/install tests from
    // narrowing PATH (to a temp stub directory) while these tests are running,
    // which would cause the system-binary lookup to fail.

    #[test]
    fn spawn_and_wait_for_exit() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cmd = Command::new("echo");
        let mut child = ManagedChild::spawn(cmd).unwrap();
        let status = child.wait().unwrap();
        assert!(status.success());
    }

    #[test]
    fn try_wait_returns_none_while_running() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut cmd = Command::new("sleep");
        cmd.arg("10");
        let mut child = ManagedChild::spawn(cmd).unwrap();

        // Should not have exited yet
        let result = child.try_wait().unwrap();
        assert!(result.is_none());

        // Drop will clean up (SIGTERM → SIGKILL)
    }

    #[test]
    fn drop_terminates_running_process() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        let child = ManagedChild::spawn(cmd).unwrap();
        let pid = child.id();

        // Drop the child — should terminate it
        drop(child);

        // Verify process is gone (on Unix)
        #[cfg(unix)]
        {
            // SAFETY: Sending signal 0 to check if a process exists is a standard
            // POSIX pattern and is safe for any PID value.
            let result = unsafe { libc::kill(pid as i32, 0) };
            assert_eq!(result, -1, "process should be dead after drop");
        }
    }

    #[test]
    fn managed_child_id() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cmd = Command::new("sleep");
        let mut cmd = cmd;
        cmd.arg("0.1");
        let child = ManagedChild::spawn(cmd).unwrap();
        assert!(child.id() > 0);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_keeps_child_in_foreground_process_group() {
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        let child = ManagedChild::spawn(cmd).unwrap();

        // SAFETY: getpgrp/getpgid only query kernel process-group state.
        let parent_pgid = unsafe { libc::getpgrp() };
        // SAFETY: child.id() is a live child PID here.
        let child_pgid = unsafe { libc::getpgid(child.id() as i32) };

        assert_eq!(child_pgid, parent_pgid);
        drop(child);
    }
}
