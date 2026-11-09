use anyhow::{Context, Result, bail};
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CHILD_WAIT_INITIAL_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CHILD_WAIT_MAX_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SUBPROCESS_SPAWN_RETRY_TIMEOUT: Duration = Duration::from_millis(250);
const SUBPROCESS_SPAWN_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Run `cmd` to completion with stdout and stderr captured and no time limit
/// (issue #2022). Both streams are drained concurrently so a child with large
/// output cannot deadlock the wait.
pub(crate) fn run_output_untimed(cmd: Command) -> Result<Output> {
    run_output_inner(cmd, None)
}

fn run_output_inner(mut cmd: Command, timeout: Option<Duration>) -> Result<Output> {
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
    let timeout = timeout.expect("wait returns without exit only when a limit is set");
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

/// Returns `Ok(None)` only when `timeout` is `Some` and has elapsed. With
/// `None` (or a limit too large to represent) it blocks in `wait()` until
/// the child exits instead of polling.
fn wait_for_child_exit(
    child: &mut std::process::Child,
    timeout: Option<Duration>,
) -> Result<Option<std::process::ExitStatus>> {
    let Some(deadline) = timeout.and_then(|t| Instant::now().checked_add(t)) else {
        return Ok(Some(child.wait()?));
    };
    let mut interval = CHILD_WAIT_INITIAL_POLL_INTERVAL;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        thread::sleep(interval.min(deadline.saturating_duration_since(now)));
        interval = (interval * 2).min(CHILD_WAIT_MAX_POLL_INTERVAL);
    }
}

fn drain_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    pipe.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue #2022: SCIP indexing is a build-type job and runs without a
    /// limit. Large output on both streams is drained concurrently, so the
    /// untimed wait cannot deadlock on a full pipe.
    #[cfg(unix)]
    #[test]
    fn run_output_untimed_drains_both_streams() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args([
            "-c",
            "head -c 70000 /dev/zero; head -c 70000 /dev/zero >&2; exit 3",
        ]);
        let output = run_output_untimed(cmd).unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout.len(), 70_000);
        assert_eq!(output.stderr.len(), 70_000);
    }

    /// No limit: the wait lasts until the child exits, and the child exits
    /// only after the test releases it over stdin.
    #[cfg(unix)]
    #[test]
    fn run_output_untimed_waits_until_the_child_is_released() {
        use std::io::Write as _;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let (reader, mut writer) = std::io::pipe().unwrap();
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "read -r _; echo indexed"]).stdin(reader);
        let released = Arc::new(AtomicBool::new(false));
        let release = {
            let released = Arc::clone(&released);
            thread::spawn(move || {
                released.store(true, Ordering::SeqCst);
                writer.write_all(b"go\n").unwrap();
            })
        };
        let output = run_output_untimed(cmd).unwrap();
        release.join().unwrap();
        assert!(released.load(Ordering::SeqCst));
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "indexed");
    }

    #[test]
    fn run_output_untimed_reports_a_spawn_failure() {
        let cmd = Command::new("/nonexistent/amplihack-2022-no-such-indexer");
        assert!(run_output_untimed(cmd).is_err());
    }
}
