//! Bounded, read-only capability subprocess with owned process-group cleanup.
use anyhow::{Context, Result, ensure};
use std::{
    io::{self, Read},
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
const LIMIT: usize = 65_536;

struct Probe {
    child: Child,
    stopped: bool,
}
impl Probe {
    fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        // One cleanup budget, including the destructor safety path. Never retry a
        // failed reap with another deadline after reporting it to the caller.
        self.stopped = true;
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut failures = Vec::new();
        #[cfg(unix)]
        {
            let signal = libc::SIGKILL;
            #[cfg(test)]
            let signal = testing::termination_signal(signal);
            let result = unsafe { libc::kill(-(self.child.id() as i32), signal) };
            if result != 0 && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                failures.push("probe group cleanup failed");
            }
        }
        match self.cleanup_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                if self.cleanup_kill().is_err() {
                    failures.push("probe termination failed");
                }
            }
            Err(_) => {
                failures.push("probe status cleanup failed");
                if self.cleanup_kill().is_err() {
                    failures.push("probe termination failed");
                }
            }
        }
        loop {
            match self.cleanup_wait() {
                Ok(Some(_)) => break,
                Err(_) => {
                    failures.push("probe reap failed");
                    break;
                }
                Ok(None) if Instant::now() >= deadline => {
                    failures.push("probe cleanup exceeded deadline");
                    break;
                }
                Ok(None) => thread::sleep(Duration::from_millis(5)),
            }
        }
        ensure!(failures.is_empty(), "{}", failures.join("; "));
        #[cfg(test)]
        testing::check()?;
        Ok(())
    }
    fn cleanup_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        #[cfg(all(test, unix))]
        testing::reap(self.child.id())?;
        self.child.try_wait()
    }
    fn cleanup_kill(&mut self) -> io::Result<()> {
        #[cfg(all(test, unix))]
        if testing::fault(testing::Fault::Termination) {
            return testing::failed_signal(self.child.id());
        }
        self.child.kill()
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            tracing::warn!(%error, "capability probe cleanup failed");
        }
    }
}
#[cfg(unix)]
#[path = "runner_probe/unix_pipe.rs"]
mod pipe;
#[cfg(windows)]
#[path = "runner_probe/windows_pipe.rs"]
mod pipe;
#[cfg(any(unix, windows))]
use pipe::Pipe;
#[cfg(any(unix, windows))]
pub(crate) fn capture(binary: &Path) -> Result<Output> {
    let mut command = Command::new(binary);
    command
        .arg("--capabilities")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut probe = Probe {
        child: command.spawn().context("capability probe spawn failed")?,
        stopped: false,
    };
    #[cfg(all(test, unix))]
    testing::record_child(probe.child.id());
    #[cfg(all(test, windows))]
    testing::record_windows_handles(&probe.child);
    let result = (|| {
        let mut stdout = Pipe::new(probe.child.stdout.take().context("probe stdout missing")?)?;
        let mut stderr = Pipe::new(probe.child.stderr.take().context("probe stderr missing")?)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            stdout.drain()?;
            stderr.drain()?;
            if let Some(status) = probe
                .child
                .try_wait()
                .context("capability probe status failed")?
                && stdout.eof
                && stderr.eof
            {
                ensure!(
                    stdout.bytes.len() <= LIMIT && stderr.bytes.len() <= LIMIT,
                    "capability output exceeds bound"
                );
                return Ok(Output {
                    status,
                    stdout: stdout.bytes,
                    stderr: stderr.bytes,
                });
            }
            ensure!(
                Instant::now() < deadline,
                "capability probe exceeded deadline"
            );
            thread::sleep(Duration::from_millis(10));
        }
    })();
    match (result, probe.stop()) {
        (Ok(output), Ok(())) => Ok(output),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(error), Err(cleanup)) => Err(anyhow::anyhow!(
            "{error:#}; capability cleanup failed: {cleanup:#}"
        )),
    }
}
/// Reject platforms without a bounded pipe mechanism before starting a child.
#[cfg(not(any(unix, windows)))]
pub(crate) fn capture(_binary: &Path) -> Result<Output> {
    anyhow::bail!("bounded capability capture is unsupported on this platform")
}

#[cfg(test)]
#[path = "runner_probe/testing.rs"]
pub(crate) mod testing;
