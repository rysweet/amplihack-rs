//! Single-attempt GitHub mutations with bounded subprocess IO and execution.
//! A failed call never establishes that the external mutation was not applied.
use super::issue::{IssueRef, parse_optional};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(120);
const OUTPUT_LIMIT: u64 = 64 * 1024;

pub(super) fn command(directory: &Path) -> Command {
    let mut command = Command::new("gh");
    command
        .current_dir(directory)
        .env("GH_HOST", "github.com")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env_remove("GH_REPO")
        .env_remove("GH_DEBUG");
    command
}

/// Both callers persist a prepared attempt before entering this boundary and
/// preserve every error as requiring reconciliation. No mutation is retried here.
pub(super) fn execute(
    command: &mut Command,
    body: Option<&str>,
    artefacts: &Path,
) -> Result<String> {
    // A private, unlinked input file cannot block when gh stops reading stdin.
    let input = if let Some(body) = body {
        let mut file = tempfile::tempfile_in(artefacts)?;
        file.write_all(body.as_bytes())?;
        file.seek(SeekFrom::Start(0))?;
        Stdio::from(file)
    } else {
        Stdio::null()
    };
    command
        .stdin(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    // Do not start a mutation on platforms lacking owned, bounded cleanup.
    #[cfg(not(unix))]
    anyhow::bail!("bounded GitHub mutations are unsupported on this platform");
    #[cfg(unix)]
    {
        let mut child = OwnedChild(command.spawn().context("start gh mutation")?, false);
        let stdout = child.0.stdout.take().context("missing gh stdout")?;
        let stderr = child.0.stderr.take().context("missing gh stderr")?;
        let out = thread::Builder::new().spawn(move || read(stdout))?;
        let err = thread::Builder::new().spawn(move || read(stderr))?;
        let status = wait(&mut child, DEADLINE);
        if !child.1 {
            // A waiter thread can fail to start. Close the child's pipes before
            // joining readers on this error path as well.
            stop(&mut child.0);
            child.1 = true;
        }
        // wait kills the owned group before reaping it, including on success,
        // so descendants cannot retain the pipes and prevent EOF.
        let stdout = out
            .join()
            .map_err(|_| anyhow::anyhow!("gh stdout reader panicked"));
        let stderr = err
            .join()
            .map_err(|_| anyhow::anyhow!("gh stderr reader panicked"));
        let status = status?;
        let stdout = stdout??;
        stderr??;
        // Do not persist untrusted stderr: gh diagnostics can include secrets.
        ensure!(
            status.success(),
            "gh mutation failed ({status}); inspect GitHub before retrying"
        );
        String::from_utf8(stdout).context("gh returned non-UTF-8 output")
    }
}

fn read(stream: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream.take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= OUTPUT_LIMIT,
        "gh output exceeds the capture limit"
    );
    Ok(bytes)
}

#[cfg(unix)]
struct OwnedChild(Child, bool);

#[cfg(unix)]
impl Drop for OwnedChild {
    fn drop(&mut self) {
        // wait marks a reaped child. Never signal its old PID.
        if !self.1 {
            stop(&mut self.0);
        }
    }
}

#[cfg(unix)]
fn stop(child: &mut Child) {
    // The child is not reaped until after this signal, so its group ID cannot
    // have been reused. Only the group created for this invocation is affected.
    if let Ok(pid) = i32::try_from(child.id())
        && pid > 1
    {
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn wait(child: &mut OwnedChild, timeout: Duration) -> Result<ExitStatus> {
    let pid = libc::id_t::from(child.0.id());
    let (tx, rx) = mpsc::channel();
    let waiter = thread::Builder::new().spawn(move || {
        let observed = (|| -> std::io::Result<()> {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            loop {
                // Observe completion without reaping, retaining ownership of the
                // PID until the caller terminates the group. No status polling.
                let rc = unsafe {
                    libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT)
                };
                if rc == 0 {
                    return Ok(());
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        })();
        let _ = tx.send(observed);
    })?;
    let observed = rx.recv_timeout(timeout);
    if matches!(&observed, Ok(Err(error)) if error.raw_os_error() == Some(libc::ECHILD)) {
        // A child reaped outside this adapter no longer proves PID ownership.
        child.1 = true;
        let _ = waiter.join();
        anyhow::bail!("gh child ownership lost before completion");
    }
    // Do not reap while the waitid thread still owns a wait on this PID.
    if let Ok(pid) = i32::try_from(child.0.id())
        && pid > 1
    {
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
    let _ = child.0.kill();
    let joined = waiter.join();
    let status = child.0.wait();
    child.1 = true;
    observed.context("gh mutation deadline exceeded or waiter disconnected")??;
    joined.map_err(|_| anyhow::anyhow!("gh completion waiter panicked"))?;
    status.context("reap gh mutation")
}

pub(super) fn created_issue(output: &str, repository: &str) -> Result<IssueRef> {
    let prefix = format!("https://github.com/{repository}/issues/");
    let number = output
        .trim()
        .strip_prefix(&prefix)
        .context("gh returned an invalid or foreign issue URL")?;
    // A service receipt must be a URL with a decimal path component. Optional
    // input placeholders and whitespace inside the URL are not valid receipts.
    ensure!(
        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()),
        "gh returned an invalid issue URL"
    );
    let number = parse_optional(&json!(number))?.context("gh returned no issue number")?;
    Ok(IssueRef {
        repository: repository.to_owned(),
        number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_2553_service_receipts_require_an_exact_repository_and_decimal_path() {
        let repository = "example/operations";
        let prefix = format!("https://github.com/{repository}/issues/");
        for invalid in [
            "0",
            "-1",
            "none",
            " 2553",
            "25 53",
            "2553/",
            "2553?x=1",
            "2553#comment",
            "18446744073709551616",
            "2553\n2554",
        ] {
            assert!(
                created_issue(&format!("{prefix}{invalid}"), repository).is_err(),
                "{invalid}"
            );
        }
        for invalid in [
            "https://github.com/example/foreign/issues/2553",
            "https://foreign.invalid/example/operations/issues/2553",
            "http://github.com/example/operations/issues/2553",
        ] {
            assert!(created_issue(invalid, repository).is_err());
        }
        assert_eq!(
            created_issue(&format!("{prefix}18446744073709551615\n"), repository)
                .unwrap()
                .number
                .get(),
            u64::MAX
        );
    }

    #[test]
    fn issue_2553_service_output_capture_is_bounded() {
        assert_eq!(read(&b"receipt"[..]).unwrap(), b"receipt");
        assert!(read(std::io::repeat(b'x')).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn issue_2553_mutation_deadline_kills_and_reaps_the_owned_child() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::process::CommandExt;
        let home = tempfile::tempdir().unwrap();
        let mut command = Command::new("/bin/cat");
        command
            .env_clear()
            .env("HOME", home.path())
            .current_dir(home.path())
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = OwnedChild(command.spawn().unwrap(), false);
        let mut input = child.0.stdin.take().unwrap();
        let mut output = BufReader::new(child.0.stdout.take().unwrap());
        input.write_all(b"ready\n").unwrap();
        let mut ready = String::new();
        output.read_line(&mut ready).unwrap();
        assert_eq!(ready, "ready\n");
        // stdin remains open, so the ready child cannot exit naturally. A zero
        // budget tests the deadline without a timing window or polling.
        let error = wait(&mut child, Duration::ZERO).unwrap_err();
        assert!(error.to_string().contains("deadline"), "{error:#}");
        assert!(child.1, "the adapter must reap the child before returning");
        assert!(!child.0.try_wait().unwrap().unwrap().success());
    }

    #[cfg(unix)]
    #[test]
    fn issue_2553_large_mutation_input_and_output_do_not_deadlock() {
        let home = tempfile::tempdir().unwrap();
        let mut command = Command::new("/bin/cat");
        command
            .env_clear()
            .env("HOME", home.path())
            .current_dir(home.path());
        let error = execute(&mut command, Some(&"x".repeat(256 * 1024)), home.path()).unwrap_err();
        assert!(error.to_string().contains("capture limit"), "{error:#}");
    }

    #[cfg(unix)]
    #[test]
    fn issue_2553_mutation_body_is_passed_literally() {
        let home = tempfile::tempdir().unwrap();
        let mut command = Command::new("/bin/cat");
        command
            .env_clear()
            .env("HOME", home.path())
            .current_dir(home.path());
        let body = "Literal $(touch injected) and `false`.\n";
        assert_eq!(
            execute(&mut command, Some(body), home.path()).unwrap(),
            body
        );
        assert!(!home.path().join("injected").exists());
    }
}
