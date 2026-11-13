//! Runs a wrapped command as an argument vector, with no shell.
//!
//! Standard output goes to an unlinked 0600 spool file in the recovery
//! directory, so a large output does not grow memory; only its tail is read
//! back for the excerpt. Standard error passes through to the step log and
//! its tail is kept for the excerpt.

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

/// Bytes of each stream kept for the excerpt.
const TAIL_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Exit(i32),
    Signal(i32),
    /// The runner does not expose a failed sub-recipe's status.
    Unavailable,
}

/// A failed run: its status and the raw (not yet redacted) output tail.
#[derive(Debug, Clone)]
pub struct Failed {
    pub status: Status,
    pub output_tail: String,
}

pub enum Ran {
    /// Exit 0; the spool holds the command's standard output.
    Success(File),
    Failed(Failed),
}

#[cfg(unix)]
fn spool(dir: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = dir.join(format!(".spool-{}", uuid::Uuid::new_v4().simple()));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .with_context(|| format!("cannot create spool file in {}", dir.display()))?;
    std::fs::remove_file(&path)
        .with_context(|| format!("cannot unlink spool file {}", path.display()))?;
    Ok(file)
}

#[cfg(not(unix))]
fn spool(_: &Path) -> Result<File> {
    anyhow::bail!("step recovery requires Unix")
}

#[cfg(unix)]
fn signal_of(status: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn signal_of(_: &ExitStatus) -> Option<i32> {
    None
}

/// Keeps the last `TAIL_BYTES` of a byte stream.
fn keep_tail(buf: &mut Vec<u8>, chunk: &[u8]) {
    buf.extend_from_slice(chunk);
    if buf.len() > TAIL_BYTES {
        let cut = buf.len() - TAIL_BYTES;
        buf.drain(..cut);
    }
}

/// Text of a tail that may start mid-line: the partial first line is dropped
/// so a secret cut in half cannot slip past redaction.
fn tail_text(bytes: &[u8], cut: bool) -> String {
    let text = String::from_utf8_lossy(bytes);
    match (cut, text.find('\n')) {
        (true, Some(at)) => text[at + 1..].to_string(),
        (true, None) => String::new(),
        (false, _) => text.into_owned(),
    }
}

fn spool_tail(file: &mut File) -> Result<String> {
    let len = file.seek(SeekFrom::End(0))?;
    let start = len.saturating_sub(TAIL_BYTES as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(tail_text(&bytes, start > 0))
}

/// Runs `command` once.
pub fn execute(dir: &Path, command: &[String]) -> Result<Ran> {
    let (program, args) = command.split_first().context("no command after --")?;
    let mut out = spool(dir)?;
    let spawned = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(out.try_clone()?)
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            let message = format!("cannot start {program}: {e}");
            eprintln!("[step-recovery] {message}");
            return Ok(Ran::Failed(Failed {
                status: Status::Exit(127),
                output_tail: message,
            }));
        }
    };
    let mut stderr = child.stderr.take().context("no child stderr")?;
    let reader = std::thread::spawn(move || -> std::io::Result<(Vec<u8>, bool)> {
        let mut tail = Vec::new();
        let mut cut = false;
        let mut chunk = [0u8; 8192];
        let mut sink = std::io::stderr();
        loop {
            let n = stderr.read(&mut chunk)?;
            if n == 0 {
                return Ok((tail, cut));
            }
            sink.write_all(&chunk[..n])?;
            if tail.len() + n > TAIL_BYTES {
                cut = true;
            }
            keep_tail(&mut tail, &chunk[..n]);
        }
    });
    let status = child
        .wait()
        .context("cannot wait for the wrapped command")?;
    let (err_tail, err_cut) = reader
        .join()
        .map_err(|_| anyhow::anyhow!("stderr reader panicked"))?
        .context("cannot read the wrapped command's stderr")?;
    if status.success() {
        out.seek(SeekFrom::Start(0))?;
        return Ok(Ran::Success(out));
    }
    let status = match (status.code(), signal_of(&status)) {
        (Some(code), _) => Status::Exit(code),
        (None, Some(signal)) => Status::Signal(signal),
        (None, None) => Status::Exit(1),
    };
    let mut parts = Vec::new();
    let out_tail = spool_tail(&mut out)?;
    if !out_tail.trim().is_empty() {
        parts.push(out_tail);
    }
    let err_tail = tail_text(&err_tail, err_cut);
    if !err_tail.trim().is_empty() {
        parts.push(err_tail);
    }
    Ok(Ran::Failed(Failed {
        status,
        output_tail: parts.join("\n"),
    }))
}

/// Copies a successful run's standard output to ours, byte for byte.
pub fn pass_through(mut spool: File) -> Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    std::io::copy(&mut spool, &mut lock).context("cannot pass the command's output on")?;
    lock.flush()?;
    Ok(())
}
