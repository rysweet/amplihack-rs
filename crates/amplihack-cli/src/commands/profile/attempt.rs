//! One supervised attempt of a nested `-p` launch, shared by the auto-switch
//! rerun supervisor (#1569) and the usage-limit wait (#1618).
//!
//! An attempt runs the same amplihack command line as a child with
//! `AMPLIHACK_AUTO_SWITCH_SUPERVISED=1`, reads its stdin from the step's
//! stdin copy, streams its stderr while keeping the last 64 KiB, and buffers
//! its stdout in a private file that is printed only for the final attempt.

use super::PROFILE_ENV;
use super::auto_switch::SUPERVISED_ENV;
use super::fsx;
use super::limit_wait::scratch::StdinCopy;
use super::name::ProfileName;
use crate::commands::recipe::run::limit_patterns::{self, Harness};
use crate::commands::recipe::run::{FailureClass, classify_failure_text};
use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

const TAIL: usize = 64 * 1024;

pub(crate) struct Attempt {
    code: i32,
    signalled: bool,
    stderr_tail: Vec<u8>,
    stdout: fs::File,
    stdout_path: PathBuf,
}

impl Attempt {
    /// Prints the buffered stdout and returns the exit code, or `code` when
    /// given.
    pub(crate) fn finish(self, code: Option<i32>) -> Result<Option<i32>> {
        let mut out = self.stdout;
        out.seek(SeekFrom::Start(0))?;
        let mut stdout = std::io::stdout().lock();
        std::io::copy(&mut out, &mut stdout)?;
        stdout.flush()?;
        drop(out);
        let _ = fs::remove_file(&self.stdout_path);
        Ok(Some(code.unwrap_or(self.code)))
    }

    /// Drops the buffered stdout of an attempt that is run again.
    pub(crate) fn discard(self) {
        drop(self.stdout);
        let _ = fs::remove_file(&self.stdout_path);
    }
}

/// Stop signals, caught once for the whole supervised run. signal-hook does
/// not restore the default action on unregister, so handlers registered per
/// attempt would leave the supervisor deaf between attempts.
pub(crate) struct Signals {
    term: Arc<AtomicBool>,
    hup: Arc<AtomicBool>,
    /// SIGINT reaches the attempt through the terminal; catching it here
    /// keeps the supervisor alive to pass the attempt's output back.
    int: Arc<AtomicBool>,
    /// The last stop signal seen, or 0.
    seen: AtomicI32,
    ids: Vec<signal_hook::SigId>,
}

impl Signals {
    pub(crate) fn register() -> std::io::Result<Self> {
        let mut s = Self {
            term: Arc::new(AtomicBool::new(false)),
            hup: Arc::new(AtomicBool::new(false)),
            int: Arc::new(AtomicBool::new(false)),
            seen: AtomicI32::new(0),
            ids: Vec::new(),
        };
        for (sig, flag) in [
            (libc::SIGTERM, Arc::clone(&s.term)),
            (libc::SIGHUP, Arc::clone(&s.hup)),
            (libc::SIGINT, Arc::clone(&s.int)),
        ] {
            s.ids.push(signal_hook::flag::register(sig, flag)?);
        }
        Ok(s)
    }

    /// Passes SIGTERM and SIGHUP on to `pid` and remembers every stop signal.
    fn forward(&self, pid: libc::pid_t) {
        for (flag, sig) in [(&self.term, libc::SIGTERM), (&self.hup, libc::SIGHUP)] {
            if flag.swap(false, Ordering::Relaxed) {
                self.seen.store(sig, Ordering::Relaxed);
                // SAFETY: plain kill(2) on our own child's pid.
                unsafe { libc::kill(pid, sig) };
            }
        }
        if self.int.swap(false, Ordering::Relaxed) {
            self.seen.store(libc::SIGINT, Ordering::Relaxed);
        }
    }

    /// The last stop signal received so far, including any not yet forwarded.
    /// No attempt is running here, so nothing is forwarded.
    pub(crate) fn received(&self) -> Option<i32> {
        for (flag, sig) in [
            (&self.term, libc::SIGTERM),
            (&self.hup, libc::SIGHUP),
            (&self.int, libc::SIGINT),
        ] {
            if flag.swap(false, Ordering::Relaxed) {
                self.seen.store(sig, Ordering::Relaxed);
            }
        }
        match self.seen.load(Ordering::Relaxed) {
            0 => None,
            sig => Some(sig),
        }
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

/// Runs attempt `n`. `env` is a profile to switch to and its directory;
/// `None` keeps the inherited environment.
pub(crate) fn run_attempt(
    exe: &Path,
    args: &[std::ffi::OsString],
    rerun_dir: &Path,
    env: Option<(&ProfileName, &Path)>,
    stdin: &StdinCopy,
    n: u32,
    signals: &Signals,
) -> Result<Attempt> {
    let stdout_path = rerun_dir.join(format!("{}-{n}.out", std::process::id()));
    let mut opts = fs::OpenOptions::new();
    opts.read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let stdout = opts.open(&stdout_path)?;
    let mut cmd = Command::new(exe);
    let input = stdin.stdio();
    let spawned = input.and_then(|input| {
        cmd.args(args)
            .env(SUPERVISED_ENV, "1")
            .stdin(input)
            .stdout(stdout.try_clone()?)
            .stderr(Stdio::piped());
        if let Some((profile, dir)) = env {
            cmd.env(PROFILE_ENV, profile.as_str())
                .env("CLAUDE_CONFIG_DIR", dir);
        }
        cmd.spawn()
    });
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            drop(stdout);
            let _ = fs::remove_file(&stdout_path);
            return Err(e).context("failed to start the supervised launch");
        }
    };
    let mut pipe = child.stderr.take().context("no stderr pipe")?;
    let reader = std::thread::spawn(move || {
        let mut tail: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        let mut err = std::io::stderr();
        while let Ok(k) = pipe.read(&mut buf) {
            if k == 0 {
                break;
            }
            let _ = err.write_all(&buf[..k]);
            tail.extend_from_slice(&buf[..k]);
            // Trim only once the buffer doubles, so each byte is moved at
            // most once instead of 64 KiB being shifted on every read.
            if tail.len() > 2 * TAIL {
                tail.drain(..tail.len() - TAIL);
            }
        }
        if tail.len() > TAIL {
            tail.drain(..tail.len() - TAIL);
        }
        tail
    });

    let status = loop {
        signals.forward(child.id() as libc::pid_t);
        if let Some(s) = child.try_wait()? {
            break s;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let stderr_tail = reader.join().unwrap_or_default();
    use std::os::unix::process::ExitStatusExt;
    let (code, signalled) = match (status.code(), status.signal()) {
        (Some(c), _) => (c, false),
        (None, Some(sig)) => (128 + sig, true),
        (None, None) => (1, true),
    };
    Ok(Attempt {
        code,
        signalled,
        stderr_tail,
        stdout,
        stdout_path,
    })
}

/// What an attempt that hit its usage limit said about it.
pub(crate) struct Limited {
    pub(crate) harness: Harness,
    /// For the auto-switch ledger: the stated reset or five hours from now.
    pub(crate) ledger_reset: i64,
    /// For the wait: the stated reset capped at `now` + 5 h; `None` unparsed.
    pub(crate) wait_reset: Option<i64>,
}

/// `Some` when `attempt` hit its usage limit. `now` is the wait's clock.
pub(crate) fn usage_limited(attempt: &Attempt, now: i64) -> Option<Limited> {
    if attempt.signalled || attempt.code == 0 {
        return None; // no need to read the output
    }
    let out_tail = tail(&attempt.stdout).unwrap_or_else(|e| {
        eprintln!("amplihack: could not read this step's buffered output: {e}");
        Vec::new()
    });
    let (err, out) = (&attempt.stderr_tail, &out_tail);
    let ledger_reset = limit_reset(attempt.signalled, attempt.code, err, out, fsx::now())?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(err),
        String::from_utf8_lossy(out)
    );
    Some(Limited {
        harness: limit_patterns::detect(&text).unwrap_or(Harness::Claude),
        ledger_reset,
        wait_reset: limit_patterns::read_reset(err, out, now),
    })
}

/// An attempt hit its usage limit only when it was not killed by a signal,
/// exited non-zero, and the recipe runner's classifier calls its stderr tail
/// followed by its stdout tail `UsageLimit`. Work and environmental markers
/// outrank a usage limit there, so a real failure that also mentions one is
/// not rerun. Returns the reset time to record.
pub fn limit_reset(
    signalled: bool,
    code: i32,
    stderr: &[u8],
    stdout: &[u8],
    now: i64,
) -> Option<i64> {
    if signalled || code == 0 {
        return None;
    }
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(stderr),
        String::from_utf8_lossy(stdout)
    );
    (classify_failure_text(&text).0 == FailureClass::UsageLimit)
        .then(|| super::auto_switch::reset_or_default(stderr, stdout, now))
}

/// The last 64 KiB of the buffered stdout, read through the attempt's own
/// handle rather than by reopening its path.
fn tail(file: &fs::File) -> std::io::Result<Vec<u8>> {
    let mut f = file.try_clone()?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL as u64)))?;
    fsx::read_bounded(f, TAIL as u64)
}
