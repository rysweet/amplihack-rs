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
            let result = unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            if result != 0 && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                failures.push("probe group cleanup failed");
            }
        }
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                if self.child.kill().is_err() {
                    failures.push("probe termination failed");
                }
            }
            Err(_) => {
                failures.push("probe status cleanup failed");
                if self.child.kill().is_err() {
                    failures.push("probe termination failed");
                }
            }
        }
        loop {
            match self.child.try_wait() {
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
}
impl Drop for Probe {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            tracing::warn!(%error, "capability probe cleanup failed");
        }
    }
}
#[cfg(not(unix))]
fn reader(mut stream: impl Read + Send + 'static) -> std::thread::JoinHandle<io::Result<Vec<u8>>> {
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
#[cfg(not(unix))]
fn finish(reader: std::thread::JoinHandle<io::Result<Vec<u8>>>) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("capability reader panicked"))?
        .context("capability pipe read failed")
}
#[cfg(unix)]
struct Pipe<T> {
    stream: T,
    bytes: Vec<u8>,
    eof: bool,
}
#[cfg(unix)]
impl<T: Read + std::os::fd::AsRawFd> Pipe<T> {
    fn new(stream: T) -> Result<Self> {
        let fd = stream.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        ensure!(
            flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
            "capability pipe nonblocking setup failed"
        );
        Ok(Self {
            stream,
            bytes: Vec::new(),
            eof: false,
        })
    }
    fn drain(&mut self) -> Result<()> {
        // A fixed quantum keeps continuous writers from starving the deadline or other pipe.
        for _ in 0..8 {
            let mut chunk = [0; 8192];
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => {
                    let retained = n.min((LIMIT + 1).saturating_sub(self.bytes.len()));
                    self.bytes.extend_from_slice(&chunk[..retained]);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => anyhow::bail!("capability pipe read failed"),
            }
        }
        Ok(())
    }
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
    #[cfg(unix)]
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
    #[cfg(unix)]
    return match (result, probe.stop()) {
        (Ok(output), Ok(())) => Ok(output),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(error), Err(cleanup)) => Err(anyhow::anyhow!(
            "{error:#}; capability cleanup failed: {cleanup:#}"
        )),
    };
    #[cfg(not(unix))]
    {
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
}
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    type CleanupHook = Box<dyn FnMut() -> Result<()>>;
    thread_local! { static HOOK: std::cell::RefCell<Option<CleanupHook>> = std::cell::RefCell::new(None); }
    #[allow(dead_code)] // The source is also included by lifecycle-only integration tests.
    pub fn with_cleanup_result_hook<R>(
        hook: impl FnMut() -> Result<()> + 'static,
        run: impl FnOnce() -> R,
    ) -> R {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                HOOK.with(|h| *h.borrow_mut() = None);
            }
        }
        HOOK.with(|h| {
            assert!(h.borrow().is_none());
            *h.borrow_mut() = Some(Box::new(hook));
        });
        let _reset = Reset;
        run()
    }
    pub(super) fn check() -> Result<()> {
        HOOK.with(|h| match h.borrow_mut().as_mut() {
            Some(h) => h(),
            None => Ok(()),
        })
    }
}
