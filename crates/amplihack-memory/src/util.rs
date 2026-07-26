use anyhow::{Context, Result, bail};
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CHILD_WAIT_INITIAL_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CHILD_WAIT_MAX_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SUBPROCESS_SPAWN_RETRY_TIMEOUT: Duration = Duration::from_millis(250);
const SUBPROCESS_SPAWN_RETRY_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) fn run_output_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Output> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = spawn_subprocess(&mut cmd).context("failed to spawn subprocess")?;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .context("failed to capture subprocess stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture subprocess stderr")?;
    let stdout_reader = thread::spawn(move || drain_pipe(stdout));
    let stderr_reader = thread::spawn(move || drain_pipe(stderr));

    if let Some(status) = wait_for_child_exit(&mut child, timeout)? {
        let stdout = stdout_reader
            .join()
            .map_err(|_| anyhow::anyhow!("stdout reader thread panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| anyhow::anyhow!("stderr reader thread panicked"))??;
        return Ok(Output {
            status,
            stdout,
            stderr,
        });
    }

    let _ = child.kill();
    let _ = child.wait();
    bail!("subprocess `{cmd:?}` timed out after {timeout:?} (pid {pid})")
}

fn spawn_subprocess(cmd: &mut Command) -> Result<std::process::Child> {
    let deadline = Instant::now() + SUBPROCESS_SPAWN_RETRY_TIMEOUT;
    loop {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(SUBPROCESS_SPAWN_RETRY_INTERVAL);
            }
            Err(err) => return Err(err).context("failed to spawn subprocess"),
        }
    }
}

fn wait_for_child_exit(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<Option<std::process::ExitStatus>> {
    let deadline = Instant::now() + timeout;
    let mut interval = CHILD_WAIT_INITIAL_POLL_INTERVAL;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(interval.min(deadline.saturating_duration_since(Instant::now())));
        interval = (interval * 2).min(CHILD_WAIT_MAX_POLL_INTERVAL);
    }
}

fn drain_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    pipe.read_to_end(&mut buf)?;
    Ok(buf)
}
