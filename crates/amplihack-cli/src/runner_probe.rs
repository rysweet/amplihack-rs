//! Bounded, read-only capability subprocess with owned process-group cleanup.
use anyhow::{Context, Result, ensure};
use std::{
    io::{self, Read},
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread::{self, JoinHandle},
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
        #[cfg(unix)]
        {
            // This group was created solely for this side-effect-free probe.
            let result = unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            if result != 0 {
                let error = io::Error::last_os_error();
                ensure!(
                    error.raw_os_error() == Some(libc::ESRCH),
                    "probe group cleanup failed"
                );
            }
        }
        if self.child.try_wait()?.is_none() {
            self.child.kill().context("probe termination failed")?;
        }
        self.child.wait().context("probe reap failed")?;
        self.stopped = true;
        Ok(())
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            tracing::warn!(%error, "capability probe cleanup failed");
        }
    }
}
fn reader(mut stream: impl Read + Send + 'static) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut chunk = [0; 8192];
        loop {
            let count = stream.read(&mut chunk)?;
            if count == 0 {
                return Ok(output);
            }
            let retained = count.min((LIMIT + 1).saturating_sub(output.len()));
            output.extend_from_slice(&chunk[..retained]);
        }
    })
}
fn finish(reader: JoinHandle<io::Result<Vec<u8>>>) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("capability reader panicked"))?
        .context("capability pipe read failed")
}
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
    let stdout = reader(probe.child.stdout.take().context("probe stdout missing")?);
    let stderr = reader(probe.child.stderr.take().context("probe stderr missing")?);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = probe.child.try_wait()?
            && stdout.is_finished()
            && stderr.is_finished()
        {
            probe.stop()?;
            let stdout = finish(stdout)?;
            let stderr = finish(stderr)?;
            ensure!(
                stdout.len() <= LIMIT && stderr.len() <= LIMIT,
                "capability output exceeds bound"
            );
            return Ok(Output {
                status,
                stdout,
                stderr,
            });
        }
        if Instant::now() >= deadline {
            probe.stop()?;
            anyhow::bail!("capability probe exceeded deadline");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
